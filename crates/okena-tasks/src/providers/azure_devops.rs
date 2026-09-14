//! Azure DevOps provider — REST over `okena_transport::http`.
//!
//! Azure DevOps Services only (`dev.azure.com/<org>` and the legacy
//! `<org>.visualstudio.com`), authenticated with a personal access token sent
//! as HTTP Basic. The token is bound to one organization, so the credential
//! carries the organization URL alongside it.
//!
//! Three things differ from Linear enough to shape this module:
//!
//! - **Kinds are native.** A work item has a type, and which type means "story"
//!   depends on the project's process template (Agile's User Story is Scrum's
//!   Product Backlog Item). Reading maps any type back to a kind; creating
//!   first finds the project's process — following an inherited process to the
//!   system template it came from — and picks that template's type.
//! - **States are per type.** Every state belongs to a category (Proposed,
//!   InProgress, Resolved, Completed, Removed), and the categories are what map
//!   onto [`TaskState`]. They are looked up per project and type, once per call.
//! - **Descriptions are HTML**, converted to Markdown for the description pane.

use super::html;
use crate::provider::{
    AuthStatus, Credential, TaskContainer, TaskDraft, TaskError, TaskPatch, TaskProvider,
};
use base64::Engine as _;
use okena_core::tasks::{GroupAxis, Task, TaskGroup, TaskId, TaskKind, TaskState};
use okena_transport::http::{self, HttpError, HttpRequest, HttpResponse, Method};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use std::time::Duration;

pub const PROVIDER_ID: &str = "azure_devops";
const API_VERSION: &str = "7.1";
/// Client-side rate floor on the queue poll, as for Linear: it only catches a
/// runaway caller.
const MIN_INTERVAL: Duration = Duration::from_secs(5);
const TIMEOUT: Duration = Duration::from_secs(20);
/// `workitemsbatch` takes at most 200 ids per request.
const BATCH_SIZE: usize = 200;
/// How much of the assigned queue to fetch. A personal queue past this is not
/// a queue anyone works through.
const QUEUE_LIMIT: usize = 200;
const JSON_PATCH: &str = "application/json-patch+json";

/// Assigned work, most recently changed first.
///
/// WIQL cannot filter on a state *category*, so the common closed state names
/// are left out server-side — which keeps a long finished history from being
/// transferred — and the real category check runs on what comes back, so a
/// custom closed state is still excluded.
const QUERY_ASSIGNED: &str = "SELECT [System.Id] FROM WorkItems \
     WHERE [System.AssignedTo] = @Me \
     AND [System.State] NOT IN ('Closed', 'Done', 'Removed') \
     ORDER BY [System.ChangedDate] DESC";

const CHILD_LINK: &str = "System.LinkTypes.Hierarchy-Forward";
const PARENT_LINK: &str = "System.LinkTypes.Hierarchy-Reverse";

/// The system process templates every Azure DevOps process descends from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProcessTemplate {
    Agile,
    Scrum,
    Cmmi,
    Basic,
}

impl ProcessTemplate {
    /// Well-known type ids of the system processes.
    fn from_type_id(id: &str) -> Option<Self> {
        match id.to_ascii_lowercase().as_str() {
            "adcc42ab-9882-485e-a3ed-7678f01f66bc" => Some(Self::Agile),
            "6b724908-ef14-45cf-84f8-768b5384da45" => Some(Self::Scrum),
            "27450541-8e31-4150-9947-dc59f998fc01" => Some(Self::Cmmi),
            "b8a3a935-7e91-48b8-a94c-606d37c3e9f2" => Some(Self::Basic),
            _ => None,
        }
    }

    /// A system process by name. Only trusted for a `system` process: an
    /// inherited one is named by whoever created it.
    fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "agile" => Some(Self::Agile),
            "scrum" => Some(Self::Scrum),
            "cmmi" => Some(Self::Cmmi),
            "basic" => Some(Self::Basic),
            _ => None,
        }
    }

    /// The native work item type for a kind. Basic has no Feature or Bug, so
    /// those take the nearest type it does have.
    fn work_item_type(self, kind: TaskKind) -> &'static str {
        match (self, kind) {
            (_, TaskKind::Epic) => "Epic",
            (Self::Basic, TaskKind::Feature) => "Epic",
            (_, TaskKind::Feature) => "Feature",
            (Self::Agile, TaskKind::Story) => "User Story",
            (Self::Scrum, TaskKind::Story) => "Product Backlog Item",
            (Self::Cmmi, TaskKind::Story) => "Requirement",
            (Self::Basic, TaskKind::Story | TaskKind::Defect) => "Issue",
            (_, TaskKind::Defect) => "Bug",
            (_, TaskKind::Task) => "Task",
        }
    }
}

/// A work item type back to a kind, whatever template it came from.
///
/// Basic's Issue reads as a story, since that is what it stands in for.
fn kind_from_type(work_item_type: &str) -> TaskKind {
    match work_item_type.trim().to_ascii_lowercase().as_str() {
        "epic" => TaskKind::Epic,
        "feature" => TaskKind::Feature,
        "user story" | "product backlog item" | "requirement" | "issue" => TaskKind::Story,
        "bug" => TaskKind::Defect,
        _ => TaskKind::Task,
    }
}

/// Find the system template behind a process, following inherited processes
/// to their parent.
fn resolve_template(process: &Value, all: &[Value]) -> Option<ProcessTemplate> {
    let mut current = process;
    // Inherited processes derive from a system one directly; the bound only
    // guards against a malformed response that points a process at itself.
    for _ in 0..4 {
        let type_id = current.get("typeId").and_then(Value::as_str).unwrap_or("");
        if let Some(t) = ProcessTemplate::from_type_id(type_id) {
            return Some(t);
        }
        let customization = current
            .get("customizationType")
            .and_then(Value::as_str)
            .unwrap_or("");
        if customization.eq_ignore_ascii_case("system")
            && let Some(t) = current
                .get("name")
                .and_then(Value::as_str)
                .and_then(ProcessTemplate::from_name)
        {
            return Some(t);
        }
        let parent = current
            .get("parentProcessTypeId")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty() && p.chars().any(|c| c != '0' && c != '-'))?;
        if let Some(t) = ProcessTemplate::from_type_id(parent) {
            return Some(t);
        }
        current = all.iter().find(|p| {
            p.get("typeId")
                .and_then(Value::as_str)
                .is_some_and(|id| id.eq_ignore_ascii_case(parent))
        })?;
    }
    None
}

/// One workflow state of a work item type.
#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkflowState {
    name: String,
    category: String,
}

fn map_category(category: &str) -> TaskState {
    match category {
        "Proposed" => TaskState::Todo,
        "InProgress" => TaskState::InProgress,
        "Resolved" => TaskState::InReview,
        "Completed" => TaskState::Done,
        "Removed" => TaskState::Canceled,
        _ => TaskState::Unknown,
    }
}

/// State categories to move to for a normalized state, best first.
///
/// Review falls back to in-progress because several types (Scrum's Product
/// Backlog Item, every Task) have no Resolved category at all.
fn target_categories(state: TaskState) -> &'static [&'static str] {
    match state {
        TaskState::Backlog | TaskState::Todo | TaskState::Unknown => &["Proposed"],
        TaskState::InProgress => &["InProgress"],
        TaskState::InReview => &["Resolved", "InProgress"],
        TaskState::Done => &["Completed"],
        TaskState::Canceled => &["Removed"],
    }
}

/// The state to set. States come back in workflow order, so the first one in
/// a category is the type's default entry into it.
fn choose_state<'a>(states: &'a [WorkflowState], categories: &[&str]) -> Option<&'a str> {
    categories.iter().find_map(|cat| {
        states
            .iter()
            .find(|s| s.category == *cat)
            .map(|s| s.name.as_str())
    })
}

/// Turn whatever the user pasted into an organization root.
///
/// Accepts the forms people copy out of a browser or a clone URL —
/// `https://dev.azure.com/contoso/Project/_workitems`, `dev.azure.com/contoso`,
/// `https://me@dev.azure.com/contoso/…`, `contoso.visualstudio.com` — and
/// refuses anything that is not Azure DevOps Services.
pub fn normalize_organization_url(raw: &str) -> Result<String, String> {
    const SERVICES_ONLY: &str = "Only Azure DevOps Services is supported: \
         https://dev.azure.com/<organization> or https://<organization>.visualstudio.com";
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Enter your Azure DevOps organization URL.".into());
    }
    let rest = match trimmed.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("https") => rest,
        Some(_) => return Err(SERVICES_ONLY.into()),
        None => trimmed,
    };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    // Clone URLs carry a user name: `https://contoso@dev.azure.com/contoso/…`.
    let host = authority
        .rsplit('@')
        .next()
        .unwrap_or(authority)
        .to_ascii_lowercase();
    let path = path.split(['?', '#']).next().unwrap_or("");
    let valid = |org: &str| {
        !org.is_empty()
            && org
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };

    if host == "dev.azure.com" {
        let org = path
            .split('/')
            .find(|s| !s.is_empty())
            .ok_or("Include the organization in the URL, e.g. https://dev.azure.com/contoso.")?;
        if !valid(org) {
            return Err(format!("`{org}` is not an Azure DevOps organization name."));
        }
        return Ok(format!("https://dev.azure.com/{org}"));
    }
    if let Some(org) = host.strip_suffix(".visualstudio.com") {
        if !valid(org) {
            return Err(SERVICES_ONLY.into());
        }
        return Ok(format!("https://{org}.visualstudio.com"));
    }
    Err(SERVICES_ONLY.into())
}

/// Percent-encode one URL path segment. Project and type names have spaces.
fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn basic_auth(token: &str) -> String {
    // A PAT goes in the password slot with an empty user name.
    let encoded = base64::engine::general_purpose::STANDARD.encode(format!(":{token}"));
    format!("Basic {encoded}")
}

/// A work item id as the provider takes it: a positive integer, with or
/// without the `#` its display key carries.
fn work_item_id(raw: &str) -> Result<u64, TaskError> {
    raw.trim()
        .trim_start_matches('#')
        .parse()
        .map_err(|_| TaskError::Protocol {
            provider: PROVIDER_ID,
            message: format!("`{raw}` is not an Azure DevOps work item id"),
        })
}

enum Payload {
    None,
    Json(Value),
    JsonPatch(Value),
}

/// The server's own explanation, when the error body carries one.
fn server_message(resp: &HttpResponse) -> Option<String> {
    let body: Value = resp.json().ok()?;
    body.get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
}

/// Read a response, mapping Azure DevOps' ways of saying "bad token".
///
/// A rejected PAT comes back as 401, or as a 203 carrying the HTML sign-in
/// page. A 403 is *not* mapped to `Unauthorized`: it usually means the token is
/// fine but lacks a permission for this one action, and flipping the whole
/// harness to "reconnect" would hide that.
fn read_response(resp: HttpResponse) -> Result<Value, TaskError> {
    let status = resp.status();
    if status == 401 || status == 203 {
        return Err(TaskError::Unauthorized {
            provider: PROVIDER_ID,
        });
    }
    if status == 404 {
        return Err(TaskError::Transport {
            provider: PROVIDER_ID,
            message: server_message(&resp)
                .unwrap_or_else(|| "not found (HTTP 404) — check the organization URL".to_string()),
        });
    }
    if !resp.is_success() {
        let message = match server_message(&resp) {
            Some(m) => format!("{m} (HTTP {status})"),
            None => format!("HTTP {status}"),
        };
        return Err(TaskError::Protocol {
            provider: PROVIDER_ID,
            message,
        });
    }
    if resp.bytes().iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'<') {
        // A sign-in page served with 200, which some redirects produce.
        return Err(TaskError::Unauthorized {
            provider: PROVIDER_ID,
        });
    }
    resp.json().map_err(|e| TaskError::Protocol {
        provider: PROVIDER_ID,
        message: e.to_string(),
    })
}

fn protocol(message: impl Into<String>) -> TaskError {
    TaskError::Protocol {
        provider: PROVIDER_ID,
        message: message.into(),
    }
}

/// A path under a project, with its name encoded. Area and iteration paths
/// start with the project name; the filter shows the part after it.
fn path_below_project<'a>(path: &'a str, project: &str) -> Option<&'a str> {
    let rest = match path.strip_prefix(project) {
        Some(rest) => rest.strip_prefix('\\')?,
        None => path,
    };
    let rest = rest.trim();
    (!rest.is_empty()).then_some(rest)
}

/// A work item's groupings: team project, area path, iteration path.
///
/// The root area and root iteration are the project itself — an item nobody
/// filed anywhere more specific — so they are left out rather than shown as a
/// filter row that repeats the project name.
fn parse_groups(fields: &Value) -> Vec<TaskGroup> {
    let s = |k: &str| fields.get(k).and_then(Value::as_str).unwrap_or("").trim();
    let project = s("System.TeamProject");
    let mut groups = Vec::new();
    if !project.is_empty() {
        groups.push(TaskGroup::new(GroupAxis::Team, project, project));
    }
    for (field, axis) in [
        ("System.AreaPath", GroupAxis::Project),
        ("System.IterationPath", GroupAxis::Iteration),
    ] {
        let path = s(field);
        if let Some(name) = path_below_project(path, project) {
            groups.push(TaskGroup::new(axis, path, name));
        }
    }
    groups
}

/// Parse one work item. `category` is its state's category, looked up by the
/// caller; `None` reads as an unknown state rather than a guessed one.
fn parse_work_item(item: &Value, organization_url: &str, category: Option<&str>) -> Option<Task> {
    let id = item.get("id")?.as_u64()?;
    let fields = item.get("fields")?;
    let s = |k: &str| fields.get(k).and_then(Value::as_str).unwrap_or("");
    let project = s("System.TeamProject").trim();
    let work_item_type = s("System.WorkItemType");

    // A bug's body is its repro steps; everything else keeps the description.
    let body_fields = if work_item_type.eq_ignore_ascii_case("bug") {
        ["Microsoft.VSTS.TCM.ReproSteps", "System.Description"]
    } else {
        ["System.Description", "Microsoft.VSTS.TCM.ReproSteps"]
    };
    let description = body_fields
        .iter()
        .map(|k| s(k))
        .find(|d| !d.trim().is_empty())
        .map(html::to_markdown)
        .filter(|d| !d.is_empty());

    let labels = s("System.Tags")
        .split(';')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    // The field when it came back, else the hierarchy link: the batch
    // documents no default field set, and a list whose parents silently went
    // missing rendered every breakdown flat.
    let parent = fields
        .get("System.Parent")
        .and_then(Value::as_u64)
        .or_else(|| linked_ids(item, PARENT_LINK).first().copied());
    let url = if project.is_empty() {
        format!("{organization_url}/_workitems/edit/{id}")
    } else {
        format!(
            "{organization_url}/{}/_workitems/edit/{id}",
            encode_segment(project)
        )
    };

    let kind = kind_from_type(work_item_type);
    let display_key = format!("#{id}");
    let title = s("System.Title");

    Some(Task {
        id: TaskId::new(PROVIDER_ID, id.to_string()),
        branch_name: crate::provider::task_branch_name(kind, &display_key, title),
        display_key,
        title: title.to_string(),
        description,
        state: category.map_or(TaskState::Unknown, map_category),
        state_name: s("System.State").to_string(),
        url,
        updated_at: s("System.ChangedDate").to_string(),
        kind,
        parent_id: parent.map(|p| p.to_string()),
        parent_key: parent.map(|p| format!("#{p}")),
        labels,
        groups: parse_groups(fields),
    })
}

/// Ids of a work item's children, from its expanded relations.
fn child_ids(item: &Value) -> Vec<u64> {
    linked_ids(item, CHILD_LINK)
}

/// Ids a work item links to with `rel`, from its expanded relations.
fn linked_ids(item: &Value, rel: &str) -> Vec<u64> {
    item.get("relations")
        .and_then(Value::as_array)
        .map(|relations| {
            relations
                .iter()
                .filter(|r| r.get("rel").and_then(Value::as_str) == Some(rel))
                .filter_map(|r| r.get("url")?.as_str()?.rsplit('/').next()?.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// The field a work item type keeps its body in. A bug's form shows Repro
/// Steps, not Description.
fn body_field(work_item_type: &str) -> &'static str {
    if work_item_type.eq_ignore_ascii_case("bug") {
        "Microsoft.VSTS.TCM.ReproSteps"
    } else {
        "System.Description"
    }
}

/// The JSON Patch that edits a work item: only the fields the patch changes.
fn update_operations(patch: &TaskPatch, work_item_type: &str) -> Value {
    let mut ops = Vec::new();
    if let Some(title) = patch.title.as_deref() {
        ops.push(json!({ "op": "add", "path": "/fields/System.Title", "value": title.trim() }));
    }
    if let Some(body) = patch.description.as_deref() {
        let path = format!("/fields/{}", body_field(work_item_type));
        ops.push(json!({ "op": "add", "path": path, "value": html::from_text(body) }));
    }
    Value::Array(ops)
}

/// The JSON Patch that creates a work item.
fn create_operations(
    draft: &TaskDraft,
    work_item_type: &str,
    parent: Option<u64>,
    organization_url: &str,
) -> Value {
    let mut ops = vec![json!({
        "op": "add", "path": "/fields/System.Title", "value": draft.title.trim(),
    })];
    if let Some(body) = draft
        .description
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        let path = format!("/fields/{}", body_field(work_item_type));
        ops.push(json!({ "op": "add", "path": path, "value": html::from_text(body) }));
    }
    if let Some(parent) = parent {
        ops.push(json!({
            "op": "add",
            "path": "/relations/-",
            "value": {
                "rel": PARENT_LINK,
                "url": format!("{organization_url}/_apis/wit/workItems/{parent}"),
            },
        }));
    }
    Value::Array(ops)
}

pub struct AzureDevOpsProvider {
    credential: Option<Credential>,
    /// Account name learned during this instance's calls, when the credential
    /// did not already record one.
    account: RwLock<Option<String>>,
    /// Workflow states per (project, work item type). Lives only as long as
    /// the provider, which is built per call, so it can never go stale.
    states: Mutex<HashMap<(String, String), Vec<WorkflowState>>>,
}

impl AzureDevOpsProvider {
    pub fn new(credential: Option<Credential>) -> Self {
        Self {
            credential,
            account: RwLock::new(None),
            states: Mutex::new(HashMap::new()),
        }
    }

    /// The account name, from this instance's calls or the stored credential.
    pub fn account(&self) -> Option<String> {
        if let Some(a) = self.account.read().ok().and_then(|a| a.clone()) {
            return Some(a);
        }
        match &self.credential {
            Some(Credential::PersonalAccessToken { account, .. }) => account.clone(),
            _ => None,
        }
    }

    fn session(&self) -> Result<(&str, &str), TaskError> {
        match &self.credential {
            None => Err(TaskError::NotAuthenticated {
                provider: PROVIDER_ID,
            }),
            Some(Credential::PersonalAccessToken {
                token,
                organization_url,
                ..
            }) => Ok((token, organization_url)),
            Some(_) => Err(TaskError::Unauthorized {
                provider: PROVIDER_ID,
            }),
        }
    }

    fn organization_url(&self) -> Result<&str, TaskError> {
        self.session().map(|(_, org)| org)
    }

    /// Issue one request against the organization. `path` starts with `/` and
    /// may carry a query; the API version is added unless it names its own.
    fn call(
        &self,
        label: &'static str,
        method: Method,
        path: &str,
        payload: Payload,
    ) -> Result<Value, TaskError> {
        let (token, org) = self.session()?;
        let url = if path.contains("api-version=") {
            format!("{org}{path}")
        } else {
            let sep = if path.contains('?') { '&' } else { '?' };
            format!("{org}{path}{sep}api-version={API_VERSION}")
        };
        let mut req = HttpRequest::new(method, url)
            .header("Authorization", basic_auth(token))
            .header("Accept", "application/json")
            .label(label)
            .timeout(TIMEOUT);
        if label == "azure_devops.assigned" {
            req = req.min_interval(MIN_INTERVAL);
        }
        req = match payload {
            Payload::None => req,
            Payload::Json(v) => req.json(&v),
            Payload::JsonPatch(v) => req.body(JSON_PATCH, v.to_string()),
        };

        let resp = http::send(req).map_err(|e| match e {
            HttpError::Status(401) => TaskError::Unauthorized {
                provider: PROVIDER_ID,
            },
            other => TaskError::Transport {
                provider: PROVIDER_ID,
                message: other.to_string(),
            },
        })?;
        read_response(resp)
    }

    /// Who the token belongs to. Best effort: a failure here must not fail a
    /// poll whose real work already succeeded.
    fn learn_account(&self) {
        if self.account().is_some() {
            return;
        }
        let name = self
            .call(
                "azure_devops.connection",
                Method::Get,
                "/_apis/connectionData?api-version=7.1-preview",
                Payload::None,
            )
            .ok()
            .and_then(|data| {
                let user = data.get("authenticatedUser")?;
                user.get("providerDisplayName")
                    .or_else(|| user.get("customDisplayName"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        if let Some(name) = name
            && let Ok(mut slot) = self.account.write()
        {
            *slot = Some(name);
        }
    }

    /// A type's workflow states in order, fetched once per instance.
    fn workflow_states(
        &self,
        project: &str,
        work_item_type: &str,
    ) -> Result<Vec<WorkflowState>, TaskError> {
        let key = (project.to_string(), work_item_type.to_string());
        if let Some(hit) = self.states.lock().ok().and_then(|m| m.get(&key).cloned()) {
            return Ok(hit);
        }
        let data = self.call(
            "azure_devops.states",
            Method::Get,
            &format!(
                "/{}/_apis/wit/workitemtypes/{}/states",
                encode_segment(project),
                encode_segment(work_item_type)
            ),
            Payload::None,
        )?;
        let states: Vec<WorkflowState> = data
            .get("value")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|s| {
                        Some(WorkflowState {
                            name: s.get("name")?.as_str()?.to_string(),
                            category: s.get("category")?.as_str()?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if let Ok(mut map) = self.states.lock() {
            map.insert(key, states.clone());
        }
        Ok(states)
    }

    /// A raw work item as a task, with its state category resolved.
    fn to_task(&self, item: &Value) -> Result<Option<Task>, TaskError> {
        let org = self.organization_url()?;
        let fields = item.get("fields");
        let field = |k: &str| {
            fields
                .and_then(|f| f.get(k))
                .and_then(Value::as_str)
                .unwrap_or("")
        };
        let (project, work_item_type, state) = (
            field("System.TeamProject"),
            field("System.WorkItemType"),
            field("System.State"),
        );
        let category = if project.is_empty() || work_item_type.is_empty() {
            None
        } else {
            self.workflow_states(project, work_item_type)?
                .into_iter()
                .find(|s| s.name == state)
                .map(|s| s.category)
        };
        Ok(parse_work_item(item, org, category.as_deref()))
    }

    /// Work items by id, in the order asked for.
    fn fetch_items(&self, ids: &[u64]) -> Result<Vec<Task>, TaskError> {
        let mut items = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(BATCH_SIZE) {
            let data = self.call(
                "azure_devops.items",
                Method::Post,
                "/_apis/wit/workitemsbatch",
                // `omit`: an item deleted since the query must not fail the rest.
                // `relations`: the parent link, which is where a task sits in
                // the breakdown. It cannot be combined with a `fields` list.
                Payload::Json(
                    json!({ "ids": chunk, "errorPolicy": "omit", "$expand": "relations" }),
                ),
            )?;
            if let Some(value) = data.get("value").and_then(Value::as_array) {
                items.extend(value.iter().filter(|v| !v.is_null()).cloned());
            }
        }

        let order: HashMap<u64, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        items.sort_by_key(|item| {
            item.get("id")
                .and_then(Value::as_u64)
                .and_then(|id| order.get(&id).copied())
                .unwrap_or(usize::MAX)
        });

        let total = items.len();
        let mut tasks = Vec::with_capacity(total);
        for item in &items {
            if let Some(task) = self.to_task(item)? {
                tasks.push(task);
            }
        }
        if tasks.len() != total {
            log::warn!(
                "[tasks] azure devops: skipped {} malformed work item(s)",
                total - tasks.len()
            );
        }
        Ok(tasks)
    }

    fn get_item(&self, id: u64, query: &str) -> Result<Value, TaskError> {
        self.call(
            "azure_devops.item",
            Method::Get,
            &format!("/_apis/wit/workitems/{id}?{query}"),
            Payload::None,
        )
    }

    /// One string field of a work item, fetched alone. Empty when absent.
    fn item_field(&self, id: u64, field: &str) -> Result<String, TaskError> {
        let item = self.get_item(id, &format!("fields={field}"))?;
        Ok(item
            .get("fields")
            .and_then(|f| f.get(field))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string())
    }

    /// Every process in the organization, with the projects that use it.
    fn processes(&self) -> Result<Vec<Value>, TaskError> {
        let data = self.call(
            "azure_devops.processes",
            Method::Get,
            "/_apis/work/processes?$expand=projects",
            Payload::None,
        )?;
        data.get("value")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| protocol("could not read the organization's processes"))
    }

    /// The system template behind a project, given its id or name.
    fn template_for_project(&self, project: &str) -> Result<ProcessTemplate, TaskError> {
        let processes = self.processes()?;
        let owner = processes
            .iter()
            .find(|p| {
                p.get("projects")
                    .and_then(Value::as_array)
                    .is_some_and(|projects| {
                        projects.iter().any(|pr| {
                            ["id", "name"].iter().any(|k| {
                                pr.get(*k)
                                    .and_then(Value::as_str)
                                    .is_some_and(|v| v.eq_ignore_ascii_case(project))
                            })
                        })
                    })
            })
            .ok_or_else(|| {
                protocol(format!(
                    "could not find the process project `{project}` uses"
                ))
            })?;
        resolve_template(owner, &processes).ok_or_else(|| {
            protocol(format!(
                "project `{project}` uses a process okena does not recognize"
            ))
        })
    }

    fn check_provider(id: &TaskId) -> Result<u64, TaskError> {
        if id.provider != PROVIDER_ID {
            return Err(protocol(format!(
                "task belongs to provider `{}`",
                id.provider
            )));
        }
        work_item_id(&id.external_id)
    }
}

impl TaskProvider for AzureDevOpsProvider {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn display_name(&self) -> &'static str {
        "Azure DevOps"
    }

    fn auth_status(&self) -> AuthStatus {
        match &self.credential {
            None => AuthStatus::Disconnected,
            Some(Credential::PersonalAccessToken { .. }) => AuthStatus::Connected {
                account: self.account(),
            },
            // Some other provider's kind of credential: unusable here, and the
            // fix is to paste a token, same as for a rejected one.
            Some(_) => AuthStatus::Expired,
        }
    }

    fn list_assigned(&self) -> Result<Vec<Task>, TaskError> {
        let data = self.call(
            "azure_devops.assigned",
            Method::Post,
            &format!("/_apis/wit/wiql?$top={QUEUE_LIMIT}"),
            Payload::Json(json!({ "query": QUERY_ASSIGNED })),
        )?;
        let ids: Vec<u64> = data
            .get("workItems")
            .and_then(Value::as_array)
            .ok_or_else(|| protocol("the query response had no `workItems`"))?
            .iter()
            .filter_map(|w| w.get("id").and_then(Value::as_u64))
            .collect();

        self.learn_account();

        Ok(self
            .fetch_items(&ids)?
            .into_iter()
            .filter(|t| !t.state.is_closed())
            .collect())
    }

    fn list_containers(&self) -> Result<Vec<TaskContainer>, TaskError> {
        // Read off the process list rather than `_apis/projects`, so the one
        // Work Items scope the token needs is enough to choose a project.
        let mut containers: Vec<TaskContainer> = Vec::new();
        for process in self.processes()? {
            let Some(projects) = process.get("projects").and_then(Value::as_array) else {
                continue;
            };
            for p in projects {
                let (Some(id), Some(name)) = (
                    p.get("id").and_then(Value::as_str),
                    p.get("name").and_then(Value::as_str),
                ) else {
                    continue;
                };
                if !containers.iter().any(|c| c.id == id) {
                    containers.push(TaskContainer {
                        id: id.to_string(),
                        name: name.to_string(),
                        key: String::new(),
                    });
                }
            }
        }
        containers.sort_by_key(|c| c.name.to_lowercase());
        Ok(containers)
    }

    fn create_task(&self, draft: &TaskDraft) -> Result<Task, TaskError> {
        if draft.title.trim().is_empty() {
            return Err(TaskError::NeedsChoice {
                message: "a task needs a title".into(),
            });
        }

        // A child goes in its parent's project: a hierarchy link across
        // projects is allowed, but nobody filing a sub-task means that.
        let parent = draft
            .parent_external_id
            .as_deref()
            .map(work_item_id)
            .transpose()?;
        let project = match parent {
            Some(parent) => {
                let item = self.get_item(parent, "fields=System.TeamProject")?;
                item.get("fields")
                    .and_then(|f| f.get("System.TeamProject"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| protocol("could not read the parent work item's project"))?
            }
            None => draft
                .container_id
                .clone()
                .ok_or_else(|| TaskError::NeedsChoice {
                    message: "choose a project for the new task".into(),
                })?,
        };

        let template = self.template_for_project(&project)?;
        let work_item_type = template.work_item_type(draft.kind);
        let org = self.organization_url()?;
        let created = self.call(
            "azure_devops.create",
            Method::Post,
            &format!(
                "/{}/_apis/wit/workitems/${}",
                encode_segment(&project),
                encode_segment(work_item_type)
            ),
            Payload::JsonPatch(create_operations(draft, work_item_type, parent, org)),
        )?;
        self.to_task(&created)?
            .ok_or_else(|| protocol("the new work item came back in a shape okena could not read"))
    }

    fn list_children(&self, id: &TaskId) -> Result<Vec<Task>, TaskError> {
        let parent = Self::check_provider(id)?;
        let item = self.get_item(parent, "$expand=relations")?;
        let ids = child_ids(&item);
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        self.fetch_items(&ids)
    }

    fn set_state(&self, id: &TaskId, state: TaskState) -> Result<(), TaskError> {
        let item_id = Self::check_provider(id)?;
        let item = self.get_item(
            item_id,
            "fields=System.TeamProject,System.WorkItemType,System.State",
        )?;
        let field = |k: &str| {
            item.get("fields")
                .and_then(|f| f.get(k))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let (project, work_item_type, current) = (
            field("System.TeamProject"),
            field("System.WorkItemType"),
            field("System.State"),
        );
        if project.is_empty() || work_item_type.is_empty() {
            return Err(protocol("could not read the work item's project and type"));
        }

        let states = self.workflow_states(&project, &work_item_type)?;
        let categories = target_categories(state);
        let chosen = choose_state(&states, categories).ok_or_else(|| {
            protocol(format!(
                "`{work_item_type}` has no state in the {} category",
                categories.join(" or ")
            ))
        })?;
        if chosen == current {
            return Ok(());
        }

        self.call(
            "azure_devops.set_state",
            Method::Patch,
            &format!("/_apis/wit/workitems/{item_id}"),
            Payload::JsonPatch(json!([
                { "op": "add", "path": "/fields/System.State", "value": chosen },
            ])),
        )?;
        Ok(())
    }

    fn get_task(&self, id: &TaskId) -> Result<Task, TaskError> {
        let item_id = Self::check_provider(id)?;
        let item = self.get_item(item_id, "$expand=fields")?;
        self.to_task(&item)?.ok_or_else(|| {
            protocol(format!(
                "work item {item_id} came back in a shape okena could not read"
            ))
        })
    }

    fn update_task(&self, id: &TaskId, patch: &TaskPatch) -> Result<Task, TaskError> {
        let item_id = Self::check_provider(id)?;
        // Which field holds the body depends on the type, so the type is only
        // worth asking for when the body is what changes.
        let work_item_type = if patch.description.is_some() {
            self.item_field(item_id, "System.WorkItemType")?
        } else {
            String::new()
        };
        let updated = self.call(
            "azure_devops.update",
            Method::Patch,
            &format!("/_apis/wit/workitems/{item_id}"),
            Payload::JsonPatch(update_operations(patch, &work_item_type)),
        )?;
        self.to_task(&updated)?.ok_or_else(|| {
            protocol("the edited work item came back in a shape okena could not read")
        })
    }

    fn add_comment(&self, id: &TaskId, body: &str) -> Result<(), TaskError> {
        let item_id = Self::check_provider(id)?;
        // Comments are addressed under the project, which the id does not say.
        let project = self.item_field(item_id, "System.TeamProject")?;
        if project.is_empty() {
            return Err(protocol("could not read the work item's project"));
        }
        self.call(
            "azure_devops.comment",
            Method::Post,
            &format!(
                "/{}/_apis/wit/workItems/{item_id}/comments?api-version=7.1-preview.4",
                encode_segment(&project)
            ),
            Payload::Json(json!({ "text": html::from_text(body) })),
        )?;
        Ok(())
    }

    fn get_tasks(&self, ids: &[TaskId]) -> Result<Vec<Task>, TaskError> {
        let ids = ids
            .iter()
            .map(Self::check_provider)
            .collect::<Result<Vec<u64>, _>>()?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // `workitemsbatch`, which already leaves out an item deleted since.
        self.fetch_items(&ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGILE: &str = "adcc42ab-9882-485e-a3ed-7678f01f66bc";
    const SCRUM: &str = "6b724908-ef14-45cf-84f8-768b5384da45";
    const NO_PARENT: &str = "00000000-0000-0000-0000-000000000000";

    #[test]
    fn organization_urls_normalize_to_their_root() {
        for (raw, want) in [
            (
                "https://dev.azure.com/contoso",
                "https://dev.azure.com/contoso",
            ),
            ("dev.azure.com/contoso/", "https://dev.azure.com/contoso"),
            (
                "https://dev.azure.com/contoso/Web%20Shop/_workitems?x=1",
                "https://dev.azure.com/contoso",
            ),
            (
                "https://contoso@dev.azure.com/contoso/Shop/_git/repo",
                "https://dev.azure.com/contoso",
            ),
            (
                "  HTTPS://Contoso.VisualStudio.com/DefaultCollection ",
                "https://contoso.visualstudio.com",
            ),
        ] {
            assert_eq!(
                normalize_organization_url(raw).as_deref(),
                Ok(want),
                "{raw}"
            );
        }
    }

    #[test]
    fn anything_but_azure_devops_services_is_refused() {
        for raw in [
            "",
            "https://dev.azure.com/",
            "http://dev.azure.com/contoso",
            "https://tfs.contoso.local/tfs/DefaultCollection",
            "https://linear.app/qblok",
            "https://.visualstudio.com",
        ] {
            assert!(
                normalize_organization_url(raw).is_err(),
                "{raw} should be refused"
            );
        }
    }

    #[test]
    fn kinds_map_to_each_templates_native_type() {
        use ProcessTemplate::*;
        let table = [
            (Agile, ["Epic", "Feature", "User Story", "Task", "Bug"]),
            (
                Scrum,
                ["Epic", "Feature", "Product Backlog Item", "Task", "Bug"],
            ),
            (Cmmi, ["Epic", "Feature", "Requirement", "Task", "Bug"]),
            (Basic, ["Epic", "Epic", "Issue", "Task", "Issue"]),
        ];
        let kinds = [
            TaskKind::Epic,
            TaskKind::Feature,
            TaskKind::Story,
            TaskKind::Task,
            TaskKind::Defect,
        ];
        for (template, types) in table {
            for (kind, want) in kinds.iter().zip(types) {
                assert_eq!(
                    template.work_item_type(*kind),
                    want,
                    "{template:?} {kind:?}"
                );
            }
        }
    }

    #[test]
    fn native_types_read_back_as_kinds() {
        assert_eq!(kind_from_type("User Story"), TaskKind::Story);
        assert_eq!(kind_from_type("Product Backlog Item"), TaskKind::Story);
        assert_eq!(kind_from_type("Requirement"), TaskKind::Story);
        // Basic: an Issue stands in for a story, an Epic is an epic.
        assert_eq!(kind_from_type("Issue"), TaskKind::Story);
        assert_eq!(kind_from_type("Epic"), TaskKind::Epic);
        assert_eq!(kind_from_type("Bug"), TaskKind::Defect);
        assert_eq!(kind_from_type("Impediment"), TaskKind::Task);
    }

    #[test]
    fn an_inherited_process_resolves_to_its_parent_template() {
        let all = vec![
            json!({ "typeId": "c0ffee00-0000-0000-0000-000000000001", "name": "Shop Scrum",
                    "customizationType": "inherited", "parentProcessTypeId": SCRUM }),
            json!({ "typeId": SCRUM, "name": "Scrum", "customizationType": "system",
                    "parentProcessTypeId": NO_PARENT }),
        ];
        assert_eq!(
            resolve_template(&all[0], &all),
            Some(ProcessTemplate::Scrum)
        );
    }

    #[test]
    fn a_system_process_is_recognized_by_name_when_its_id_is_new() {
        let basic = json!({ "typeId": "1111", "name": "Basic", "customizationType": "system",
                            "parentProcessTypeId": NO_PARENT });
        assert_eq!(resolve_template(&basic, &[]), Some(ProcessTemplate::Basic));
        // An inherited process named like a template is not trusted by name.
        let impostor = json!({ "typeId": "2222", "name": "Agile", "customizationType": "inherited",
                               "parentProcessTypeId": NO_PARENT });
        assert_eq!(resolve_template(&impostor, &[]), None);
    }

    #[test]
    fn categories_map_onto_normalized_states() {
        assert_eq!(map_category("Proposed"), TaskState::Todo);
        assert_eq!(map_category("InProgress"), TaskState::InProgress);
        assert_eq!(map_category("Resolved"), TaskState::InReview);
        assert_eq!(map_category("Completed"), TaskState::Done);
        assert_eq!(map_category("Removed"), TaskState::Canceled);
        assert_eq!(map_category("Whatever"), TaskState::Unknown);
    }

    fn states(pairs: &[(&str, &str)]) -> Vec<WorkflowState> {
        pairs
            .iter()
            .map(|(name, category)| WorkflowState {
                name: name.to_string(),
                category: category.to_string(),
            })
            .collect()
    }

    #[test]
    fn the_first_state_in_a_category_is_the_one_set() {
        let agile_story = states(&[
            ("New", "Proposed"),
            ("Active", "InProgress"),
            ("Testing", "InProgress"),
            ("Resolved", "Resolved"),
            ("Closed", "Completed"),
            ("Removed", "Removed"),
        ]);
        assert_eq!(
            choose_state(&agile_story, target_categories(TaskState::InProgress)),
            Some("Active")
        );
        assert_eq!(
            choose_state(&agile_story, target_categories(TaskState::InReview)),
            Some("Resolved")
        );
        assert_eq!(
            choose_state(&agile_story, target_categories(TaskState::Done)),
            Some("Closed")
        );
    }

    #[test]
    fn review_falls_back_to_in_progress_where_a_type_has_no_resolved_state() {
        let scrum_pbi = states(&[
            ("New", "Proposed"),
            ("Approved", "Proposed"),
            ("Committed", "InProgress"),
            ("Done", "Completed"),
        ]);
        assert_eq!(
            choose_state(&scrum_pbi, target_categories(TaskState::InReview)),
            Some("Committed")
        );
        assert_eq!(
            choose_state(&scrum_pbi, target_categories(TaskState::Todo)),
            Some("New")
        );
        assert_eq!(
            choose_state(&scrum_pbi, target_categories(TaskState::Canceled)),
            None
        );
    }

    fn item(id: u64, fields: Value) -> Value {
        json!({ "id": id, "fields": fields })
    }

    #[test]
    fn parses_a_work_item() {
        let t = parse_work_item(
            &item(
                42,
                json!({
                    "System.TeamProject": "Web Shop",
                    "System.WorkItemType": "User Story",
                    "System.Title": "Checkout flow",
                    "System.State": "Resolved",
                    "System.Description": "<div>Pay <b>now</b></div>",
                    "System.Tags": "frontend; p1",
                    "System.ChangedDate": "2026-09-01T10:00:00Z",
                    "System.Parent": 7,
                    "System.AreaPath": "Web Shop\\Payments",
                    "System.IterationPath": "Web Shop\\Sprint 3",
                }),
            ),
            "https://dev.azure.com/contoso",
            Some("Resolved"),
        )
        .expect("parses");
        assert_eq!(t.id, TaskId::new("azure_devops", "42"));
        assert_eq!(t.display_key, "#42");
        assert_eq!(t.state, TaskState::InReview);
        assert_eq!(t.state_name, "Resolved");
        assert_eq!(t.kind, TaskKind::Story);
        assert_eq!(t.labels, ["frontend", "p1"]);
        assert_eq!(t.description.as_deref(), Some("Pay **now**"));
        assert_eq!(t.parent_id.as_deref(), Some("7"));
        assert_eq!(t.parent_key.as_deref(), Some("#7"));
        assert_eq!(
            t.url,
            "https://dev.azure.com/contoso/Web%20Shop/_workitems/edit/42"
        );
        let names: Vec<_> = t
            .groups
            .iter()
            .map(|g| (g.axis.clone(), g.name.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                (GroupAxis::Team, "Web Shop"),
                (GroupAxis::Project, "Payments"),
                (GroupAxis::Iteration, "Sprint 3"),
            ]
        );
    }

    #[test]
    fn a_bugs_body_is_its_repro_steps() {
        let t = parse_work_item(
            &item(
                3,
                json!({
                    "System.WorkItemType": "Bug",
                    "Microsoft.VSTS.TCM.ReproSteps": "<p>Click pay</p>",
                    "System.Description": "",
                }),
            ),
            "https://dev.azure.com/contoso",
            None,
        )
        .expect("parses");
        assert_eq!(t.kind, TaskKind::Defect);
        assert_eq!(t.description.as_deref(), Some("Click pay"));
        assert_eq!(t.state, TaskState::Unknown);
    }

    #[test]
    fn root_area_and_iteration_are_not_filter_rows() {
        let groups = parse_groups(&json!({
            "System.TeamProject": "Shop",
            "System.AreaPath": "Shop",
            "System.IterationPath": "Shop",
        }));
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].axis, GroupAxis::Team);
    }

    #[test]
    fn the_branch_is_slugged_from_id_and_title() {
        let p = AzureDevOpsProvider::new(None);
        let t = parse_work_item(
            &item(
                1234,
                json!({ "System.Title": "Fix: checkout / totals", "System.WorkItemType": "Bug" }),
            ),
            "https://dev.azure.com/contoso",
            None,
        )
        .expect("parses");
        assert_eq!(t.branch_name, "fix/1234-fix-checkout-totals");
        assert_eq!(p.branch_name(&t), "fix/1234-fix-checkout-totals");
    }

    #[test]
    fn children_come_from_forward_hierarchy_links_only() {
        let item = json!({ "relations": [
            { "rel": CHILD_LINK, "url": "https://dev.azure.com/c/_apis/wit/workItems/11" },
            { "rel": PARENT_LINK, "url": "https://dev.azure.com/c/_apis/wit/workItems/1" },
            { "rel": "System.LinkTypes.Related", "url": "https://dev.azure.com/c/_apis/wit/workItems/5" },
            { "rel": CHILD_LINK, "url": "https://dev.azure.com/c/_apis/wit/workItems/12" },
        ]});
        assert_eq!(child_ids(&item), [11, 12]);
    }

    #[test]
    fn a_pat_is_sent_as_basic_with_an_empty_user() {
        assert_eq!(basic_auth("pat"), "Basic OnBhdA==");
    }

    #[test]
    fn unauthenticated_provider_reports_disconnected() {
        let p = AzureDevOpsProvider::new(None);
        assert_eq!(p.auth_status(), AuthStatus::Disconnected);
        assert!(matches!(
            p.list_assigned(),
            Err(TaskError::NotAuthenticated { .. })
        ));
    }

    #[test]
    fn a_linear_key_is_not_an_azure_devops_credential() {
        let p = AzureDevOpsProvider::new(Some(Credential::ApiKey("lin_api".into())));
        assert_eq!(p.auth_status(), AuthStatus::Expired);
    }

    #[test]
    fn rejected_tokens_read_as_unauthorized() {
        for status in [401, 203] {
            let r = read_response(HttpResponse::new(status, vec![], b"<html>".to_vec()));
            assert!(matches!(r, Err(TaskError::Unauthorized { .. })), "{status}");
        }
        // A missing permission is not a dead token.
        let r = read_response(HttpResponse::new(
            403,
            vec![],
            br#"{"message":"TF237111: no permission"}"#.to_vec(),
        ));
        match r {
            Err(TaskError::Protocol { message, .. }) => assert!(message.contains("TF237111")),
            other => panic!("expected a protocol error, got {other:?}"),
        }
    }

    #[test]
    fn foreign_and_non_numeric_ids_are_refused_before_any_call() {
        let p = AzureDevOpsProvider::new(Some(pat()));
        let foreign = TaskId::new("linear", "7");
        assert!(matches!(
            p.set_state(&foreign, TaskState::Done),
            Err(TaskError::Protocol { .. })
        ));
        let bad = TaskId::new(PROVIDER_ID, "uuid-1");
        assert!(matches!(
            p.list_children(&bad),
            Err(TaskError::Protocol { .. })
        ));
    }

    // ── Against a mocked API ───────────────────────────────────────────────

    use crate::providers::NET;

    fn pat() -> Credential {
        Credential::PersonalAccessToken {
            token: "pat".into(),
            organization_url: "https://dev.azure.com/contoso".into(),
            account: Some("Nima".into()),
        }
    }

    fn ok(v: Value) -> Result<HttpResponse, HttpError> {
        Ok(HttpResponse::new(200, vec![], v.to_string().into_bytes()))
    }

    fn story_states() -> Value {
        json!({ "value": [
            { "name": "New", "category": "Proposed" },
            { "name": "Active", "category": "InProgress" },
            { "name": "Resolved", "category": "Resolved" },
            { "name": "Closed", "category": "Completed" },
        ]})
    }

    #[test]
    fn the_assigned_queue_keeps_query_order_and_drops_closed_categories() {
        let _net = NET.lock().unwrap_or_else(|e| e.into_inner());
        let _mock = http::testing::mock(|req| {
            assert_eq!(req.header_value("Authorization"), Some("Basic OnBhdA=="));
            let url = req.url();
            if url.contains("/_apis/wit/wiql") {
                assert!(url.contains("api-version=7.1"), "{url}");
                return ok(json!({ "workItems": [{ "id": 2 }, { "id": 1 }, { "id": 3 }] }));
            }
            if url.contains("/_apis/wit/workitemsbatch") {
                let item = |id: u64, state: &str, tags: &str| {
                    json!({ "id": id, "fields": {
                        "System.TeamProject": "Shop", "System.WorkItemType": "User Story",
                        "System.State": state, "System.Title": format!("t{id}"),
                        "System.Tags": tags,
                    }})
                };
                // Returned out of order, with a custom-closed item among them.
                return ok(json!({ "value": [
                    item(1, "Active", ""), item(3, "Closed", ""), item(2, "Resolved", "api"),
                ]}));
            }
            if url.contains("/Shop/_apis/wit/workitemtypes/User%20Story/states") {
                return ok(story_states());
            }
            panic!("unexpected request {url}");
        });

        let tasks = AzureDevOpsProvider::new(Some(pat()))
            .list_assigned()
            .expect("lists");
        let keys: Vec<_> = tasks.iter().map(|t| t.display_key.as_str()).collect();
        assert_eq!(keys, ["#2", "#1"]);
        assert_eq!(tasks[0].state, TaskState::InReview);
        assert_eq!(tasks[0].labels, ["api"]);
        assert_eq!(tasks[1].state, TaskState::InProgress);
    }

    #[test]
    fn a_listed_task_knows_its_parent_from_its_hierarchy_link() {
        // The batch documents no default field set, so `System.Parent` may not
        // be among the fields; the hierarchy link is what says where it sits.
        let _net = NET.lock().unwrap_or_else(|e| e.into_inner());
        let _mock = http::testing::mock(|req| {
            let url = req.url();
            if url.contains("/_apis/wit/workitemsbatch") {
                let body = req.json_body().cloned().unwrap_or_default();
                let relations = body["$expand"] == "relations";
                let item = |id: u64, parent: Option<u64>| {
                    let mut v = json!({ "id": id, "fields": {
                        "System.TeamProject": "Shop", "System.WorkItemType": "User Story",
                        "System.State": "Active", "System.Title": format!("t{id}"),
                    }});
                    if relations {
                        v["relations"] = json!(parent.map(|p| vec![json!({
                            "rel": PARENT_LINK,
                            "url": format!("https://dev.azure.com/contoso/_apis/wit/workItems/{p}"),
                        })]).unwrap_or_default());
                    }
                    v
                };
                return ok(json!({ "value": [item(1, Some(2)), item(2, Some(3)), item(3, None)] }));
            }
            if url.contains("/states") {
                return ok(story_states());
            }
            panic!("unexpected request {url}");
        });

        // Through `get_tasks` rather than `list_assigned`: both read the batch
        // the same way, and the queue's rate floor would refuse a second
        // test's poll in the same process.
        let ids: Vec<TaskId> = ["1", "2", "3"]
            .iter()
            .map(|id| TaskId::new(PROVIDER_ID, *id))
            .collect();
        let tasks = AzureDevOpsProvider::new(Some(pat()))
            .get_tasks(&ids)
            .expect("reads");
        let parents: Vec<_> = tasks
            .iter()
            .map(|t| {
                (
                    t.id.external_id.as_str(),
                    t.parent_id.as_deref(),
                    t.parent_key.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            parents,
            [
                ("1", Some("2"), Some("#2")),
                ("2", Some("3"), Some("#3")),
                ("3", None, None),
            ]
        );
    }

    #[test]
    fn tasks_by_id_include_closed_ones_and_other_peoples() {
        // What an ancestor is: often finished, often somebody else's. The queue
        // leaves both out; reading by id must not.
        let _net = NET.lock().unwrap_or_else(|e| e.into_inner());
        let _mock = http::testing::mock(|req| {
            let url = req.url();
            if url.contains("/_apis/wit/workitemsbatch") {
                let item = |id: u64, state: &str| {
                    json!({ "id": id, "fields": {
                        "System.TeamProject": "Shop", "System.WorkItemType": "User Story",
                        "System.State": state, "System.Title": format!("t{id}"),
                        "System.AssignedTo": { "displayName": "Someone Else" },
                    }})
                };
                return ok(json!({ "value": [item(10, "Closed"), item(11, "Active")] }));
            }
            if url.contains("/states") {
                return ok(story_states());
            }
            panic!("unexpected request {url}");
        });

        let ids = [
            TaskId::new(PROVIDER_ID, "10"),
            TaskId::new(PROVIDER_ID, "11"),
        ];
        let tasks = AzureDevOpsProvider::new(Some(pat()))
            .get_tasks(&ids)
            .expect("reads");
        let keys: Vec<_> = tasks.iter().map(|t| t.display_key.as_str()).collect();
        assert_eq!(keys, ["#10", "#11"]);
        assert!(tasks[0].state.is_closed(), "{:?}", tasks[0].state);
    }

    #[test]
    fn the_parent_field_and_the_hierarchy_link_both_name_a_parent() {
        let fields = json!({ "System.WorkItemType": "Task", "System.Title": "t" });
        let linked = json!({ "id": 5, "fields": fields, "relations": [
            { "rel": "System.LinkTypes.Related", "url": "https://dev.azure.com/c/_apis/wit/workItems/8" },
            { "rel": PARENT_LINK, "url": "https://dev.azure.com/c/_apis/wit/workItems/9" },
        ]});
        let t = parse_work_item(&linked, "https://dev.azure.com/c", None).expect("parses");
        assert_eq!(t.parent_id.as_deref(), Some("9"));

        let orphan = json!({ "id": 5, "fields": fields, "relations": [
            { "rel": CHILD_LINK, "url": "https://dev.azure.com/c/_apis/wit/workItems/8" },
        ]});
        let t = parse_work_item(&orphan, "https://dev.azure.com/c", None).expect("parses");
        assert_eq!(t.parent_id, None, "a child link is not a parent");
    }

    #[test]
    fn moving_to_review_patches_the_resolved_state() {
        let _net = NET.lock().unwrap_or_else(|e| e.into_inner());
        let sent = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let log = sent.clone();
        let _mock = http::testing::mock(move |req| {
            let url = req.url();
            if url.contains("/_apis/wit/workitems/7?fields=") {
                return ok(json!({ "id": 7, "fields": {
                    "System.TeamProject": "Shop", "System.WorkItemType": "User Story",
                    "System.State": "Active",
                }}));
            }
            if url.contains("/states") {
                return ok(story_states());
            }
            if req.method() == Method::Patch {
                let (content_type, body) = req.raw_body().expect("a JSON Patch body");
                assert_eq!(content_type, JSON_PATCH);
                if let Ok(mut log) = log.lock() {
                    log.push(format!("{url} {body}"));
                }
                return ok(json!({ "id": 7 }));
            }
            panic!("unexpected request {url}");
        });

        AzureDevOpsProvider::new(Some(pat()))
            .set_state(&TaskId::new(PROVIDER_ID, "7"), TaskState::InReview)
            .expect("moves");
        let sent = sent.lock().map(|s| s.clone()).unwrap_or_default();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert!(
            sent[0].contains("/_apis/wit/workitems/7?api-version=7.1"),
            "{}",
            sent[0]
        );
        assert!(sent[0].contains(r#""value":"Resolved""#), "{}", sent[0]);
    }

    #[test]
    fn a_story_in_an_inherited_scrum_project_is_a_linked_product_backlog_item() {
        let _net = NET.lock().unwrap_or_else(|e| e.into_inner());
        let sent = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let log = sent.clone();
        let _mock = http::testing::mock(move |req| {
            let url = req.url();
            if url.contains("/_apis/wit/workitems/10?fields=System.TeamProject") {
                return ok(json!({ "id": 10, "fields": { "System.TeamProject": "Shop" } }));
            }
            if url.contains("/_apis/work/processes") {
                return ok(json!({ "value": [
                    { "typeId": AGILE, "name": "Agile", "customizationType": "system",
                      "parentProcessTypeId": NO_PARENT, "projects": [{ "id": "p0", "name": "Other" }] },
                    { "typeId": "c0ffee00-0000-0000-0000-000000000001", "name": "Shop Scrum",
                      "customizationType": "inherited", "parentProcessTypeId": SCRUM,
                      "projects": [{ "id": "p1", "name": "Shop" }] },
                ]}));
            }
            if url.contains("/Shop/_apis/wit/workitems/$Product%20Backlog%20Item") {
                let (_, body) = req.raw_body().expect("a JSON Patch body");
                if let Ok(mut log) = log.lock() {
                    log.push(body.to_string());
                }
                return ok(json!({ "id": 11, "fields": {
                    "System.TeamProject": "Shop", "System.WorkItemType": "Product Backlog Item",
                    "System.State": "New", "System.Title": "Split payments", "System.Parent": 10,
                }}));
            }
            if url.contains("/Shop/_apis/wit/workitemtypes/Product%20Backlog%20Item/states") {
                return ok(json!({ "value": [{ "name": "New", "category": "Proposed" }] }));
            }
            panic!("unexpected request {url}");
        });

        let task = AzureDevOpsProvider::new(Some(pat()))
            .create_task(&TaskDraft {
                title: "Split payments".into(),
                description: Some("Two cards".into()),
                kind: TaskKind::Story,
                parent_external_id: Some("10".into()),
                container_id: None,
            })
            .expect("creates");
        assert_eq!(task.display_key, "#11");
        assert_eq!(task.kind, TaskKind::Story);
        assert_eq!(task.state, TaskState::Todo);
        assert_eq!(task.parent_key.as_deref(), Some("#10"));

        let body = sent.lock().map(|s| s.join("")).unwrap_or_default();
        assert!(body.contains(PARENT_LINK), "{body}");
        assert!(
            body.contains("https://dev.azure.com/contoso/_apis/wit/workItems/10"),
            "{body}"
        );
        assert!(body.contains("<div>Two cards</div>"), "{body}");
    }

    #[test]
    fn a_display_key_reads_its_work_item() {
        let _net = NET.lock().unwrap_or_else(|e| e.into_inner());
        let _mock = http::testing::mock(|req| {
            let url = req.url();
            if url.contains("/_apis/wit/workitems/7?") && url.contains("expand=fields") {
                return ok(json!({ "id": 7, "fields": {
                    "System.TeamProject": "Shop", "System.WorkItemType": "User Story",
                    "System.State": "Active", "System.Title": "Checkout",
                    "System.Description": "<div>Pay</div>",
                }}));
            }
            if url.contains("/states") {
                return ok(story_states());
            }
            panic!("unexpected request {url}");
        });

        let task = AzureDevOpsProvider::new(Some(pat()))
            .get_task(&TaskId::new(PROVIDER_ID, "#7"))
            .expect("reads");
        assert_eq!(task.display_key, "#7");
        assert_eq!(task.state, TaskState::InProgress);
        assert_eq!(task.description.as_deref(), Some("Pay"));
    }

    #[test]
    fn editing_a_bugs_description_patches_its_repro_steps() {
        let _net = NET.lock().unwrap_or_else(|e| e.into_inner());
        let sent = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let log = sent.clone();
        let _mock = http::testing::mock(move |req| {
            let url = req.url();
            if url.contains("/_apis/wit/workitems/3?fields=System.WorkItemType") {
                return ok(json!({ "id": 3, "fields": { "System.WorkItemType": "Bug" } }));
            }
            if req.method() == Method::Patch {
                let (_, body) = req.raw_body().expect("a JSON Patch body");
                if let Ok(mut log) = log.lock() {
                    log.push(body.to_string());
                }
                return ok(json!({ "id": 3, "fields": {
                    "System.TeamProject": "Shop", "System.WorkItemType": "Bug",
                    "System.State": "New", "System.Title": "Totals wrong",
                    "Microsoft.VSTS.TCM.ReproSteps": "<div>Add two items</div>",
                }}));
            }
            if url.contains("/states") {
                return ok(json!({ "value": [{ "name": "New", "category": "Proposed" }] }));
            }
            panic!("unexpected request {url}");
        });

        let task = AzureDevOpsProvider::new(Some(pat()))
            .update_task(
                &TaskId::new(PROVIDER_ID, "3"),
                &TaskPatch {
                    title: None,
                    description: Some("Add two items".into()),
                },
            )
            .expect("edits");
        assert_eq!(task.description.as_deref(), Some("Add two items"));
        let body = sent.lock().map(|s| s.join("")).unwrap_or_default();
        assert!(
            body.contains("/fields/Microsoft.VSTS.TCM.ReproSteps"),
            "{body}"
        );
        assert!(!body.contains("System.Title"), "{body}");
    }

    #[test]
    fn a_comment_is_posted_under_the_items_project() {
        let _net = NET.lock().unwrap_or_else(|e| e.into_inner());
        let posted = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let log = posted.clone();
        let _mock = http::testing::mock(move |req| {
            let url = req.url();
            if url.contains("/_apis/wit/workitems/5?fields=System.TeamProject") {
                return ok(json!({ "id": 5, "fields": { "System.TeamProject": "Web Shop" } }));
            }
            if req.method() == Method::Post && url.contains("/comments") {
                if let Ok(mut log) = log.lock() {
                    log.push(url.to_string());
                }
                return ok(json!({ "id": 1 }));
            }
            panic!("unexpected request {url}");
        });

        AzureDevOpsProvider::new(Some(pat()))
            .add_comment(&TaskId::new(PROVIDER_ID, "#5"), "Picked up")
            .expect("comments");
        let urls = posted.lock().map(|u| u.clone()).unwrap_or_default();
        assert_eq!(urls.len(), 1, "{urls:?}");
        assert!(
            urls[0]
                .contains("/Web%20Shop/_apis/wit/workItems/5/comments?api-version=7.1-preview.4"),
            "{}",
            urls[0]
        );
    }
}
