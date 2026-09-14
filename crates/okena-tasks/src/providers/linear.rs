//! Linear provider — GraphQL over `okena_transport::http`.
//!
//! Why GraphQL rather than Linear's MCP server: the harness needs typed rows it
//! can sort, diff against the previous poll, and render in a table. MCP is an
//! agent-facing protocol whose tool output is shaped for a model to read, with
//! no stable schema across server versions. The MCP server still has a role —
//! the *agent* spawned in a task's worktree talks to it — but it is handed the
//! same OAuth token this provider holds rather than being the harness's own
//! data path.

use crate::provider::{
    AuthStatus, Credential, TaskContainer, TaskDraft, TaskError, TaskPatch, TaskProvider,
    task_branch_name,
};
use okena_core::tasks::{GroupAxis, Task, TaskGroup, TaskId, TaskKind, TaskState};
use okena_transport::http::{self, HttpError, HttpRequest};
use std::time::Duration;

const PROVIDER_ID: &str = "linear";
const API_URL: &str = "https://api.linear.app/graphql";
/// Client-side rate floor. Well below the real poll cadence — it exists only to
/// catch a runaway caller, never a legitimate refresh.
const MIN_INTERVAL: Duration = Duration::from_secs(5);
const TIMEOUT: Duration = Duration::from_secs(20);
/// Linear caps page size at 250; the assigned-work queue is far smaller.
const PAGE_SIZE: u32 = 100;

/// The issue fields `parse_issue` reads, as a fragment every query that
/// returns issues spreads, so the queries cannot drift from the parser or
/// from each other.
macro_rules! issue_fields {
    () => {
        r#"
fragment IssueFields on Issue {
  id
  identifier
  title
  description
  url
  updatedAt
  state { name type }
  parent { id identifier }
  labels(first: 20) { nodes { name } }
  team { id key name }
  project { id name }
  cycle { id number name }
}
"#
    };
}

/// Assigned, still-open issues, most recently updated first.
///
/// The `state.type` filter runs server-side so a large finished backlog is
/// never transferred. Linear's state types are a closed set:
/// `triage | backlog | unstarted | started | completed | canceled`.
const QUERY_ASSIGNED: &str = concat!(
    r#"
query AssignedIssues($first: Int!) {
  viewer {
    id
    name
    assignedIssues(
      first: $first
      filter: { state: { type: { nin: ["completed", "canceled"] } } }
      orderBy: updatedAt
    ) {
      nodes { ...IssueFields }
    }
  }
}
"#,
    issue_fields!()
);

/// Teams the authenticated user belongs to, for choosing where a new issue
/// goes. Linear scopes issues to a team and requires one on create.
const QUERY_TEAMS: &str = r#"
query MyTeams {
  viewer {
    teams(first: 50) {
      nodes { id name key }
    }
  }
}
"#;

/// The team a parent issue lives in, plus that team's labels.
///
/// One round-trip because a sub-task needs both: the team it inherits, and the
/// label carrying its kind, which is per-team.
const QUERY_PARENT_CONTEXT: &str = r#"
query ParentContext($id: String!) {
  issue(id: $id) {
    id
    team {
      id
      labels(first: 100) { nodes { id name } }
    }
  }
}
"#;

/// A team's labels, for resolving the one that carries a kind.
const QUERY_TEAM_LABELS: &str = r#"
query TeamLabels($id: String!) {
  team(id: $id) {
    id
    labels(first: 100) { nodes { id name } }
  }
}
"#;

/// Create the label carrying a kind, when the team has none matching.
const MUTATION_CREATE_LABEL: &str = r#"
mutation CreateLabel($teamId: String!, $name: String!) {
  issueLabelCreate(input: { teamId: $teamId, name: $name }) {
    success
    issueLabel { id name }
  }
}
"#;

/// Create an issue, returning it in the same shape the list query uses so the
/// caller gets a real `Task` without a second fetch.
const MUTATION_CREATE_ISSUE: &str = concat!(
    r#"
mutation CreateIssue(
  $teamId: String!
  $title: String!
  $description: String
  $parentId: String
  $labelIds: [String!]
) {
  issueCreate(
    input: {
      teamId: $teamId
      title: $title
      description: $description
      parentId: $parentId
      labelIds: $labelIds
    }
  ) {
    success
    issue { ...IssueFields }
  }
}
"#,
    issue_fields!()
);

/// Sub-issues of a parent, whoever they are assigned to.
const QUERY_CHILDREN: &str = concat!(
    r#"
query IssueChildren($id: String!) {
  issue(id: $id) {
    children(first: 100) {
      nodes { ...IssueFields }
    }
  }
}
"#,
    issue_fields!()
);

/// The workflow states available to the issue's own team.
///
/// A state cannot be set by category — `issueUpdate` takes a concrete state id,
/// and those are per-team — so a state change is always resolve-then-mutate.
const QUERY_ISSUE_STATES: &str = r#"
query IssueStates($id: String!) {
  issue(id: $id) {
    id
    team {
      states(first: 100) {
        nodes { id name type position }
      }
    }
  }
}
"#;

const MUTATION_SET_STATE: &str = r#"
mutation SetState($id: String!, $stateId: String!) {
  issueUpdate(id: $id, input: { stateId: $stateId }) {
    success
  }
}
"#;

/// One issue, by UUID or identifier — `issue(id:)` takes either.
const QUERY_ISSUE: &str = concat!(
    r#"
query Issue($id: String!) {
  issue(id: $id) { ...IssueFields }
}
"#,
    issue_fields!()
);

/// The UUID behind an identifier. Mutations are handed the UUID: an
/// identifier changes when its issue moves team, the UUID never does.
const QUERY_ISSUE_ID: &str = r#"
query IssueId($id: String!) {
  issue(id: $id) { id }
}
"#;

/// Edit an issue. `$input` carries only the fields being changed — an explicit
/// `null` would clear one.
const MUTATION_UPDATE_ISSUE: &str = concat!(
    r#"
mutation UpdateIssue($id: String!, $input: IssueUpdateInput!) {
  issueUpdate(id: $id, input: $input) {
    success
    issue { ...IssueFields }
  }
}
"#,
    issue_fields!()
);

/// Several issues by UUID, in one request.
const QUERY_ISSUES_BY_ID: &str = concat!(
    r#"
query IssuesById($ids: [ID!]!, $first: Int!) {
  issues(filter: { id: { in: $ids } }, first: $first) {
    nodes { ...IssueFields }
  }
}
"#,
    issue_fields!()
);

const MUTATION_CREATE_COMMENT: &str = r#"
mutation CreateComment($issueId: String!, $body: String!) {
  commentCreate(input: { issueId: $issueId, body: $body }) {
    success
  }
}
"#;

pub struct LinearProvider {
    credential: Option<Credential>,
    /// Cached display name of the authenticated account, filled by the first
    /// successful `list_assigned` (the same query returns `viewer`).
    account: std::sync::RwLock<Option<String>>,
}

impl LinearProvider {
    pub fn new(credential: Option<Credential>) -> Self {
        Self {
            credential,
            account: std::sync::RwLock::new(None),
        }
    }

    fn credential(&self) -> Result<&Credential, TaskError> {
        self.credential.as_ref().ok_or(TaskError::NotAuthenticated {
            provider: PROVIDER_ID,
        })
    }

    /// Apply Linear's auth header.
    ///
    /// Linear takes a personal API key *raw* in `Authorization` but an OAuth
    /// access token as `Bearer <token>`. Sending a personal key with a `Bearer`
    /// prefix is rejected, which is the whole reason [`Credential`] keeps the
    /// two kinds apart rather than collapsing them into one string.
    fn authorize(req: HttpRequest, cred: &Credential) -> HttpRequest {
        match cred {
            Credential::ApiKey(key) => req.header("Authorization", key.clone()),
            Credential::OAuth { access_token, .. } => req.bearer(access_token),
            // Not a Linear credential; sent as a key so Linear rejects it and
            // the UI asks for a new one.
            Credential::PersonalAccessToken { token, .. } => {
                req.header("Authorization", token.clone())
            }
        }
    }

    /// Issue a GraphQL request and return the `data` object.
    /// The label ids that record a task's kind on Linear.
    ///
    /// Linear has no native kind, so okena carries it as a label — the same
    /// signal `TaskKind::from_labels` reads back. An existing team label wins;
    /// only when none of the kind's aliases match does okena create one, so a
    /// team that already says "Bug" does not end up with "Defect" beside it.
    ///
    /// `Task` is the absence of a marker, so it gets no label at all.
    fn kind_label_ids(
        &self,
        team_id: &str,
        kind: okena_core::tasks::TaskKind,
        labels: &std::collections::HashMap<String, String>,
    ) -> Result<Vec<String>, TaskError> {
        use okena_core::tasks::TaskKind;
        let aliases: &[&str] = match kind {
            TaskKind::Epic => &["epic", "initiative"],
            TaskKind::Feature => &["feature"],
            TaskKind::Story => &["story", "user story"],
            TaskKind::Defect => &["bug", "defect", "fix", "hotfix"],
            TaskKind::Task => return Ok(Vec::new()),
        };
        if let Some(id) = aliases.iter().find_map(|a| labels.get(*a)) {
            return Ok(vec![id.clone()]);
        }

        let data = self.graphql(
            "linear.create_label",
            MUTATION_CREATE_LABEL,
            serde_json::json!({ "teamId": team_id, "name": kind.label() }),
        )?;
        let id = data
            .get("issueLabelCreate")
            .filter(|c| c.get("success").and_then(|v| v.as_bool()).unwrap_or(false))
            .and_then(|c| c.get("issueLabel"))
            .and_then(|l| l.get("id"))
            .and_then(|v| v.as_str());
        match id {
            Some(id) => Ok(vec![id.to_string()]),
            // A task without its kind label is still a task; refusing to
            // create it because a label failed would be the wrong trade.
            None => Ok(Vec::new()),
        }
    }

    /// The UUID for a UUID or an identifier, asked for only when it is the
    /// latter.
    fn issue_uuid(&self, id: &str) -> Result<String, TaskError> {
        if is_uuid(id) {
            return Ok(id.to_string());
        }
        let data = self.graphql(
            "linear.issue_id",
            QUERY_ISSUE_ID,
            serde_json::json!({ "id": id }),
        )?;
        data.get("issue")
            .and_then(|i| i.get("id"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| TaskError::Protocol {
                provider: PROVIDER_ID,
                message: format!("no issue `{id}`"),
            })
    }

    fn graphql(
        &self,
        label: &'static str,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<serde_json::Value, TaskError> {
        let cred = self.credential()?;
        let mut req = HttpRequest::post(API_URL)
            .json(&serde_json::json!({ "query": query, "variables": variables }))
            .label(label)
            .timeout(TIMEOUT);
        // Only the queue poll is floored. The floor refuses rather than waits,
        // so on anything else it would fail an agent's second comment or
        // second sub-task in a row.
        if label == "linear.assigned" {
            req = req.min_interval(MIN_INTERVAL);
        }
        let req = Self::authorize(req, cred);

        let resp = http::send(req).map_err(|e| match e {
            // 401/403 mean the credential is bad — surfaced distinctly so the
            // UI prompts for re-auth instead of silently retrying forever.
            HttpError::Status(401) | HttpError::Status(403) => TaskError::Unauthorized {
                provider: PROVIDER_ID,
            },
            other => TaskError::Transport {
                provider: PROVIDER_ID,
                message: other.to_string(),
            },
        })?;

        if resp.status() == 401 || resp.status() == 403 {
            return Err(TaskError::Unauthorized {
                provider: PROVIDER_ID,
            });
        }
        if !resp.is_success() {
            return Err(TaskError::Transport {
                provider: PROVIDER_ID,
                message: format!("HTTP {}", resp.status()),
            });
        }

        let body: serde_json::Value = resp.json().map_err(|e| TaskError::Protocol {
            provider: PROVIDER_ID,
            message: e.to_string(),
        })?;

        // GraphQL reports application errors inside a 200 response.
        if let Some(errors) = body.get("errors").and_then(|e| e.as_array())
            && !errors.is_empty()
        {
            let joined = errors
                .iter()
                .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                .collect::<Vec<_>>()
                .join("; ");
            let message = if joined.is_empty() {
                "GraphQL error".to_string()
            } else {
                joined
            };
            // Linear reports an invalid/expired token as a GraphQL error rather
            // than an HTTP status on some endpoints.
            if message.to_ascii_lowercase().contains("authentication") {
                return Err(TaskError::Unauthorized {
                    provider: PROVIDER_ID,
                });
            }
            return Err(TaskError::Protocol {
                provider: PROVIDER_ID,
                message,
            });
        }

        body.get("data")
            .cloned()
            .ok_or_else(|| TaskError::Protocol {
                provider: PROVIDER_ID,
                message: "response had no `data`".into(),
            })
    }
}

fn check_provider(id: &TaskId) -> Result<(), TaskError> {
    if id.provider == PROVIDER_ID {
        Ok(())
    } else {
        Err(TaskError::Protocol {
            provider: PROVIDER_ID,
            message: format!("task belongs to provider `{}`", id.provider),
        })
    }
}

/// Whether `id` is a Linear UUID rather than an identifier like `QBL-12`.
fn is_uuid(id: &str) -> bool {
    id.len() == 36
        && id.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// The `issueUpdate` input for a patch: only the fields it changes.
fn update_input(patch: &TaskPatch) -> serde_json::Value {
    let mut input = serde_json::Map::new();
    if let Some(title) = patch.title.as_deref() {
        input.insert("title".into(), title.trim().into());
    }
    if let Some(description) = patch.description.as_deref() {
        input.insert("description".into(), description.into());
    }
    serde_json::Value::Object(input)
}

/// Map a Linear workflow-state type onto a normalized category.
///
/// Linear has no distinct "in review" type — review columns are `started` — so
/// [`TaskState::InReview`] is never produced here. It exists for providers that
/// model review as its own category (Jira does).
fn map_state(type_: &str) -> TaskState {
    match type_ {
        "backlog" | "triage" => TaskState::Backlog,
        "unstarted" => TaskState::Todo,
        "started" => TaskState::InProgress,
        "completed" => TaskState::Done,
        "canceled" => TaskState::Canceled,
        _ => TaskState::Unknown,
    }
}

/// The Linear state type to move to for a normalized category.
fn target_state_type(state: TaskState) -> &'static str {
    match state {
        TaskState::Backlog => "backlog",
        TaskState::Todo => "unstarted",
        // Linear folds review into `started`; see `map_state`.
        TaskState::InProgress | TaskState::InReview => "started",
        TaskState::Done => "completed",
        TaskState::Canceled => "canceled",
        TaskState::Unknown => "unstarted",
    }
}

/// Parse one issue node. Returns `None` for a node missing the fields the
/// harness cannot work without, so one malformed row can't fail the whole poll.
/// A team's label names (lowercased) to their ids.
fn label_map(team: &serde_json::Value) -> std::collections::HashMap<String, String> {
    team.get("labels")
        .and_then(|l| l.get("nodes"))
        .and_then(|n| n.as_array())
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|n| {
                    Some((
                        n.get("name")?.as_str()?.trim().to_ascii_lowercase(),
                        n.get("id")?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn team_id_of(team: &serde_json::Value) -> Result<String, TaskError> {
    team.get("id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| TaskError::Protocol {
            provider: PROVIDER_ID,
            message: "a team came back without an id".into(),
        })
}

/// Linear's groupings for one issue, in the harness's own vocabulary.
///
/// Linear's Project is a body of work and its Cycle is a time box, which is
/// what [`GroupAxis::Project`] and [`GroupAxis::Iteration`] mean — Azure
/// DevOps will map its area and iteration paths onto the same two.
///
/// A group with no usable name is dropped rather than shown blank: an
/// unnamed row in a filter list is worse than one fewer row. The exception is
/// a cycle, which Linear routinely leaves unnamed and numbers instead.
fn parse_groups(node: &serde_json::Value) -> Vec<TaskGroup> {
    let mut groups = Vec::new();
    let named = |key: &str, axis: GroupAxis| -> Option<TaskGroup> {
        let v = node.get(key)?;
        let id = v.get("id")?.as_str()?;
        let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("").trim();
        (!name.is_empty()).then(|| TaskGroup::new(axis, id, name))
    };
    groups.extend(named("team", GroupAxis::Team));
    groups.extend(named("project", GroupAxis::Project));
    groups.extend(cycle_group(node.get("cycle")));
    groups
}

/// A cycle as a group, naming it by number when Linear has no name for it.
///
/// Split out because it is the only axis whose display name may have to be
/// composed, and burying that in a closure hid the one case worth testing.
fn cycle_group(cycle: Option<&serde_json::Value>) -> Option<TaskGroup> {
    let cycle = cycle?;
    let id = cycle.get("id")?.as_str()?;
    let name = cycle
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .trim();
    if !name.is_empty() {
        return Some(TaskGroup::new(GroupAxis::Iteration, id, name));
    }
    // Most Linear cycles are never named; they are "Cycle 42" to everyone
    // reading the board, so that is what the filter should say.
    let number = cycle.get("number").and_then(|n| n.as_u64())?;
    Some(TaskGroup::new(
        GroupAxis::Iteration,
        id,
        format!("Cycle {number}"),
    ))
}

fn parse_issue(node: &serde_json::Value) -> Option<Task> {
    let str_at = |k: &str| node.get(k).and_then(|v| v.as_str());
    let id = str_at("id")?;
    let identifier = str_at("identifier")?;
    let state = node.get("state");
    let labels: Vec<String> = node
        .get("labels")
        .and_then(|l| l.get("nodes"))
        .and_then(|n| n.as_array())
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|l| l.get("name").and_then(|v| v.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let parent = node.get("parent").and_then(|p| {
        let id = p.get("id").and_then(|v| v.as_str())?;
        let key = p.get("identifier").and_then(|v| v.as_str()).unwrap_or(id);
        Some((id.to_string(), key.to_string()))
    });
    let state_type = state
        .and_then(|s| s.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Linear has no issue-type field, so the breakdown level is derived from
    // labels. A sub-issue with no telling label falls back to Story rather
    // than Task: it sits under something, which is what a story is.
    let kind = TaskKind::from_labels(labels.iter().map(String::as_str)).unwrap_or({
        if parent.is_some() {
            TaskKind::Story
        } else {
            TaskKind::Task
        }
    });
    let title = str_at("title").unwrap_or_default();

    Some(Task {
        id: TaskId::new(PROVIDER_ID, id),
        display_key: identifier.to_string(),
        title: title.to_string(),
        description: str_at("description").map(str::to_string),
        state: map_state(state_type),
        state_name: state
            .and_then(|s| s.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        url: str_at("url").unwrap_or_default().to_string(),
        // Not Linear's `branchName`: that follows each user's personal format
        // and leads with their username. The identifier in okena's name is
        // enough for Linear to link the branch and its PR.
        branch_name: task_branch_name(kind, identifier, title),
        updated_at: str_at("updatedAt").unwrap_or_default().to_string(),
        kind,
        parent_id: parent.as_ref().map(|(id, _)| id.clone()),
        parent_key: parent.as_ref().map(|(_, key)| key.clone()),
        labels,
        groups: parse_groups(node),
    })
}

impl TaskProvider for LinearProvider {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn display_name(&self) -> &'static str {
        "Linear"
    }

    fn auth_status(&self) -> AuthStatus {
        match &self.credential {
            None => AuthStatus::Disconnected,
            Some(c) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                if c.is_expired(now, 60) {
                    AuthStatus::Expired
                } else {
                    AuthStatus::Connected {
                        account: self.account.read().ok().and_then(|a| a.clone()),
                    }
                }
            }
        }
    }

    fn list_assigned(&self) -> Result<Vec<Task>, TaskError> {
        let data = self.graphql(
            "linear.assigned",
            QUERY_ASSIGNED,
            serde_json::json!({ "first": PAGE_SIZE }),
        )?;

        let viewer = data.get("viewer").ok_or_else(|| TaskError::Protocol {
            provider: PROVIDER_ID,
            message: "response had no `viewer`".into(),
        })?;

        if let Some(name) = viewer.get("name").and_then(|v| v.as_str())
            && let Ok(mut slot) = self.account.write()
        {
            *slot = Some(name.to_string());
        }

        let nodes = viewer
            .get("assignedIssues")
            .and_then(|a| a.get("nodes"))
            .and_then(|n| n.as_array())
            .ok_or_else(|| TaskError::Protocol {
                provider: PROVIDER_ID,
                message: "response had no `assignedIssues.nodes`".into(),
            })?;

        let total = nodes.len();
        let tasks: Vec<Task> = nodes.iter().filter_map(parse_issue).collect();
        if tasks.len() != total {
            log::warn!(
                "[tasks] linear: skipped {} malformed issue node(s)",
                total - tasks.len()
            );
        }
        Ok(tasks)
    }

    fn list_containers(&self) -> Result<Vec<TaskContainer>, TaskError> {
        let data = self.graphql("linear.teams", QUERY_TEAMS, serde_json::json!({}))?;
        let nodes = data
            .get("viewer")
            .and_then(|v| v.get("teams"))
            .and_then(|t| t.get("nodes"))
            .and_then(|n| n.as_array())
            .ok_or_else(|| TaskError::Protocol {
                provider: PROVIDER_ID,
                message: "could not read your teams".into(),
            })?;
        Ok(nodes
            .iter()
            .filter_map(|n| {
                Some(TaskContainer {
                    id: n.get("id")?.as_str()?.to_string(),
                    name: n.get("name")?.as_str()?.to_string(),
                    key: n
                        .get("key")
                        .and_then(|k| k.as_str())
                        .unwrap_or_default()
                        .to_string(),
                })
            })
            .collect())
    }

    fn create_task(&self, draft: &TaskDraft) -> Result<Task, TaskError> {
        if draft.title.trim().is_empty() {
            return Err(TaskError::NeedsChoice {
                message: "a task needs a title".into(),
            });
        }

        // A sub-task inherits its parent's team — Linear has no cross-team
        // parenting, and asking the user to pick one that must match would be
        // a choice with exactly one right answer.
        let (team_id, labels, parent_id) = match draft.parent_external_id.as_deref() {
            Some(parent) => {
                let data = self.graphql(
                    "linear.parent_context",
                    QUERY_PARENT_CONTEXT,
                    serde_json::json!({ "id": parent }),
                )?;
                let team = data
                    .get("issue")
                    .and_then(|i| i.get("team"))
                    .ok_or_else(|| TaskError::Protocol {
                        provider: PROVIDER_ID,
                        message: "could not read the parent issue's team".into(),
                    })?;
                // The parent may have been named by its identifier; the
                // mutation gets the UUID this lookup returned.
                let parent_id = data
                    .get("issue")
                    .and_then(|i| i.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(parent)
                    .to_string();
                (team_id_of(team)?, label_map(team), Some(parent_id))
            }
            None => {
                let team_id = draft.container_id.clone().ok_or_else(|| {
                    // Not a protocol failure: the caller simply has to say
                    // which team, and the UI turns this into a picker.
                    TaskError::NeedsChoice {
                        message: "choose a team for the new task".into(),
                    }
                })?;
                let data = self.graphql(
                    "linear.team_labels",
                    QUERY_TEAM_LABELS,
                    serde_json::json!({ "id": team_id }),
                )?;
                let team = data.get("team").ok_or_else(|| TaskError::Protocol {
                    provider: PROVIDER_ID,
                    message: "could not read the team".into(),
                })?;
                (team_id, label_map(team), None)
            }
        };

        let label_ids = self.kind_label_ids(&team_id, draft.kind, &labels)?;

        let data = self.graphql(
            "linear.create_issue",
            MUTATION_CREATE_ISSUE,
            serde_json::json!({
                "teamId": team_id,
                "title": draft.title.trim(),
                "description": draft.description,
                "parentId": parent_id,
                "labelIds": label_ids,
            }),
        )?;

        let created = data.get("issueCreate").ok_or_else(|| TaskError::Protocol {
            provider: PROVIDER_ID,
            message: "issueCreate returned nothing".into(),
        })?;
        if !created
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Err(TaskError::Protocol {
                provider: PROVIDER_ID,
                message: "issueCreate reported failure".into(),
            });
        }
        created
            .get("issue")
            .and_then(parse_issue)
            .ok_or_else(|| TaskError::Protocol {
                provider: PROVIDER_ID,
                message: "issueCreate returned an issue okena could not read".into(),
            })
    }

    fn list_children(&self, id: &TaskId) -> Result<Vec<Task>, TaskError> {
        if id.provider != PROVIDER_ID {
            return Err(TaskError::Protocol {
                provider: PROVIDER_ID,
                message: format!("task belongs to provider `{}`", id.provider),
            });
        }
        let data = self.graphql(
            "linear.children",
            QUERY_CHILDREN,
            serde_json::json!({ "id": id.external_id }),
        )?;
        let nodes = data
            .get("issue")
            .and_then(|i| i.get("children"))
            .and_then(|c| c.get("nodes"))
            .and_then(|n| n.as_array())
            .ok_or_else(|| TaskError::Protocol {
                provider: PROVIDER_ID,
                message: "could not read the issue's sub-tasks".into(),
            })?;
        Ok(nodes.iter().filter_map(parse_issue).collect())
    }

    fn set_state(&self, id: &TaskId, state: TaskState) -> Result<(), TaskError> {
        if id.provider != PROVIDER_ID {
            return Err(TaskError::Protocol {
                provider: PROVIDER_ID,
                message: format!("task belongs to provider `{}`", id.provider),
            });
        }

        let want = target_state_type(state);
        let data = self.graphql(
            "linear.issue_states",
            QUERY_ISSUE_STATES,
            serde_json::json!({ "id": id.external_id }),
        )?;

        let states = data
            .get("issue")
            .and_then(|i| i.get("team"))
            .and_then(|t| t.get("states"))
            .and_then(|s| s.get("nodes"))
            .and_then(|n| n.as_array())
            .ok_or_else(|| TaskError::Protocol {
                provider: PROVIDER_ID,
                message: "could not read the issue's team workflow states".into(),
            })?;

        // A team may define several states of the same type ("In Progress",
        // "In Review" are both `started`). Lowest `position` is the team's
        // leftmost, which is the canonical entry point for that category.
        let chosen = states
            .iter()
            .filter(|s| s.get("type").and_then(|v| v.as_str()) == Some(want))
            .min_by(|a, b| {
                let pos = |v: &serde_json::Value| {
                    v.get("position")
                        .and_then(|p| p.as_f64())
                        .unwrap_or(f64::MAX)
                };
                pos(a).total_cmp(&pos(b))
            })
            .and_then(|s| s.get("id").and_then(|v| v.as_str()))
            .ok_or_else(|| TaskError::Protocol {
                provider: PROVIDER_ID,
                message: format!("the issue's team has no `{want}` workflow state"),
            })?;

        // `id` may be an identifier; the lookup above returned the UUID.
        let issue_id = data
            .get("issue")
            .and_then(|i| i.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or(id.external_id.as_str())
            .to_string();
        let data = self.graphql(
            "linear.set_state",
            MUTATION_SET_STATE,
            serde_json::json!({ "id": issue_id, "stateId": chosen }),
        )?;

        let ok = data
            .get("issueUpdate")
            .and_then(|u| u.get("success"))
            .and_then(|s| s.as_bool())
            .unwrap_or(false);
        if ok {
            Ok(())
        } else {
            Err(TaskError::Protocol {
                provider: PROVIDER_ID,
                message: "issueUpdate reported failure".into(),
            })
        }
    }

    fn get_task(&self, id: &TaskId) -> Result<Task, TaskError> {
        check_provider(id)?;
        let data = self.graphql(
            "linear.issue",
            QUERY_ISSUE,
            serde_json::json!({ "id": id.external_id }),
        )?;
        data.get("issue")
            .and_then(parse_issue)
            .ok_or_else(|| TaskError::Protocol {
                provider: PROVIDER_ID,
                message: format!("could not read issue `{}`", id.external_id),
            })
    }

    fn update_task(&self, id: &TaskId, patch: &TaskPatch) -> Result<Task, TaskError> {
        check_provider(id)?;
        let issue_id = self.issue_uuid(&id.external_id)?;
        let data = self.graphql(
            "linear.update_issue",
            MUTATION_UPDATE_ISSUE,
            serde_json::json!({ "id": issue_id, "input": update_input(patch) }),
        )?;
        let updated = data
            .get("issueUpdate")
            .filter(|u| u.get("success").and_then(|v| v.as_bool()).unwrap_or(false))
            .ok_or_else(|| TaskError::Protocol {
                provider: PROVIDER_ID,
                message: "issueUpdate reported failure".into(),
            })?;
        updated
            .get("issue")
            .and_then(parse_issue)
            .ok_or_else(|| TaskError::Protocol {
                provider: PROVIDER_ID,
                message: "issueUpdate returned an issue okena could not read".into(),
            })
    }

    fn add_comment(&self, id: &TaskId, body: &str) -> Result<(), TaskError> {
        check_provider(id)?;
        let issue_id = self.issue_uuid(&id.external_id)?;
        let data = self.graphql(
            "linear.comment",
            MUTATION_CREATE_COMMENT,
            serde_json::json!({ "issueId": issue_id, "body": body }),
        )?;
        let ok = data
            .get("commentCreate")
            .and_then(|c| c.get("success"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if ok {
            Ok(())
        } else {
            Err(TaskError::Protocol {
                provider: PROVIDER_ID,
                message: "commentCreate reported failure".into(),
            })
        }
    }

    fn get_tasks(&self, ids: &[TaskId]) -> Result<Vec<Task>, TaskError> {
        // Tasks okena holds carry the UUID already, so this is normally no
        // lookup at all; a key still works, at a round trip each.
        let mut uuids = Vec::with_capacity(ids.len());
        for id in ids {
            check_provider(id)?;
            uuids.push(self.issue_uuid(&id.external_id)?);
        }
        let mut tasks = Vec::with_capacity(uuids.len());
        for chunk in uuids.chunks(PAGE_SIZE as usize) {
            let data = self.graphql(
                "linear.issues_by_id",
                QUERY_ISSUES_BY_ID,
                serde_json::json!({ "ids": chunk, "first": chunk.len() }),
            )?;
            let nodes = data
                .get("issues")
                .and_then(|i| i.get("nodes"))
                .and_then(|n| n.as_array())
                .ok_or_else(|| TaskError::Protocol {
                    provider: PROVIDER_ID,
                    message: "could not read the issues".into(),
                })?;
            tasks.extend(nodes.iter().filter_map(parse_issue));
        }
        Ok(tasks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_linear_state_types() {
        assert_eq!(map_state("backlog"), TaskState::Backlog);
        assert_eq!(map_state("triage"), TaskState::Backlog);
        assert_eq!(map_state("unstarted"), TaskState::Todo);
        assert_eq!(map_state("started"), TaskState::InProgress);
        assert_eq!(map_state("completed"), TaskState::Done);
        assert_eq!(map_state("canceled"), TaskState::Canceled);
        // An unrecognized type must not masquerade as a real category.
        assert_eq!(map_state("something-new"), TaskState::Unknown);
    }

    #[test]
    fn closed_categories_are_closed() {
        assert!(TaskState::Done.is_closed());
        assert!(TaskState::Canceled.is_closed());
        assert!(!TaskState::InProgress.is_closed());
    }

    #[test]
    fn parses_a_full_issue_node() {
        let node = serde_json::json!({
            "id": "uuid-1",
            "identifier": "LIN-42",
            "title": "Fix the thing",
            "description": "details",
            "url": "https://linear.app/x/issue/LIN-42",
            "updatedAt": "2026-08-26T10:00:00.000Z",
            "state": { "name": "In Progress", "type": "started" }
        });
        let t = parse_issue(&node).expect("should parse");
        assert_eq!(t.id, TaskId::new("linear", "uuid-1"));
        assert_eq!(t.display_key, "LIN-42");
        assert_eq!(t.state, TaskState::InProgress);
        assert_eq!(t.state_name, "In Progress");
        assert_eq!(t.branch_name, "chore/lin-42-fix-the-thing");
    }

    #[test]
    fn rejects_node_without_identity() {
        // No `id` — unusable, must be skipped rather than defaulted.
        let node = serde_json::json!({ "identifier": "LIN-1", "title": "x" });
        assert!(parse_issue(&node).is_none());
    }

    #[test]
    fn tolerates_missing_optional_fields() {
        let node = serde_json::json!({ "id": "u", "identifier": "LIN-2" });
        let t = parse_issue(&node).expect("identity present, should parse");
        assert_eq!(t.title, "");
        assert_eq!(t.description, None);
        assert_eq!(t.state, TaskState::Unknown);
    }

    #[test]
    fn ignores_linear_branch_name() {
        let p = LinearProvider::new(None);
        let t = parse_issue(&serde_json::json!({
            "id": "u", "identifier": "LIN-3", "title": "Some title",
            "branchName": "nima/lin-3-some-title",
            "labels": { "nodes": [{ "name": "Feature" }] }
        }))
        .unwrap();
        assert_eq!(t.branch_name, "feat/lin-3-some-title");
        assert_eq!(p.branch_name(&t), "feat/lin-3-some-title");
    }

    #[test]
    fn branch_prefix_follows_kind() {
        let branch = |extra: serde_json::Value| {
            let mut node = serde_json::json!({
                "id": "u", "identifier": "LIN-4", "title": "Some Title"
            });
            node.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            parse_issue(&node).unwrap().branch_name
        };
        let bug = serde_json::json!({ "labels": { "nodes": [{ "name": "Bug" }] } });
        let sub = serde_json::json!({ "parent": { "id": "p", "identifier": "LIN-1" } });
        assert_eq!(branch(bug), "fix/lin-4-some-title");
        assert_eq!(branch(serde_json::json!({})), "chore/lin-4-some-title");
        // A sub-issue with no kind label reads as a Story.
        assert_eq!(branch(sub), "feat/lin-4-some-title");
    }

    #[test]
    fn api_key_goes_raw_and_oauth_gets_bearer() {
        // Linear rejects a personal API key sent with a `Bearer` prefix, so the
        // two credential kinds must produce different headers.
        let raw = LinearProvider::authorize(
            HttpRequest::post(API_URL),
            &Credential::ApiKey("lin_api_xyz".into()),
        );
        assert_eq!(raw.header_value("Authorization"), Some("lin_api_xyz"));

        let oauth = LinearProvider::authorize(
            HttpRequest::post(API_URL),
            &Credential::OAuth {
                access_token: "tok".into(),
                refresh_token: None,
                expires_at: None,
            },
        );
        assert_eq!(oauth.header_value("Authorization"), Some("Bearer tok"));
    }

    #[test]
    fn unauthenticated_provider_reports_disconnected() {
        let p = LinearProvider::new(None);
        assert_eq!(p.auth_status(), AuthStatus::Disconnected);
        assert!(matches!(
            p.list_assigned(),
            Err(TaskError::NotAuthenticated { .. })
        ));
    }

    #[test]
    fn set_state_rejects_foreign_provider_task() {
        let p = LinearProvider::new(Some(Credential::ApiKey("k".into())));
        let foreign = TaskId::new("jira", "ABC-1");
        assert!(matches!(
            p.set_state(&foreign, TaskState::Done),
            Err(TaskError::Protocol { .. })
        ));
    }

    #[test]
    fn tells_a_uuid_from_an_identifier() {
        assert!(is_uuid("f0fe2bc3-d9fe-4db4-b8ce-9aac476ac19d"));
        assert!(!is_uuid("QBL-373"));
        assert!(!is_uuid("f0fe2bc3d9fe4db4b8ce9aac476ac19d0000"));
    }

    #[test]
    fn an_update_sends_only_what_changes() {
        // An explicit null would clear what Linear already has.
        let title_only = update_input(&TaskPatch {
            title: Some(" New title ".into()),
            description: None,
        });
        assert_eq!(title_only, serde_json::json!({ "title": "New title" }));
        let cleared = update_input(&TaskPatch {
            title: None,
            description: Some(String::new()),
        });
        assert_eq!(cleared, serde_json::json!({ "description": "" }));
    }

    #[test]
    fn reads_and_writes_refuse_a_foreign_provider_task() {
        let p = LinearProvider::new(Some(Credential::ApiKey("k".into())));
        let foreign = TaskId::new("azure_devops", "7");
        assert!(matches!(
            p.get_task(&foreign),
            Err(TaskError::Protocol { .. })
        ));
        assert!(matches!(
            p.add_comment(&foreign, "hi"),
            Err(TaskError::Protocol { .. })
        ));
    }
}

#[cfg(test)]
mod group_tests {
    use super::{cycle_group, parse_groups};
    use okena_core::tasks::GroupAxis;
    use serde_json::json;

    #[test]
    fn an_issue_reports_its_team_project_and_cycle() {
        let groups = parse_groups(&json!({
            "team": { "id": "t1", "key": "QBL", "name": "Qblok" },
            "project": { "id": "p1", "name": "Harness" },
            "cycle": { "id": "c1", "number": 9, "name": "Hardening" },
        }));
        assert_eq!(
            groups
                .iter()
                .map(|g| (g.axis.clone(), &*g.name))
                .collect::<Vec<_>>(),
            vec![
                (GroupAxis::Team, "Qblok"),
                (GroupAxis::Project, "Harness"),
                (GroupAxis::Iteration, "Hardening"),
            ]
        );
    }

    #[test]
    fn absent_groupings_are_simply_absent() {
        // Verified against the live API: an issue with no project and no cycle
        // comes back with both keys present and null, which must not produce
        // two blank filter rows.
        let groups = parse_groups(&json!({
            "team": { "id": "t1", "key": "QBL", "name": "Qblok" },
            "project": serde_json::Value::Null,
            "cycle": serde_json::Value::Null,
        }));
        assert_eq!(groups.len(), 1, "{groups:?}");
        assert_eq!(groups[0].axis, GroupAxis::Team);
    }

    #[test]
    fn an_unnamed_cycle_is_named_by_its_number() {
        let g = cycle_group(Some(&json!({ "id": "c1", "number": 42, "name": "" })))
            .expect("a numbered cycle is still a cycle");
        assert_eq!(g.name, "Cycle 42");
        assert_eq!(g.id, "c1");
    }

    #[test]
    fn a_cycle_with_neither_name_nor_number_is_dropped() {
        // Nothing to call it, so it would render as an empty filter row.
        assert!(cycle_group(Some(&json!({ "id": "c1" }))).is_none());
        assert!(cycle_group(None).is_none());
    }

    #[test]
    fn every_issue_query_carries_the_fields_it_spreads() {
        for query in [
            super::QUERY_ASSIGNED,
            super::QUERY_CHILDREN,
            super::QUERY_ISSUE,
            super::MUTATION_CREATE_ISSUE,
            super::MUTATION_UPDATE_ISSUE,
        ] {
            assert!(query.contains("...IssueFields"), "{query}");
            assert!(query.contains("fragment IssueFields on Issue"), "{query}");
        }
    }

    #[test]
    fn a_group_with_a_blank_name_is_dropped_rather_than_shown_empty() {
        let groups = parse_groups(&json!({
            "team": { "id": "t1", "name": "   " },
            "project": { "id": "p1", "name": "Harness" },
        }));
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].axis, GroupAxis::Project);
    }
}

/// Writes by display key, against a mocked API: every mutation must be handed
/// the UUID, since an identifier changes when its issue moves team.
#[cfg(test)]
mod mock_tests {
    use super::*;
    use crate::providers::NET;
    use okena_transport::http::{HttpResponse, testing};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    const UUID: &str = "f0fe2bc3-d9fe-4db4-b8ce-9aac476ac19d";

    type Sent = Arc<Mutex<Vec<(String, Value)>>>;

    fn ok(v: Value) -> Result<HttpResponse, HttpError> {
        Ok(HttpResponse::new(200, vec![], v.to_string().into_bytes()))
    }

    fn issue_node() -> Value {
        json!({
            "id": UUID, "identifier": "QBL-9", "title": "Split payments",
            "state": { "name": "In Progress", "type": "started" },
        })
    }

    /// Answer each GraphQL operation by name, recording its variables.
    fn linear_api(sent: Sent) -> testing::MockGuard {
        testing::mock(move |req| {
            let body = req.json_body().cloned().unwrap_or(Value::Null);
            let operation = body["query"]
                .as_str()
                .and_then(|q| q.split_whitespace().nth(1))
                .and_then(|name| name.split('(').next())
                .unwrap_or("")
                .to_string();
            if let Ok(mut log) = sent.lock() {
                log.push((operation.clone(), body["variables"].clone()));
            }
            ok(match operation.as_str() {
                "IssueId" => json!({ "data": { "issue": { "id": UUID } } }),
                "UpdateIssue" => {
                    json!({ "data": { "issueUpdate": { "success": true, "issue": issue_node() } } })
                }
                "CreateComment" => json!({ "data": { "commentCreate": { "success": true } } }),
                "IssueStates" => json!({ "data": { "issue": { "id": UUID, "team": { "states": {
                    "nodes": [
                        { "id": "s-todo", "name": "Todo", "type": "unstarted", "position": 0.0 },
                        { "id": "s-doing", "name": "In Progress", "type": "started", "position": 1.0 },
                    ]
                } } } } }),
                "SetState" => json!({ "data": { "issueUpdate": { "success": true } } }),
                "ParentContext" => json!({ "data": { "issue": {
                    "id": UUID, "team": { "id": "team-1", "labels": { "nodes": [] } },
                } } }),
                "IssuesById" => json!({ "data": { "issues": { "nodes": [issue_node()] } } }),
                "CreateIssue" => {
                    json!({ "data": { "issueCreate": { "success": true, "issue": issue_node() } } })
                }
                other => panic!("unexpected operation `{other}`"),
            })
        })
    }

    fn provider() -> LinearProvider {
        LinearProvider::new(Some(Credential::ApiKey("k".into())))
    }

    fn by_key() -> TaskId {
        TaskId::new("linear", "QBL-9")
    }

    fn variables(sent: &Sent, operation: &str) -> Value {
        sent.lock()
            .ok()
            .and_then(|s| {
                s.iter()
                    .find(|(op, _)| op == operation)
                    .map(|(_, v)| v.clone())
            })
            .unwrap_or_else(|| panic!("no `{operation}` was sent"))
    }

    #[test]
    fn an_update_by_key_writes_to_the_uuid() {
        let _net = NET.lock().unwrap_or_else(|e| e.into_inner());
        let sent = Sent::default();
        let _mock = linear_api(sent.clone());

        let task = provider()
            .update_task(
                &by_key(),
                &TaskPatch {
                    title: None,
                    description: Some("Two cards".into()),
                },
            )
            .expect("updates");
        assert_eq!(task.display_key, "QBL-9");
        let vars = variables(&sent, "UpdateIssue");
        assert_eq!(vars["id"], UUID);
        assert_eq!(vars["input"], json!({ "description": "Two cards" }));
    }

    #[test]
    fn a_comment_by_key_is_filed_on_the_uuid() {
        let _net = NET.lock().unwrap_or_else(|e| e.into_inner());
        let sent = Sent::default();
        let _mock = linear_api(sent.clone());

        provider()
            .add_comment(&by_key(), "Picked up")
            .expect("comments");
        let vars = variables(&sent, "CreateComment");
        assert_eq!(vars["issueId"], UUID);
        assert_eq!(vars["body"], "Picked up");
    }

    #[test]
    fn a_state_change_by_key_moves_the_uuid() {
        let _net = NET.lock().unwrap_or_else(|e| e.into_inner());
        let sent = Sent::default();
        let _mock = linear_api(sent.clone());

        provider()
            .set_state(&by_key(), TaskState::InProgress)
            .expect("moves");
        assert_eq!(variables(&sent, "IssueStates")["id"], "QBL-9");
        let vars = variables(&sent, "SetState");
        assert_eq!(vars["id"], UUID);
        assert_eq!(vars["stateId"], "s-doing");
    }

    #[test]
    fn a_child_of_a_key_is_created_under_the_uuid() {
        let _net = NET.lock().unwrap_or_else(|e| e.into_inner());
        let sent = Sent::default();
        let _mock = linear_api(sent.clone());

        provider()
            .create_task(&TaskDraft {
                title: "Split payments".into(),
                parent_external_id: Some("QBL-9".into()),
                ..Default::default()
            })
            .expect("creates");
        let vars = variables(&sent, "CreateIssue");
        assert_eq!(vars["parentId"], UUID);
        assert_eq!(vars["teamId"], "team-1");
    }

    #[test]
    fn back_to_back_writes_are_not_refused_by_the_rate_floor() {
        // The floor refuses rather than waits; only the queue poll has one.
        let _net = NET.lock().unwrap_or_else(|e| e.into_inner());
        let _mock = linear_api(Sent::default());
        let p = provider();
        for _ in 0..3 {
            p.add_comment(&by_key(), "again").expect("not throttled");
        }
    }

    #[test]
    fn several_tasks_by_uuid_are_read_in_one_request() {
        let _net = NET.lock().unwrap_or_else(|e| e.into_inner());
        let sent = Sent::default();
        let _mock = linear_api(sent.clone());

        let ids = [TaskId::new("linear", UUID), TaskId::new("linear", UUID)];
        let tasks = provider().get_tasks(&ids).expect("reads");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].state_name, "In Progress");
        let log = sent.lock().map(|s| s.clone()).unwrap_or_default();
        // A UUID needs no lookup: the batch is the only request.
        assert_eq!(log.len(), 1, "{log:?}");
        assert_eq!(log[0].0, "IssuesById");
        assert_eq!(log[0].1["ids"], json!([UUID, UUID]));
    }

    #[test]
    fn issues_by_id_include_closed_ones_with_their_parents() {
        // What an ancestor is: often finished, often somebody else's. The queue
        // filters both out; reading by id must not.
        const EPIC: &str = "0c6d1f5e-1b2a-4c3d-9e8f-0a1b2c3d4e5f";
        const FEATURE: &str = "1d7e2a6f-2c3b-4d4e-8f9a-1b2c3d4e5f60";
        let _net = NET.lock().unwrap_or_else(|e| e.into_inner());
        let sent = Sent::default();
        let log = sent.clone();
        let _mock = testing::mock(move |req| {
            let body = req.json_body().cloned().unwrap_or(Value::Null);
            if let Ok(mut l) = log.lock() {
                l.push((
                    body["query"].as_str().unwrap_or("").to_string(),
                    body["variables"].clone(),
                ));
            }
            ok(json!({ "data": { "issues": { "nodes": [
                { "id": EPIC, "identifier": "QBL-1", "title": "Epic",
                  "state": { "name": "Done", "type": "completed" } },
                { "id": FEATURE, "identifier": "QBL-2", "title": "Feature",
                  "state": { "name": "Canceled", "type": "canceled" },
                  "parent": { "id": EPIC, "identifier": "QBL-1" } },
            ] } } }))
        });

        let ids = [TaskId::new("linear", FEATURE), TaskId::new("linear", EPIC)];
        let tasks = provider().get_tasks(&ids).expect("reads");

        let keys: Vec<_> = tasks.iter().map(|t| t.display_key.as_str()).collect();
        assert_eq!(keys, ["QBL-1", "QBL-2"]);
        assert!(tasks.iter().all(|t| t.state.is_closed()), "{tasks:?}");
        assert_eq!(tasks[1].parent_id.as_deref(), Some(EPIC));
        let log = sent.lock().map(|s| s.clone()).unwrap_or_default();
        assert_eq!(log.len(), 1, "one batch, no lookups: {log:?}");
        assert!(log[0].0.contains("IssuesById"), "{}", log[0].0);
        assert!(
            !log[0].0.contains("state:"),
            "no state filter on a read by id: {}",
            log[0].0
        );
    }
}
