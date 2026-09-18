//! Engineering-harness task actions.
//!
//! The daemon owns the provider credential and every network call, so a thin
//! client (desktop, web, mobile) never holds a task-manager token and never
//! talks to Linear directly — it asks the daemon, exactly as it does for git.
//!
//! Task *lists* are deliberately not cached in workspace state: they're remote
//! data whose staleness is invisible to the user, and a stale issue list is
//! worse than a slow one. Only the task↔worktree link persists, on the project.

use super::ActionResult;
use super::briefs::{self, PromptRoots};
use crate::workspace::focus::FocusManager;
use crate::workspace::persistence::AppSettings;
use crate::workspace::state::{WindowId, Workspace};
use okena_core::tasks::{TaskAuthState, TaskAuthStatusResponse, TaskProviderStatus};
use okena_knowledge::prompts::{Flow, Vars};
use okena_tasks::provider::{AuthStatus, Credential, TaskError, TaskProvider};
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::TerminalBackend;
use okena_workspace::context::WorkspaceCx;

/// Render a provider error for the UI.
///
/// `Unauthorized` and `NotAuthenticated` are worth distinguishing in the text
/// because they need different user action — reconnect vs connect — and the UI
/// surfaces the message verbatim.
fn describe(e: TaskError) -> String {
    e.to_string()
}

fn resolve(provider: &str) -> Result<Box<dyn TaskProvider>, String> {
    okena_tasks::provider_for(provider)
        .ok_or_else(|| format!("unknown task provider: `{provider}`"))
}

/// Map a provider's live auth state onto the shared wire type.
fn provider_status(p: &dyn TaskProvider) -> TaskProviderStatus {
    let auth = match p.auth_status() {
        AuthStatus::Disconnected => TaskAuthState::Disconnected,
        AuthStatus::Connected { account } => TaskAuthState::Connected { account },
        AuthStatus::Expired => TaskAuthState::Expired,
    };
    TaskProviderStatus {
        provider: p.id().to_string(),
        display_name: p.display_name().to_string(),
        auth,
    }
}

/// Auth state for every provider this build knows about. Local only.
pub(super) fn auth_status() -> ActionResult {
    let providers: Vec<TaskProviderStatus> = okena_tasks::KNOWN_PROVIDERS
        .iter()
        .filter_map(|id| okena_tasks::provider_for(id))
        .map(|p| provider_status(p.as_ref()))
        .collect();
    let response = TaskAuthStatusResponse { providers };
    match serde_json::to_value(&response) {
        Ok(v) => ActionResult::Ok(Some(v)),
        Err(e) => ActionResult::Err(format!("could not serialize auth status: {e}")),
    }
}

/// Verify an API key with one live call, then store it.
///
/// Verify-before-store is the point: a mistyped key that got written would fail
/// every later call with no obvious cause, and the user would have no signal
/// that the key — rather than the network — was the problem. For Azure DevOps
/// the same call checks the organization URL, so a mistyped one is refused too.
pub(super) fn connect_api_key(
    provider: String,
    api_key: String,
    organization_url: Option<String>,
) -> ActionResult {
    use okena_tasks::providers::azure_devops;

    let key = api_key.trim().to_string();
    if key.is_empty() {
        return ActionResult::Err("API key is empty".into());
    }
    // Construct a provider bound to the candidate credential without touching
    // disk.
    let (credential, candidate): (Credential, Box<dyn TaskProvider>) = match provider.as_str() {
        "linear" => {
            let credential = Credential::ApiKey(key);
            let candidate = Box::new(okena_tasks::LinearProvider::new(Some(credential.clone())));
            (credential, candidate)
        }
        azure_devops::PROVIDER_ID => {
            let url = match azure_devops::normalize_organization_url(
                organization_url.as_deref().unwrap_or_default(),
            ) {
                Ok(url) => url,
                Err(e) => return ActionResult::Err(e),
            };
            let credential = Credential::PersonalAccessToken {
                token: key,
                organization_url: url,
                account: None,
            };
            let candidate = Box::new(okena_tasks::AzureDevOpsProvider::new(Some(
                credential.clone(),
            )));
            (credential, candidate)
        }
        other => return ActionResult::Err(format!("unknown task provider: `{other}`")),
    };

    match candidate.list_assigned() {
        Ok(tasks) => {
            // Keep who the token belongs to, learned by the call just made, so
            // "Connected as …" needs no call of its own later.
            let credential = match credential {
                Credential::PersonalAccessToken {
                    token,
                    organization_url,
                    ..
                } => Credential::PersonalAccessToken {
                    token,
                    organization_url,
                    account: match candidate.auth_status() {
                        AuthStatus::Connected { account } => account,
                        _ => None,
                    },
                },
                other => other,
            };
            if let Err(e) = okena_tasks::store::save(&provider, &credential) {
                return ActionResult::Err(format!("could not store credential: {e}"));
            }
            // Re-read through the stored path so the reported status is what a
            // later call will actually see, not what we just held in hand.
            let stored = match resolve(&provider) {
                Ok(p) => provider_status(p.as_ref()),
                Err(e) => return ActionResult::Err(e),
            };
            match serde_json::to_value(&stored) {
                Ok(status) => ActionResult::Ok(Some(serde_json::json!({
                    "status": status,
                    "task_count": tasks.len(),
                }))),
                Err(e) => ActionResult::Err(format!("could not serialize status: {e}")),
            }
        }
        Err(e) => ActionResult::Err(describe(e)),
    }
}

pub(super) fn disconnect(provider: String) -> ActionResult {
    match okena_tasks::store::clear(&provider) {
        Ok(()) => ActionResult::Ok(Some(serde_json::json!({ "provider": provider }))),
        Err(e) => ActionResult::Err(format!("could not clear credential: {e}")),
    }
}

/// Teams or projects the user can file a new task in.
pub(super) fn containers(provider: String) -> ActionResult {
    let p = match resolve(&provider) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    match p.list_containers() {
        Ok(list) => match serde_json::to_value(&list) {
            Ok(v) => ActionResult::Ok(Some(serde_json::json!({ "containers": v }))),
            Err(e) => ActionResult::Err(format!("could not serialize teams: {e}")),
        },
        Err(e) => ActionResult::Err(describe(e)),
    }
}

/// Create a task, optionally as a child of another.
pub(super) fn create(
    provider: String,
    title: String,
    description: String,
    kind: String,
    parent_external_id: Option<String>,
    container_id: Option<String>,
) -> ActionResult {
    let p = match resolve(&provider) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    let description = description.trim().to_string();
    let draft = okena_tasks::provider::TaskDraft {
        title,
        // Empty means "no body", not an empty body — the provider should not
        // be asked to store a blank description.
        description: (!description.is_empty()).then_some(description),
        kind: parse_kind(&kind),
        parent_external_id,
        container_id,
    };
    task_result(p.create_task(&draft))
}

/// Record a task an agent just filed on the session that filed it.
///
/// `created` is the answer to the create. Best effort: the task exists on the
/// provider whether or not the session is still here, so a session that went
/// away — or an answer that is not a task, such as a choice the provider needs
/// made — records nothing and fails nothing. A plain push; matching it against
/// the session's other rows happens when the list is read. Returns whether it
/// was recorded.
pub(super) fn record_created_task(
    ws: &mut Workspace,
    project_id: &str,
    created: &serde_json::Value,
    cx: &mut impl WorkspaceCx,
) -> bool {
    let Ok(task) = serde_json::from_value::<okena_core::tasks::Task>(created.clone()) else {
        return false;
    };
    let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == project_id) else {
        return false;
    };
    p.agent
        .get_or_insert_with(Default::default)
        .assets
        .push(task_asset(&task, now_millis()));
    ws.notify_data(cx);
    true
}

/// A filed task as a session asset.
fn task_asset(task: &okena_core::tasks::Task, created_at: u64) -> okena_core::harness::AgentAsset {
    okena_core::harness::AgentAsset {
        kind: okena_core::harness::AgentAssetKind::Task,
        title: task.title.clone(),
        url: (!task.url.trim().is_empty()).then(|| task.url.clone()),
        project: None,
        branch: None,
        created_at,
        task: Some(okena_core::tasks::TaskRef::from(task)),
    }
}

/// Sub-tasks of a task, whoever they are assigned to.
pub(super) fn children(provider: String, task_external_id: String) -> ActionResult {
    let p = match resolve(&provider) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    let id = okena_core::tasks::TaskId::new(provider, task_external_id);
    match p.list_children(&id) {
        Ok(tasks) => match serde_json::to_value(&tasks) {
            Ok(v) => ActionResult::Ok(Some(serde_json::json!({ "tasks": v }))),
            Err(e) => ActionResult::Err(format!("could not serialize sub-tasks: {e}")),
        },
        Err(e) => ActionResult::Err(describe(e)),
    }
}

/// One task, by the provider's id or its display key.
pub(super) fn get(provider: String, task: String) -> ActionResult {
    let p = match resolve(&provider) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    task_result(p.get_task(&okena_core::tasks::TaskId::new(provider, task)))
}

/// Change a task's title or description.
pub(super) fn update(
    provider: String,
    task: String,
    title: Option<String>,
    description: Option<String>,
) -> ActionResult {
    let patch = okena_tasks::TaskPatch {
        title,
        description: description.map(|d| d.trim().to_string()),
    };
    if patch.is_empty() {
        return ActionResult::Err("nothing to change: pass a title, a description or both".into());
    }
    if patch.title.as_deref().is_some_and(|t| t.trim().is_empty()) {
        return ActionResult::Err("a task's title cannot be empty".into());
    }
    let p = match resolve(&provider) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    task_result(p.update_task(&okena_core::tasks::TaskId::new(provider, task), &patch))
}

/// Move a task to a state, and report where it landed.
pub(super) fn set_state(
    provider: String,
    task: String,
    state: okena_core::tasks::TaskState,
) -> ActionResult {
    if state == okena_core::tasks::TaskState::Unknown {
        return ActionResult::Err(
            "choose a state: backlog, todo, in_progress, in_review, done or canceled".into(),
        );
    }
    let p = match resolve(&provider) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    let id = okena_core::tasks::TaskId::new(provider, task);
    if let Err(e) = p.set_state(&id, state) {
        return ActionResult::Err(describe(e));
    }
    // Read back for the provider's own name for where it landed — a team's
    // in-progress column may be called anything. The move has happened, so a
    // failed read must not report it as a failed move.
    match p.get_task(&id) {
        Ok(task) => task_result(Ok(task)),
        Err(_) => ActionResult::Ok(Some(serde_json::json!({ "state": state }))),
    }
}

/// Comment on a task.
pub(super) fn comment(provider: String, task: String, body: String) -> ActionResult {
    let body = body.trim();
    if body.is_empty() {
        return ActionResult::Err("a comment needs a body".into());
    }
    let p = match resolve(&provider) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    match p.add_comment(
        &okena_core::tasks::TaskId::new(provider, task.clone()),
        body,
    ) {
        Ok(()) => ActionResult::Ok(Some(serde_json::json!({ "task": task, "commented": true }))),
        Err(e) => ActionResult::Err(describe(e)),
    }
}

fn task_result(result: Result<okena_core::tasks::Task, TaskError>) -> ActionResult {
    match result {
        Ok(task) => match serde_json::to_value(&task) {
            Ok(v) => ActionResult::Ok(Some(v)),
            Err(e) => ActionResult::Err(format!("could not serialize the task: {e}")),
        },
        Err(e) => error_result(e),
    }
}

/// A provider error as an action result.
///
/// `NeedsChoice` is not a failure — the caller has to decide something, such
/// as which team — so it comes back as a result carrying the question, which
/// an agent reads and acts on instead of being told the call failed.
fn error_result(e: TaskError) -> ActionResult {
    match e {
        TaskError::NeedsChoice { message } => {
            ActionResult::Ok(Some(serde_json::json!({ "needs_choice": message })))
        }
        other => ActionResult::Err(describe(other)),
    }
}

/// Several tasks at once, by provider id.
///
/// A failure is logged as well as returned: callers refresh in the background,
/// where an error nobody reads would otherwise vanish.
pub(super) fn get_many(provider: String, ids: Vec<String>) -> ActionResult {
    let p = match resolve(&provider) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    let ids: Vec<okena_core::tasks::TaskId> = ids
        .into_iter()
        .filter(|id| !id.trim().is_empty())
        .map(|id| okena_core::tasks::TaskId::new(provider.clone(), id))
        .collect();
    if ids.is_empty() {
        return ActionResult::Ok(Some(serde_json::json!({ "tasks": [] })));
    }
    match p.get_tasks(&ids) {
        Ok(tasks) => match serde_json::to_value(&tasks) {
            Ok(v) => ActionResult::Ok(Some(serde_json::json!({ "tasks": v }))),
            Err(e) => ActionResult::Err(format!("could not serialize tasks: {e}")),
        },
        Err(e) => {
            log::warn!(
                "[tasks] refreshing {} task(s) on {provider} failed: {e}",
                ids.len()
            );
            ActionResult::Err(describe(e))
        }
    }
}

/// Read a kind off the wire.
///
/// Unknown values fall back to `Task` rather than failing: a newer client — or
/// an agent guessing — asking for a kind this build does not model should still
/// get a task, not an error.
pub(super) fn parse_kind(raw: &str) -> okena_core::tasks::TaskKind {
    use okena_core::tasks::TaskKind;
    match raw.trim().to_ascii_lowercase().as_str() {
        "epic" | "initiative" => TaskKind::Epic,
        "feature" => TaskKind::Feature,
        "story" | "user story" => TaskKind::Story,
        "defect" | "bug" | "fix" | "hotfix" => TaskKind::Defect,
        _ => TaskKind::Task,
    }
}

/// Tasks assigned to the authenticated user.
pub(super) fn list(provider: String) -> ActionResult {
    let p = match resolve(&provider) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    match p.list_assigned() {
        Ok(tasks) => match serde_json::to_value(&tasks) {
            Ok(v) => ActionResult::Ok(Some(serde_json::json!({
                "provider": provider,
                "tasks": v,
            }))),
            Err(e) => ActionResult::Err(format!("could not serialize tasks: {e}")),
        },
        Err(e) => ActionResult::Err(describe(e)),
    }
}

/// The shell an agent session (or agent worktree) should run.
///
/// `None` when no agent command is configured — starting work then just creates
/// worktrees and leaves an ordinary shell. Launching an AI agent is opt-in.
#[allow(clippy::too_many_arguments)]
fn agent_shell(
    settings: &AppSettings,
    override_command: Option<&str>,
    task: &okena_core::tasks::Task,
    branch: &str,
    context: &[(String, String)],
    note: Option<&str>,
    shape: Option<&BriefShape>,
    prompts: &PromptRoots,
    context_items: &[okena_core::context::ContextItem],
    // Whether `context` lists worktrees cut for this work, or the repos
    // themselves. The brief must not call a shared checkout a worktree.
    worktrees: bool,
) -> Option<okena_terminal::shell_config::ShellType> {
    // An explicit override wins, including an explicit empty string, which is
    // how a caller says "worktrees only" despite a configured default.
    // Trim both paths: a whitespace-only value from either source must read as
    // "no agent", not become the program name.
    let command = match override_command {
        Some(c) => c,
        None => settings.harness.agent_command.as_deref().unwrap_or(""),
    }
    .trim()
    .to_string();
    if command.is_empty() {
        return None;
    }
    // Always briefed: an agent started on a task without its brief knows
    // nothing of the task, its worktrees, or how to report and verify.
    // Skills and agents go in before the brief is written, so the brief can
    // say whether they are loaded or list where they are.
    let install = super::agent_context::install(&command, context_items);
    let brief = task_brief(
        task,
        branch,
        context,
        note,
        shape,
        context_items,
        install.loaded(),
        prompts,
        worktrees,
    );
    // Named, so a restart can resume this exact conversation, then the agent's
    // own options. Both before the brief rather than after: a flag after a
    // positional prompt is not guaranteed to be read as a flag by every agent
    // CLI.
    let mut args = super::agent_resume::session_args(&command);
    args.extend(super::agent_options::option_args(&command, settings));
    args.extend(super::briefs::brief_args(&command, &brief));
    // Hand the agent okena's MCP server so it can ask what task it is on and
    // report back without the user configuring anything.
    args.extend(super::agent_mcp::injection_args(&command, settings));
    args.extend(install.args);

    Some(okena_terminal::shell_config::ShellType::Custom {
        path: command,
        args,
    })
}

/// Resolve the directory an agent session should run in.
///
/// Explicit argument wins, then the configured root, then the parent of the
/// first project — right for a `~/p/<repo>` layout, which is why the setting
/// exists for everyone else.
fn resolve_agent_root(
    explicit: Option<String>,
    settings: &AppSettings,
    first_project_path: &str,
) -> Option<String> {
    if let Some(root) = explicit.filter(|r| !r.trim().is_empty()) {
        return Some(root);
    }
    if let Some(root) = settings
        .harness
        .agent_root
        .clone()
        .filter(|r| !r.trim().is_empty())
    {
        return Some(root);
    }
    std::path::Path::new(first_project_path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
}

/// Where a task's agent session runs.
///
/// Inside the worktree when there is exactly one — that is the checkout the
/// agent is changing, and rooting it anywhere else makes every path it reads
/// relative to the wrong place. Above them when there are several, so it can
/// reach them all; `above` is that directory, already resolved.
fn session_root(worktree_paths: &[String], above: Option<String>) -> Option<String> {
    match worktree_paths {
        [only] => Some(only.clone()),
        _ => above,
    }
}

/// A `TaskStartWork`, unpacked.
pub(super) struct StartWork {
    pub(super) provider: String,
    pub(super) task_external_id: String,
    pub(super) project_ids: Vec<String>,
    pub(super) agent_root: Option<String>,
    pub(super) branch: Option<String>,
    pub(super) agent_command: Option<String>,
    pub(super) note: Option<String>,
    pub(super) coordinate: bool,
    pub(super) also: Vec<String>,
    pub(super) siblings: Vec<String>,
    pub(super) branches: std::collections::BTreeMap<String, String>,
    pub(super) hand_picked: bool,
    /// Map entries, specs and knowledge picked for the agents, as refs.
    pub(super) context: Vec<okena_core::context::ContextRef>,
}

/// Start work on a task across one or more projects.
///
/// Every task the start covers gets worktrees of its own, one per project, on
/// its own branch: the task itself and each one in `also`. One agent on three
/// tasks used to work all of them on the first one's branch, so their changes
/// landed as one. A coordinator over picked tasks gets none — it changes
/// nothing, and each agent it starts gets its worktrees from that start.
///
/// Order matters: worktrees are created first and links written second, so a
/// failure part-way leaves usable checkouts rather than links pointing at
/// nothing.
pub(super) fn start_work(
    ws: &mut Workspace,
    window_id: WindowId,
    req: StartWork,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let StartWork {
        provider,
        task_external_id,
        project_ids,
        agent_root,
        branch: branch_override,
        agent_command,
        note,
        coordinate,
        also,
        siblings,
        branches,
        hand_picked,
        context: context_refs,
    } = req;
    if project_ids.is_empty() {
        return ActionResult::Err("pick at least one project to work in".into());
    }
    // Validate every target up front: creating three of four worktrees and
    // then discovering the fourth was bogus is worse than refusing early.
    for id in &project_ids {
        if ws.project(id).is_none() {
            return ActionResult::Err(format!("project not found: {id}"));
        }
    }
    // Resolved on this side before anything is created; stale refs drop out.
    let context_items =
        super::context::resolve_for_launch(&ws.data.projects, settings, &context_refs);
    let scope = super::context::scope_projects(&project_ids, &context_items);

    let p = match resolve(&provider) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };

    // Re-fetch rather than trusting a branch name the client supplied: the
    // client's list may be minutes old, and the branch name is what every
    // worktree — and the provider's branch-to-issue linking — is keyed on.
    //
    // Trade-off: unlike the other task actions this read still runs under the
    // workspace lock, because the rest of this function creates worktrees.
    // Moving it onto the blocking pool means splitting start_work into a fetch
    // and an apply step; it is one round trip, so that waits until it shows.
    let task = match fetch_task(&*p, &provider, &task_external_id) {
        Ok(task) => task,
        Err(e) => return ActionResult::Err(e),
    };

    // A coordinator over tasks the user picked has no worktree: every branch
    // it could take is one of its agents', and it changes nothing anyway.
    let lone_coordinator = coordinate && hand_picked;

    // A name the user gave wins, when it says anything.
    let named = |b: Option<&String>| b.map(|b| b.trim().to_string()).filter(|b| !b.is_empty());

    // okena's `<kind>/<key>-<title>` name is the default; the user can still
    // name it themselves.
    let branch = if lone_coordinator {
        String::new()
    } else {
        named(branch_override.as_ref()).unwrap_or_else(|| {
            let own = p.branch_name(&task);
            // A coordinator of a task's sub-tasks makes no changes, so it
            // takes a branch of its own and leaves every task's — its own
            // task's too — to the agents it starts.
            if coordinate {
                okena_core::tasks::coordinator_branch(&own)
            } else {
                own
            }
        })
    };
    if !lone_coordinator && branch.is_empty() {
        return ActionResult::Err(format!(
            "could not derive a branch name for {}",
            task.display_key
        ));
    }

    // Resolved once: every launch below briefs from the same root, and
    // discovery walks the disk.
    let prompts = briefs::prompt_roots(&ws.data.projects, settings);
    let task_ref = okena_core::tasks::TaskRef::from(&task);

    // Every task the session covers beyond its own, read so each is linked
    // and gets its worktrees: a session belongs to every task it works on,
    // not only the one it is named after.
    let mut also_tasks = Vec::new();
    for key in &also {
        match fetch_task(&*p, &provider, key) {
            Ok(t) => also_tasks.push(t),
            Err(e) => return ActionResult::Err(e),
        }
    }
    let also_refs: Vec<okena_core::tasks::TaskRef> = also_tasks
        .iter()
        .map(okena_core::tasks::TaskRef::from)
        .collect();

    // The repos this start was pointed at, as the brief names them.
    let given: Vec<(String, String)> = project_ids
        .iter()
        .filter_map(|id| ws.project(id))
        .map(|p| (p.name.clone(), p.path.clone()))
        .collect();

    // Picked tasks started one per agent are told where the others work:
    // each sibling's branch, and the worktrees its own start creates — worked
    // out here, since those starts run after this one.
    let siblings: Vec<String> = if hand_picked {
        let mut listed = Vec::new();
        for key in &siblings {
            let sibling_branch = match named(branches.get(key)) {
                Some(b) => b,
                None => match fetch_task(&*p, &provider, key) {
                    Ok(t) => p.branch_name(&t),
                    Err(e) => return ActionResult::Err(e),
                },
            };
            let paths: Vec<String> = given
                .iter()
                .map(|(_, path)| worktree_path_for(path, &sibling_branch, settings))
                .collect();
            listed.push(picked_sibling(key, &sibling_branch, &paths, &prompts));
        }
        listed
    } else {
        siblings
    };

    // Everything the agent is told beyond its task. The agent-written note
    // arrives as-is — an agent wrote those words. okena's own notes are
    // partials, so how it describes a group or a fan-out is editable.
    //
    // A coordinator over picked tasks has them listed in its brief instead:
    // they are what it splits, not a group it was handed to do.
    let grouped: &[String] = if lone_coordinator { &[] } else { &also };
    let note = compose_note(note, grouped, &siblings, hand_picked, &task, &prompts);

    // A coordinator is told what it is splitting: the task's children, or the
    // tasks picked with it. Fetched here rather than handed in by the client:
    // the provider is the authority, and the client's list may be a refresh
    // behind a breakdown that just landed.
    let coordination = if !coordinate {
        None
    } else if hand_picked {
        if also.is_empty() {
            return ActionResult::Err(
                "pick at least two tasks for a coordinator to split".to_string(),
            );
        }
        let picked: Vec<okena_core::tasks::Task> = std::iter::once(task.clone())
            .chain(also_tasks.iter().cloned())
            .collect();
        Some(BriefShape::Picked(list_children(&picked, &prompts)))
    } else {
        let id = okena_core::tasks::TaskId::new(provider.clone(), task.id.external_id.clone());
        match p.list_children(&id) {
            Ok(children) if !children.is_empty() => {
                Some(BriefShape::Children(list_children(&children, &prompts)))
            }
            Ok(_) => {
                return ActionResult::Err(format!(
                    "{} has no sub-tasks to split",
                    task.display_key
                ));
            }
            Err(e) => return ActionResult::Err(describe(e)),
        }
    };
    let first_project_path = given
        .first()
        .map(|(_, path)| path.clone())
        .unwrap_or_default();

    let mut failed: Vec<serde_json::Value> = Vec::new();

    // ── Which task gets worktrees on which branch ────────────────────────────
    let mut work: Vec<(okena_core::tasks::Task, String)> = Vec::new();
    if !lone_coordinator {
        work.push((task.clone(), branch.clone()));
        for (key, t) in also.iter().zip(&also_tasks) {
            match named(branches.get(key)).unwrap_or_else(|| p.branch_name(t)) {
                b if b.is_empty() => failed.push(serde_json::json!({
                    "task": t.display_key,
                    "project": t.display_key,
                    "error": "could not derive a branch name",
                })),
                b => work.push((t.clone(), b)),
            }
        }
    }
    let several = work.len() > 1;

    // ── Worktrees, one per task per assigned project ─────────────────────────
    let mut created: Vec<serde_json::Value> = Vec::new();
    // Projects that were never candidates: not a checkout, so nothing to cut.
    let mut skipped: Vec<serde_json::Value> = Vec::new();

    for (work_task, work_branch) in &work {
        let work_ref = okena_core::tasks::TaskRef::from(work_task);
        for project_id in &project_ids {
            let project_name = ws
                .project(project_id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| project_id.clone());
            // Named for its task too when there are several, so a failure
            // says whose worktree it was.
            let label = if several {
                format!("{} in {project_name}", work_task.display_key)
            } else {
                project_name.clone()
            };

            // Asked first: outside a repository `git worktree add` exits 128,
            // and a project pointed at a projects root is a plain directory.
            let project_path = ws
                .project(project_id)
                .map(|p| p.path.clone())
                .unwrap_or_default();
            if let Some(reason) = worktree_skip_reason(&project_path) {
                skipped.push(serde_json::json!({
                    "task": work_task.display_key,
                    "project": label,
                    "path": project_path,
                    "reason": reason,
                }));
                continue;
            }

            let result = super::project::create_worktree(
                ws,
                window_id,
                project_id.clone(),
                work_branch.clone(),
                // New work by definition. An existing branch surfaces as a
                // create error rather than silently attaching to someone
                // else's work.
                true,
                // Never the agent. A worktree copies its repo's layout, so an
                // agent set as the worktree's shell started once per terminal
                // in that layout — four identical agents for a four-pane repo
                // — and again in every terminal opened there later. The
                // worktree stays the human's; the agent gets the session below.
                None,
                backend,
                terminals,
                settings,
                cx,
            );

            match result {
                ActionResult::Ok(Some(payload)) => {
                    let new_id = payload
                        .get("project_id")
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                    match new_id {
                        Some(new_id) => {
                            // Its own task only: this checkout holds that
                            // task's branch, whichever session works in it.
                            link_task(ws, &new_id, &work_ref, &[]);
                            created.push(serde_json::json!({
                                "task": work_task.display_key,
                                "branch": work_branch,
                                "project": project_name,
                                "project_id": new_id,
                                "path": payload.get("path").cloned(),
                            }));
                        }
                        None => failed.push(serde_json::json!({
                            "task": work_task.display_key,
                            "project": label,
                            "error": "worktree creation returned no project id",
                        })),
                    }
                }
                ActionResult::Ok(None) => failed.push(serde_json::json!({
                    "task": work_task.display_key,
                    "project": label,
                    "error": "worktree creation returned no project",
                })),
                ActionResult::Err(e) => failed.push(serde_json::json!({
                    "task": work_task.display_key,
                    "project": label,
                    "error": e,
                })),
            }
        }
    }

    if no_worktrees_is_fatal(lone_coordinator, &created, &failed) {
        let detail = failed
            .iter()
            .filter_map(|f| {
                let name = f.get("project")?.as_str()?;
                let err = f.get("error")?.as_str()?;
                Some(format!("{name}: {err}"))
            })
            .collect::<Vec<_>>()
            .join("; ");
        return ActionResult::Err(format!("no worktrees were created — {detail}"));
    }

    // ── Agent session ────────────────────────────────────────────────────────
    //
    // Always its own project, for one repo as much as for several. A session
    // is what okena recognizes as an agent: it is listed under AGENTS, nested
    // under its task's parent, and found by the task's launcher. An agent run
    // inside a worktree was none of those things.
    //
    // The brief names the worktrees, not the repos they were cut from — the
    // agent is meant to change the checkout, and pointing it at the original
    // repo sent it to the wrong directory. A coordinator has no worktrees, so
    // it is told the repos its agents will work in.
    let worktrees_of = |key: Option<&str>| -> Vec<(String, String)> {
        created
            .iter()
            .filter(|c| key.is_none() || c.get("task").and_then(|v| v.as_str()) == key)
            .filter_map(|c| {
                let name = c.get("project")?.as_str()?.to_string();
                let path = c.get("path")?.as_str()?.to_string();
                Some((name, path))
            })
            .collect()
    };
    let worktrees = worktrees_of(None);
    // One agent on several tasks is told which worktrees are whose.
    let group = several.then(|| {
        let keys: Vec<&str> = work.iter().map(|(t, _)| t.display_key.as_str()).collect();
        let listed: Vec<_> = work
            .iter()
            .map(|(t, b)| (t, b.as_str(), worktrees_of(Some(&t.display_key))))
            .collect();
        BriefShape::Group {
            key: join_keys(&keys),
            tasks: list_group(&listed, &prompts),
        }
    });
    let shape = coordination.or(group);
    // With no worktree of its own the agent is given the repos instead, and
    // must be told that is what they are: briefed as worktrees, it would take a
    // shared checkout for its own task branch and commit there.
    let has_worktrees = !worktrees.is_empty();
    let context: &[(String, String)] = if has_worktrees { &worktrees } else { &given };
    let shell = agent_shell(
        settings,
        agent_command.as_deref(),
        &task,
        &branch,
        context,
        note.as_deref(),
        shape.as_ref(),
        &prompts,
        &context_items,
        has_worktrees,
    );
    let mut agent_session: Option<serde_json::Value> = None;
    // No agent and one worktree: nothing would run in a session, so there is
    // no session. Several worktrees still get one — it is the place above
    // them — and a coordinator always does: it is the whole of what starts.
    // Nothing was cut and nothing failed: every project was a plain directory.
    // The session is then the only thing this start produces, so it must exist
    // — otherwise the call reports success having done nothing at all.
    let nothing_to_cut = created.is_empty() && !skipped.is_empty();
    let wants_session =
        lone_coordinator || shell.is_some() || created.len() > 1 || nothing_to_cut;
    let root = if lone_coordinator {
        let paths: Vec<String> = given.iter().map(|(_, path)| path.clone()).collect();
        coordinator_root(agent_root, &paths, settings)
    } else {
        let worktree_paths: Vec<String> = worktrees.iter().map(|(_, p)| p.clone()).collect();
        session_root(
            &worktree_paths,
            resolve_agent_root(agent_root, settings, &first_project_path),
        )
    };
    if lone_coordinator && root.is_none() {
        return ActionResult::Err(
            "set a projects root in Settings → Harness for a coordinator over several projects"
                .into(),
        );
    }
    if wants_session && let Some(root) = root {
        // No "(agent)" suffix: the sidebar badges the row with what it is,
        // and a name repeating the badge said the same thing twice.
        let name = task.display_key.clone();
        match ws.add_project(
            name.clone(),
            root.clone(),
            // One terminal: this is where the agent runs, once.
            true,
            &settings.hooks,
            window_id,
            cx,
        ) {
            Ok(session_id) => {
                link_task(ws, &session_id, &task_ref, &also_refs);
                if let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == session_id) {
                    p.agent_purpose = Some(okena_core::harness::AgentPurpose::Work);
                    // With no worktree to say which repos it was given, a
                    // coordinator keeps them: its agents start there.
                    if lone_coordinator || nothing_to_cut {
                        p.repo_ids = project_ids.clone();
                    }
                    // The repositories, not their worktrees: the stores a
                    // repository follows are what its lookups may see.
                    p.context_projects = scope.clone();
                }
                // Set before spawning: the terminal reads the project's default
                // shell as it starts.
                if let Some(shell) = shell
                    && let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == session_id)
                {
                    p.default_shell = Some(shell);
                }
                let result = super::spawn_session_terminals(
                    ws,
                    &session_id,
                    backend,
                    terminals,
                    settings,
                    cx,
                );
                if let ActionResult::Err(e) = result {
                    log::warn!("[tasks] agent session terminal failed to spawn: {e}");
                }
                agent_session = Some(serde_json::json!({
                    "project_id": session_id,
                    "name": name,
                    "root": root,
                }));
            }
            // The worktrees are real and usable even if the session project
            // couldn't be created, so report rather than fail the whole call.
            Err(e) => failed.push(serde_json::json!({
                "project": "agent session",
                "error": e,
            })),
        }
    }

    ws.notify_data(cx);

    let branches: Vec<serde_json::Value> = work
        .iter()
        .map(|(t, b)| serde_json::json!({ "task": t.display_key, "branch": b }))
        .collect();
    ActionResult::Ok(Some(serde_json::json!({
        "task": task_ref,
        "also_tasks": also_refs,
        "branch": (!branch.is_empty()).then_some(branch),
        "branches": branches,
        "created": created,
        // Reported, not silent: the caller sees why there is no worktree.
        "skipped": skipped,
        "failed": failed,
        "agent_session": agent_session,
    })))
}

/// Why this project cannot be given a worktree, if it cannot.
///
/// A projects root is a plain directory holding repositories, not a checkout of
/// one. `git worktree add` there exits 128, so such a project is skipped up
/// front rather than driven into a call that cannot succeed.
fn worktree_skip_reason(path: &str) -> Option<String> {
    okena_git::get_repo_root(std::path::Path::new(path))
        .is_none()
        .then(|| "not a git repository".to_string())
}

/// Whether cutting no worktrees should fail the whole start.
///
/// Only when one was meant to be cut and could not. Having nothing to cut —
/// every assigned project a plain directory — is what a coordinator has always
/// done: the start goes ahead and the agent works in the repos themselves.
fn no_worktrees_is_fatal(
    lone_coordinator: bool,
    created: &[serde_json::Value],
    failed: &[serde_json::Value],
) -> bool {
    !lone_coordinator && created.is_empty() && !failed.is_empty()
}

/// Where a coordinator over picked tasks runs, having no worktree of its own.
///
/// An explicit root wins. Otherwise one project's own checkout — the agent
/// reads it to place the tasks and changes nothing — or, for several, the
/// directory a multi-project session already runs in.
fn coordinator_root(
    explicit: Option<String>,
    project_paths: &[String],
    settings: &AppSettings,
) -> Option<String> {
    if let Some(root) = explicit.filter(|r| !r.trim().is_empty()) {
        return Some(root);
    }
    match project_paths {
        [only] => Some(only.clone()),
        _ => resolve_agent_root(
            None,
            settings,
            project_paths
                .first()
                .map(String::as_str)
                .unwrap_or_default(),
        ),
    }
}

/// The directory a worktree of `project_path` on `branch` gets: the path
/// `create_worktree` picks, worked out without creating anything.
fn worktree_path_for(project_path: &str, branch: &str, settings: &AppSettings) -> String {
    let (git_root, subdir) =
        okena_git::resolve_git_root_and_subdir(std::path::Path::new(project_path));
    okena_git::compute_target_paths(&git_root, &subdir, &settings.worktree.path_template, branch).1
}

/// Several keys as one phrase: `A`, `A and B`, `A, B and C`.
fn join_keys(keys: &[&str]) -> String {
    match keys {
        [] => String::new(),
        [only] => only.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// `- name (path)` lines, as the brief lists worktrees everywhere.
fn worktree_lines(worktrees: &[(String, String)]) -> String {
    worktrees
        .iter()
        .map(|(name, path)| format!("- {name} ({path})"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A task of a group, its branch, and its own `(name, path)` worktrees.
type GroupedTask<'a> = (&'a okena_core::tasks::Task, &'a str, Vec<(String, String)>);

/// Each task of a group with its branch and the worktrees that are its own,
/// one `task-in-group` partial apiece.
fn list_group(tasks: &[GroupedTask<'_>], prompts: &PromptRoots) -> String {
    let listed = tasks
        .iter()
        .map(|(t, branch, worktrees)| {
            briefs::fragment(
                "task-in-group",
                prompts,
                &Vars::from([
                    ("key", t.display_key.clone()),
                    ("title", t.title.clone()),
                    ("url", t.url.clone()),
                    ("branch", branch.to_string()),
                    ("worktrees", worktree_lines(worktrees)),
                    (
                        "description",
                        briefs::block(t.description.as_deref().unwrap_or_default()),
                    ),
                ]),
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    briefs::block(&listed)
}

/// One other picked task in a one-per-task note, with where its agent works.
fn picked_sibling(key: &str, branch: &str, paths: &[String], prompts: &PromptRoots) -> String {
    briefs::fragment(
        "picked-sibling",
        prompts,
        &Vars::from([
            ("key", key.to_string()),
            ("branch", branch.to_string()),
            ("worktrees", paths.join(", ")),
        ]),
    )
}

/// Read one task by provider id or display key.
///
/// The one task rather than the assigned queue: a sub-task a coordinator just
/// filed has no assignee, and the queue poll is rate-floored, so two starts
/// back to back would be refused. The queue is only the fallback for a
/// provider that cannot read a single task.
fn fetch_task(
    p: &dyn TaskProvider,
    provider: &str,
    id_or_key: &str,
) -> Result<okena_core::tasks::Task, String> {
    let id = okena_core::tasks::TaskId::new(provider, id_or_key);
    match p.get_task(&id) {
        Ok(task) => Ok(task),
        Err(TaskError::Unsupported { .. }) => p
            .list_assigned()
            .map_err(describe)?
            .into_iter()
            .find(|t| {
                t.id.external_id == id_or_key || t.display_key.eq_ignore_ascii_case(id_or_key)
            })
            .ok_or_else(|| format!("task `{id_or_key}` is not in your assigned list")),
        Err(e) => Err(describe(e)),
    }
}

/// What a brief is built around beyond the one task it is named after,
/// already listed.
enum BriefShape {
    /// A task's sub-tasks, for the agent coordinating them.
    Children(String),
    /// Tasks the user picked together, for the agent coordinating them.
    Picked(String),
    /// Several tasks one agent works on, each in worktrees of its own: every
    /// key as one phrase, and each task with its branch and worktrees.
    Group { key: String, tasks: String },
}

/// The note an agent starts with: what an agent wrote, then what okena adds.
///
/// `hand_picked` says the user put these tasks together rather than a
/// coordinator or a parent, which is a different thing to tell an agent: they
/// share no parent, and nobody decided they cannot be verified apart. Its
/// `siblings` are `picked-sibling` lines rather than bare keys.
fn compose_note(
    written: Option<String>,
    also: &[String],
    siblings: &[String],
    hand_picked: bool,
    task: &okena_core::tasks::Task,
    prompts: &PromptRoots,
) -> Option<String> {
    let mut parts: Vec<String> = written
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .into_iter()
        .collect();
    if !also.is_empty() {
        parts.push(briefs::fragment(
            if hand_picked {
                "picked-group-note"
            } else {
                "group-note"
            },
            prompts,
            &Vars::from([("also", also.join(", "))]),
        ));
    }
    if !siblings.is_empty() {
        if hand_picked {
            // Already one `picked-sibling` line each, with its branch and
            // worktrees.
            parts.push(briefs::fragment(
                "picked-fan-out-note",
                prompts,
                &Vars::from([("siblings", siblings.join("\n"))]),
            ));
        } else {
            let parent = task
                .parent_key
                .clone()
                .unwrap_or_else(|| "the parent task".to_string());
            parts.push(briefs::fragment(
                "fan-out-note",
                prompts,
                &Vars::from([("parent", parent), ("siblings", siblings.join(", "))]),
            ));
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// A coordinator's sub-tasks, one `coordinate-child` partial each.
///
/// The first lines of a description ride along: enough to judge whether two
/// children touch the same thing, not so much that the list stops being one.
fn list_children(children: &[okena_core::tasks::Task], prompts: &PromptRoots) -> String {
    children
        .iter()
        .map(|c| {
            let summary = c
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(|d| format!("\n  {}", d.lines().take(3).collect::<Vec<_>>().join(" ")))
                .unwrap_or_default();
            briefs::fragment(
                "coordinate-child",
                prompts,
                &Vars::from([
                    ("key", c.display_key.clone()),
                    ("kind", c.kind.label().to_string()),
                    ("title", c.title.clone()),
                    ("summary", summary),
                ]),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Point a project at the task it was created for, and at any others it
/// covers too.
fn link_task(
    ws: &mut Workspace,
    project_id: &str,
    task_ref: &okena_core::tasks::TaskRef,
    also: &[okena_core::tasks::TaskRef],
) {
    if let Some(project) = ws.data.projects.iter_mut().find(|p| p.id == project_id) {
        project.task_ref = Some(task_ref.clone());
        project.also_tasks = also.to_vec();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pull the error string out of a result, failing loudly on `Ok`.
    fn err_of(r: ActionResult) -> String {
        match r {
            ActionResult::Err(e) => e,
            ActionResult::Ok(v) => panic!("expected an error, got Ok({v:?})"),
        }
    }

    #[test]
    fn one_agent_on_picked_tasks_is_told_every_other_one() {
        // The session is named after the first task; the brief is where the
        // agent learns it was handed the rest too.
        let task: okena_core::tasks::Task = serde_json::from_value(serde_json::json!({
            "id": { "provider": "linear", "external_id": "u1" },
            "display_key": "QBL-1", "title": "t", "state": "todo", "state_name": "Todo",
            "url": "http://x", "branch_name": "feat/qbl-1", "updated_at": "",
        }))
        .unwrap();
        let also = ["QBL-2".to_string(), "QBL-3".to_string()];
        let note = compose_note(None, &also, &[], true, &task, &Vec::new()).expect("a note");
        assert!(note.contains("QBL-2, QBL-3"), "got: {note}");
    }

    // These run with no profile initialized, so the credential store resolves
    // to "nothing stored" — which is exactly the state a fresh install is in.

    #[test]
    fn auth_status_lists_every_known_provider() {
        let ActionResult::Ok(Some(v)) = auth_status() else {
            panic!("auth_status should always succeed — it makes no network call");
        };
        // Decode through the shared wire type: this pins that what the daemon
        // emits is exactly what a client can parse.
        let decoded: TaskAuthStatusResponse =
            serde_json::from_value(v).expect("daemon output must decode as the shared type");
        assert_eq!(decoded.providers.len(), okena_tasks::KNOWN_PROVIDERS.len());
        let linear = decoded.provider("linear").expect("linear should be listed");
        assert_eq!(linear.display_name, "Linear");
        assert_eq!(linear.auth, TaskAuthState::Disconnected);
        let ado = decoded
            .provider("azure_devops")
            .expect("azure devops should be listed");
        assert_eq!(ado.display_name, "Azure DevOps");
        assert_eq!(ado.auth, TaskAuthState::Disconnected);
    }

    #[test]
    fn provider_calls_are_claimed_for_the_blocking_pool_and_nothing_else() {
        use crate::workspace::actions::execute::execute_task_provider_action;
        use okena_core::api::ActionRequest;
        // No credential stored, so these fail without touching the network —
        // what matters is that they are claimed at all.
        let get = ActionRequest::TaskGet {
            provider: "linear".into(),
            task_external_id: "QBL-1".into(),
        };
        assert!(execute_task_provider_action(&get).is_some());
        // Starting work creates worktrees, and disconnecting is a local write:
        // neither is a provider call the loop may run without the workspace.
        let start = ActionRequest::TaskStartWork {
            provider: "linear".into(),
            task_external_id: "QBL-1".into(),
            project_ids: vec!["p".into()],
            agent_root: None,
            branch: None,
            agent_command: None,
            note: None,
            coordinate: false,
            also: Vec::new(),
            siblings: Vec::new(),
            branches: Default::default(),
            hand_picked: false,
            context: Vec::new(),
        };
        assert!(execute_task_provider_action(&start).is_none());
        assert!(
            execute_task_provider_action(&ActionRequest::TasksDisconnect {
                provider: "linear".into()
            })
            .is_none()
        );
    }

    #[test]
    fn a_choice_the_provider_needs_is_a_result_not_an_error() {
        let ActionResult::Ok(Some(v)) = error_result(TaskError::NeedsChoice {
            message: "choose a team for the new task".into(),
        }) else {
            panic!("a needed choice must not come back as an error");
        };
        assert_eq!(v["needs_choice"], "choose a team for the new task");
        let failed = err_of(error_result(TaskError::Unauthorized { provider: "linear" }));
        assert!(failed.contains("rejected"), "got: {failed}");
    }

    #[test]
    fn task_edits_are_checked_before_any_network_call() {
        let task = || "QBL-1".to_string();
        let nothing = err_of(update("linear".into(), task(), None, None));
        assert!(nothing.contains("nothing to change"), "got: {nothing}");
        let blank = err_of(update("linear".into(), task(), Some("  ".into()), None));
        assert!(blank.contains("title"), "got: {blank}");
        let unknown = err_of(set_state(
            "linear".into(),
            task(),
            okena_core::tasks::TaskState::Unknown,
        ));
        assert!(unknown.contains("choose a state"), "got: {unknown}");
        let silent = err_of(comment("linear".into(), task(), " ".into()));
        assert!(silent.contains("body"), "got: {silent}");
    }

    #[test]
    fn empty_api_key_is_rejected_before_any_network_call() {
        // Whitespace-only must be caught too — otherwise it reaches Linear as a
        // valid-looking header and fails with a confusing 401 instead.
        assert!(err_of(connect_api_key("linear".into(), "   ".into(), None)).contains("empty"));
    }

    #[test]
    fn azure_devops_needs_an_organization_url_before_any_network_call() {
        let missing = err_of(connect_api_key("azure_devops".into(), "pat".into(), None));
        assert!(missing.contains("organization URL"), "got: {missing}");
        // Only Azure DevOps Services: a server install, or a typo'd host, is
        // refused before a token is sent anywhere.
        let elsewhere = err_of(connect_api_key(
            "azure_devops".into(),
            "pat".into(),
            Some("https://tfs.contoso.local/tfs".into()),
        ));
        assert!(
            elsewhere.contains("Azure DevOps Services"),
            "got: {elsewhere}"
        );
    }

    #[test]
    fn unknown_provider_is_an_error_not_a_silent_noop() {
        // A newer client asking an older daemon for a provider it lacks should
        // say so plainly rather than appearing to succeed.
        assert!(
            err_of(connect_api_key("jira".into(), "k".into(), None))
                .contains("unknown task provider")
        );
        assert!(err_of(list("jira".into())).contains("unknown task provider"));
    }

    #[test]
    fn listing_without_a_credential_reports_not_authenticated() {
        let e = err_of(list("linear".into()));
        assert!(
            e.contains("not authenticated"),
            "expected a not-authenticated message, got: {e}"
        );
    }

    #[test]
    fn credential_writes_refuse_when_no_profile_is_active() {
        // Every real run initializes a profile before serving actions, so this
        // path only fires on a misconfiguration — and it must say so rather
        // than reporting a success that wrote nothing to disk.
        let e = err_of(disconnect("linear".into()));
        assert!(
            e.contains("no active profile"),
            "expected the missing-profile reason to surface, got: {e}"
        );
    }
}

#[cfg(test)]
mod agent_root_tests {
    use super::resolve_agent_root;
    use crate::workspace::persistence::AppSettings;

    fn settings_with_root(root: Option<&str>) -> AppSettings {
        let mut s = AppSettings::default();
        s.harness.agent_root = root.map(str::to_string);
        s
    }

    #[test]
    fn explicit_argument_wins() {
        let s = settings_with_root(Some("/configured"));
        assert_eq!(
            resolve_agent_root(Some("/explicit".into()), &s, "/Users/me/p/repo").as_deref(),
            Some("/explicit")
        );
    }

    #[test]
    fn falls_back_to_the_configured_root() {
        let s = settings_with_root(Some("/configured"));
        assert_eq!(
            resolve_agent_root(None, &s, "/Users/me/p/repo").as_deref(),
            Some("/configured")
        );
    }

    #[test]
    fn falls_back_to_the_projects_parent_directory() {
        // The `~/p/<repo>` layout: the agent runs one level above the repo so
        // it can see every sibling worktree.
        let s = settings_with_root(None);
        assert_eq!(
            resolve_agent_root(None, &s, "/Users/me/p/repo").as_deref(),
            Some("/Users/me/p")
        );
    }

    #[test]
    fn blank_values_are_treated_as_unset() {
        // A whitespace-only setting would otherwise become the agent's cwd.
        let s = settings_with_root(Some("   "));
        assert_eq!(
            resolve_agent_root(Some("  ".into()), &s, "/Users/me/p/repo").as_deref(),
            Some("/Users/me/p")
        );
    }

    #[test]
    fn a_root_level_path_has_no_parent_fallback() {
        let s = settings_with_root(None);
        assert_eq!(resolve_agent_root(None, &s, "/"), None);
    }
}

#[cfg(test)]
pub(super) mod agent_shell_tests {
    use super::agent_shell;
    use crate::workspace::persistence::AppSettings;
    use okena_core::tasks::{Task, TaskId, TaskState};
    use okena_terminal::shell_config::ShellType;

    pub(super) fn task() -> Task {
        Task {
            id: TaskId::new("linear", "uuid-1"),
            display_key: "LIN-42".into(),
            title: "Ship the harness".into(),
            description: None,
            state: TaskState::Todo,
            state_name: "Todo".into(),
            url: "https://linear.app/x/issue/LIN-42".into(),
            branch_name: "chore/lin-42-ship-the-harness".into(),
            updated_at: "2026-09-02T00:00:00Z".into(),
            kind: okena_core::tasks::TaskKind::Task,
            parent_id: None,
            parent_key: None,
            labels: Vec::new(),
            groups: Vec::new(),
        }
    }

    #[test]
    fn no_agent_configured_means_no_launch() {
        // Starting work must not spawn an AI agent unless asked to.
        let s = AppSettings::default();
        assert!(
            agent_shell(&s, None, &task(), "b", &[], None, None, &Vec::new(), &[], true).is_none()
        );
    }

    #[test]
    fn blank_command_is_treated_as_unset() {
        let mut s = AppSettings::default();
        s.harness.agent_command = Some("   ".into());
        assert!(
            agent_shell(&s, None, &task(), "b", &[], None, None, &Vec::new(), &[], true).is_none()
        );
    }

    /// `args` of a custom shell, or a panic.
    pub(super) fn custom_args(shell: Option<ShellType>) -> Vec<String> {
        match shell.expect("configured") {
            ShellType::Custom { args, .. } => args,
            other => panic!("expected a custom shell, got {other:?}"),
        }
    }

    /// Settings with a fixed MCP flag, so where it lands in argv can be
    /// asserted without a profile directory to write its config into.
    pub(super) fn settings_with_mcp_marker() -> AppSettings {
        let mut s = AppSettings::default();
        s.harness.agent_mcp_args = Some(vec!["--mcp-config".into(), "marker".into()]);
        s
    }

    #[test]
    fn skip_permissions_goes_between_the_session_id_and_the_brief() {
        let mut s = settings_with_mcp_marker();
        s.harness.agent_command = Some("claude".into());
        s.harness.agents.claude.skip_permissions = true;
        let args = custom_args(agent_shell(
            &s,
            None,
            &task(),
            "b1",
            &[],
            None,
            None,
            &Vec::new(),
            &[],
            true,
        ));
        assert_eq!(args[0], "--session-id");
        assert!(uuid::Uuid::parse_str(&args[1]).is_ok(), "{args:?}");
        assert_eq!(args[2], "--dangerously-skip-permissions");
        // The task-start brief, not something in its place.
        assert!(args[3].contains("LIN-42"), "{args:?}");
        assert!(args[3].contains("okena_test_plan"), "{args:?}");
        assert_eq!(args[4..6], ["--mcp-config", "marker"]);
    }

    #[test]
    fn a_single_worktree_is_where_its_agent_runs() {
        // The agent changes that checkout; rooting it above would make every
        // relative path it reads point at the wrong place.
        assert_eq!(
            super::session_root(&["/wt/qbl-1".to_string()], Some("/p".to_string())),
            Some("/wt/qbl-1".to_string())
        );
    }

    #[test]
    fn several_worktrees_put_the_agent_above_them() {
        let paths = ["/wt/a".to_string(), "/wt/b".to_string()];
        assert_eq!(
            super::session_root(&paths, Some("/wt".to_string())),
            Some("/wt".to_string())
        );
        assert_eq!(super::session_root(&paths, None), None);
    }

    #[test]
    fn a_command_with_no_configured_args_gets_the_task_start_brief() {
        // Agents used to be launched having been told nothing at all, opening
        // in a worktree with no idea what it was for. The template fills that
        // gap.
        let mut s = AppSettings::default();
        s.harness.agent_command = Some("codex".into());
        match agent_shell(&s, None, &task(), "b1", &[], None, None, &Vec::new(), &[], true)
            .expect("configured")
        {
            ShellType::Custom { path, args } => {
                assert_eq!(path, "codex");
                let brief = args.first().expect("a brief was passed");
                for needle in ["LIN-42", "Ship the harness", "b1"] {
                    assert!(
                        brief.contains(needle),
                        "brief is missing {needle}:\n{brief}"
                    );
                }
            }
            other => panic!("expected a custom shell, got {other:?}"),
        }
    }

    #[test]
    fn a_started_task_is_told_to_plan_and_verify_its_work() {
        // With no per-project configuration: the built-in carries it.
        let brief =
            super::task_brief(&task(), "b1", &[], None, None, &[], false, &Vec::new(), true);
        for needle in [
            "okena_test_plan",
            "okena_test_step_start",
            "okena_test_step_result",
            "okena_test_run_finish",
        ] {
            assert!(
                brief.contains(needle),
                "brief is missing {needle}:\n{brief}"
            );
        }
        // Once, from the host brief, not again from the embedded one.
        assert_eq!(brief.matches("okena_report_status").count(), 1, "{brief}");
        assert!(!brief.contains("{verify}"), "{brief}");
    }

    #[test]
    fn a_stores_verify_template_replaces_only_the_verify_instruction() {
        let dir = std::env::temp_dir().join(format!("okena-task-verify-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("templates")).expect("mkdir");
        std::fs::write(
            dir.join("templates/task-verify.md"),
            "---\nfor: task-verify\n---\nVerify {key} on staging.\n",
        )
        .expect("write");
        let root = vec![("acme".to_string(), dir.clone())];
        let brief = super::task_brief(&task(), "b1", &[], None, None, &[], false, &root, true);
        std::fs::remove_dir_all(&dir).ok();
        assert!(brief.contains("Verify LIN-42 on staging."), "{brief}");
        assert!(!brief.contains("okena_test_plan"), "{brief}");
        // The store did not override task-start, so the rest is okena's.
        assert!(
            brief.contains("The worktrees are already created"),
            "{brief}"
        );
        assert!(brief.contains("okena_report_status"), "{brief}");
    }

    #[test]
    fn options_and_extra_args_never_displace_the_brief() {
        // Setting them used to launch the agent without its task.
        let mut s = settings_with_mcp_marker();
        s.harness.agents.codex.approvals = Some(okena_workspace::settings::CodexApprovals::Bypass);
        s.harness.agents.codex.extra_args = vec!["--search".into()];
        let args = custom_args(agent_shell(
            &s,
            Some("codex"),
            &task(),
            "b1",
            &[],
            None,
            None,
            &Vec::new(),
            &[],
            true,
        ));
        assert_eq!(
            args[..2],
            ["--dangerously-bypass-approvals-and-sandbox", "--search"]
        );
        assert!(args[2].contains("LIN-42"), "{args:?}");
        assert_eq!(args[3..5], ["--mcp-config", "marker"]);
    }

    #[test]
    fn picking_codex_carries_none_of_claudes_options() {
        let mut s = settings_with_mcp_marker();
        s.harness.agent_command = Some("claude".into());
        s.harness.agents.claude.skip_permissions = true;
        s.harness.agents.claude.permission_mode =
            Some(okena_workspace::settings::ClaudePermissionMode::Plan);
        s.harness.agents.claude.extra_args = vec!["--verbose".into()];
        let args = custom_args(agent_shell(
            &s,
            Some("codex"),
            &task(),
            "b1",
            &[],
            None,
            None,
            &Vec::new(),
            &[],
            true,
        ));
        assert!(args[0].contains("LIN-42"), "brief comes first: {args:?}");
        // Then only okena's wiring (MCP flags, Codex's notify hook).
        assert_eq!(args[1..3], ["--mcp-config", "marker"]);
        for claude_only in [
            "--dangerously-skip-permissions",
            "--permission-mode",
            "--verbose",
        ] {
            assert!(!args.iter().any(|a| a == claude_only), "{args:?}");
        }
    }

    #[test]
    fn with_nothing_set_every_route_launches_as_before() {
        // Session id, prompt, MCP flags — nothing else.
        let mut s = settings_with_mcp_marker();
        s.harness.agent_command = Some("claude".into());
        let routes = [
            custom_args(agent_shell(
                &s,
                None,
                &task(),
                "b1",
                &[],
                None,
                None,
                &Vec::new(),
                &[],
                true,
            )),
            custom_args(super::custom_agent_shell(
                &s,
                None,
                "goal",
                &Default::default(),
            )),
            custom_args(super::super::specs::spec_agent_shell(
                &s,
                None,
                "draft",
                &Default::default(),
            )),
        ];
        for args in routes {
            assert_eq!(args[0], "--session-id", "{args:?}");
            assert!(
                !args[2].starts_with("--"),
                "prompt follows the id: {args:?}"
            );
            assert_eq!(args[3..5], ["--mcp-config", "marker"], "{args:?}");
        }
    }

    #[test]
    fn every_route_hands_a_long_brief_over_in_a_file() {
        use okena_terminal::backend::TerminalLaunchPlan;
        use okena_terminal::brief_file;
        use okena_terminal::session_backend::{ResolvedBackend, SessionCommand};
        let dir = tempfile::tempdir().unwrap();
        super::super::briefs::test_briefs_dir::set(Some(dir.path().to_path_buf()));
        let mut s = settings_with_mcp_marker();
        s.harness.agent_command = Some("claude".into());
        // Past tmux's ~16 KB message limit on its own, with what breaks quoting.
        let long = "Don't \"quote\" $HOME `id` — ünïcødé\n".repeat(500);
        assert!(long.len() > 17_000);
        let group = super::BriefShape::Group {
            key: "LIN-1, LIN-2".into(),
            tasks: "- LIN-1\n- LIN-2".into(),
        };
        let picked = super::BriefShape::Picked("- LIN-42\n- LIN-7".into());
        let routes = [
            (
                "task start",
                agent_shell(
                    &s, None, &task(), "b1", &[], Some(&long), None, &Vec::new(), &[], true,
                ),
            ),
            (
                "multi-task start",
                agent_shell(
                    &s,
                    None,
                    &task(),
                    "b1",
                    &[],
                    Some(&long),
                    Some(&group),
                    &Vec::new(),
                    &[],
                    true,
                ),
            ),
            (
                "coordinator",
                agent_shell(
                    &s,
                    None,
                    &task(),
                    "b1",
                    &[],
                    Some(&long),
                    Some(&picked),
                    &Vec::new(),
                    &[],
                    true,
                ),
            ),
            (
                "custom session",
                super::custom_agent_shell(&s, None, &long, &Default::default()),
            ),
            // Spec drafts, knowledge drafts and doc refine all launch here.
            (
                "spec draft",
                super::super::specs::spec_agent_shell(&s, None, &long, &Default::default()),
            ),
        ];
        for (route, shell) in routes {
            let shell = shell.expect("an agent");
            let args = custom_args(Some(shell.clone()));
            // Named, then the reference where the brief went, then MCP.
            assert_eq!(args[0], "--session-id", "{route}: {args:?}");
            let file = brief_file::path_of(&args[2]).unwrap_or_else(|| panic!("{route}: {args:?}"));
            assert!(
                std::path::Path::new(file).starts_with(dir.path()),
                "{route}"
            );
            let written = std::fs::read_to_string(file).unwrap();
            assert!(written.contains(&long), "{route}: the file holds the brief");
            assert_eq!(args[3..5], ["--mcp-config", "marker"], "{route}");
            assert!(args.iter().all(|a| a.len() < 1_000), "{route}: {args:?}");

            // What tmux is handed stays far under its limit.
            let plan = brief_file::resolve_plan(&TerminalLaunchPlan::for_shell(shell.clone()))
                .expect("a reference to resolve");
            let ShellType::Custom {
                path,
                args: wrapped,
            } = &plan.route
            else {
                panic!("{route}: {:?}", plan.route)
            };
            let (_, tmux) = ResolvedBackend::Tmux
                .build_command_with_custom(
                    "tm-12345678",
                    "/Users/someone/p/okena",
                    Some(SessionCommand::Program {
                        program: path,
                        args: wrapped,
                    }),
                    &[],
                )
                .expect("tmux command");
            let size: usize = tmux.iter().map(String::len).sum();
            assert!(size < 4_096, "{route}: {size} bytes");

            // Restart finds the conversation and sends no prompt.
            use super::super::agent_resume as resume;
            let id = resume::session_id_of(&args).expect("named").to_string();
            assert_eq!(
                resume::resume_args("claude", &args),
                Some(vec!["--resume".to_string(), id])
            );
            assert!(resume::resumable(&shell, true), "{route}");
        }

        // A short brief with no context: same argv shape, and the file holds
        // exactly the brief.
        let args = custom_args(super::custom_agent_shell(
            &s,
            None,
            "goal",
            &Default::default(),
        ));
        assert_eq!(args.len(), 5, "{args:?}");
        let file = brief_file::path_of(&args[2]).expect("a reference");
        assert_eq!(std::fs::read_to_string(file).unwrap(), "goal");
        // Copilot takes it after `--prompt`, as it took the brief.
        let args = custom_args(super::super::specs::spec_agent_shell(
            &s,
            Some("copilot"),
            "draft",
            &Default::default(),
        ));
        let at = args.iter().position(|a| a == "--prompt").expect("--prompt");
        assert_eq!(
            std::fs::read_to_string(brief_file::path_of(&args[at + 1]).unwrap()).unwrap(),
            "draft"
        );
        super::super::briefs::test_briefs_dir::set(None);
    }

    #[test]
    fn custom_and_draft_sessions_carry_options_with_their_prompt() {
        // Spec drafts, knowledge drafts, doc refine, project scans and links
        // all launch through `spec_agent_shell`.
        let mut s = settings_with_mcp_marker();
        s.harness.agent_command = Some("claude".into());
        s.harness.agents.claude.permission_mode =
            Some(okena_workspace::settings::ClaudePermissionMode::AcceptEdits);
        s.harness.agents.claude.extra_args = vec!["--verbose".into()];
        for (args, prompt) in [
            (
                custom_args(super::custom_agent_shell(
                    &s,
                    None,
                    "goal",
                    &Default::default(),
                )),
                "goal",
            ),
            (
                custom_args(super::super::specs::spec_agent_shell(
                    &s,
                    None,
                    "draft",
                    &Default::default(),
                )),
                "draft",
            ),
        ] {
            assert_eq!(
                args[2..6],
                ["--permission-mode", "acceptEdits", "--verbose", prompt],
                "{args:?}"
            );
            assert_eq!(args[6..8], ["--mcp-config", "marker"]);
        }
        // Copilot's prompt is a flag, and still arrives after the options.
        s.harness.agents.copilot.mode = Some(okena_workspace::settings::CopilotMode::Autopilot);
        let args = custom_args(super::super::specs::spec_agent_shell(
            &s,
            Some("copilot"),
            "draft",
            &Default::default(),
        ));
        assert_eq!(args[..4], ["--mode", "autopilot", "--prompt", "draft"]);
    }

    #[test]
    fn the_brief_names_the_repos_the_agent_was_given() {
        // A multi-repo task opens the agent above the worktrees, where "the
        // project" is ambiguous until they are listed.
        let mut s = AppSettings::default();
        s.harness.agent_command = Some("codex".into());
        let given = [("okena".to_string(), "/p/okena".to_string())];
        match agent_shell(&s, None, &task(), "b1", &given, None, None, &Vec::new(), &[], true)
            .expect("configured")
        {
            ShellType::Custom { args, .. } => {
                let brief = args.first().expect("a brief was passed");
                assert!(brief.contains("- okena (/p/okena)"), "{brief}");
            }
            other => panic!("expected a custom shell, got {other:?}"),
        }
    }

    #[test]
    fn a_coordinator_over_picked_tasks_is_briefed_with_every_one() {
        let mut s = AppSettings::default();
        s.harness.agent_command = Some("codex".into());
        let picked = super::BriefShape::Picked(
            "- LIN-42 (Task): Ship the harness\n- LIN-7 (Defect): Fix the login".into(),
        );
        match agent_shell(&s, None, &task(), "b1", &[], None, Some(&picked), &Vec::new(), &[], true)
            .expect("configured")
        {
            ShellType::Custom { args, .. } => {
                let brief = args.first().expect("a brief was passed");
                for needle in [
                    "LIN-42",
                    "LIN-7",
                    "Fix the login",
                    "okena_start_work",
                    "no worktree of your own",
                ] {
                    assert!(
                        brief.contains(needle),
                        "brief is missing {needle}:\n{brief}"
                    );
                }
                // It has no branch, and is told of none.
                assert!(!brief.contains("b1"), "{brief}");
                // Not the sub-task brief: these share no parent.
                assert!(!brief.contains("sub-tasks"), "{brief}");
                // Verifying is the work agents' job, not the coordinator's.
                assert!(!brief.contains("okena_test_plan"), "{brief}");
            }
            other => panic!("expected a custom shell, got {other:?}"),
        }
    }

    #[test]
    fn picked_tasks_are_not_told_they_share_a_parent() {
        let fanned = super::compose_note(None, &[], &["LIN-7".into()], true, &task(), &Vec::new())
            .expect("a note");
        assert!(fanned.contains("LIN-7"), "{fanned}");
        assert!(!fanned.contains("parent"), "{fanned}");

        let grouped = super::compose_note(None, &["LIN-7".into()], &[], true, &task(), &Vec::new())
            .expect("a note");
        assert!(grouped.contains("LIN-7"), "{grouped}");
        assert!(!grouped.contains("verified apart"), "{grouped}");
    }

    #[test]
    fn a_coordinator_is_given_repos_not_worktrees() {
        let picked = super::BriefShape::Picked("- LIN-42 (Task): Ship the harness".into());
        let given = [("okena".to_string(), "/p/okena".to_string())];
        let brief = super::task_brief(
            &task(),
            "",
            &given,
            None,
            Some(&picked),
            &[],
            false,
            &Vec::new(),
            true,
        );
        assert!(
            brief.contains("Projects you were given:\n- okena (/p/okena)"),
            "{brief}"
        );
        assert!(!brief.contains("Worktrees you were given"), "{brief}");
    }

    #[test]
    fn a_plain_directory_gets_no_worktree() {
        // The bug: a project pointed at a projects root — a directory holding
        // repositories, not a checkout — was still driven into
        // `git worktree add`, which exits 128 outside a repository.
        let root = std::env::temp_dir().join("okena-worktree-skip-test");
        std::fs::create_dir_all(&root).expect("a temp directory");
        let reason = super::worktree_skip_reason(&root.to_string_lossy());
        std::fs::remove_dir_all(&root).ok();
        assert!(reason.is_some(), "a plain directory is not a checkout");

        // A real checkout, and a subdirectory of one, are both cuttable.
        let checkout = env!("CARGO_MANIFEST_DIR");
        assert!(super::worktree_skip_reason(checkout).is_none());
        assert!(super::worktree_skip_reason(&format!("{checkout}/src")).is_none());
    }

    #[test]
    fn only_a_worktree_that_failed_refuses_the_start() {
        let one = vec![serde_json::json!({ "project": "okena" })];
        // Meant to cut one and could not: the start cannot go ahead.
        assert!(super::no_worktrees_is_fatal(false, &[], &one));
        // Nothing was ever meant to be cut, so there is nothing to refuse.
        assert!(!super::no_worktrees_is_fatal(false, &[], &[]));
        // Some were cut, so a failure elsewhere is reported, not fatal.
        assert!(!super::no_worktrees_is_fatal(false, &one, &one));
        // A coordinator never has one of its own.
        assert!(!super::no_worktrees_is_fatal(true, &[], &one));
    }

    #[test]
    fn with_no_worktrees_the_brief_names_repos() {
        // Briefed as worktrees, the agent would take a shared checkout for its
        // own task branch and commit there.
        let given = [("okena".to_string(), "/p/okena".to_string())];
        let repos = super::task_brief(
            &task(),
            "b1",
            &given,
            None,
            None,
            &[],
            false,
            &Vec::new(),
            false,
        );
        assert!(
            repos.contains("Projects you were given:\n- okena (/p/okena)"),
            "{repos}"
        );
        assert!(!repos.contains("Worktrees you were given"), "{repos}");

        let cut = super::task_brief(
            &task(),
            "b1",
            &given,
            None,
            None,
            &[],
            false,
            &Vec::new(),
            true,
        );
        assert!(cut.contains("Worktrees you were given"), "{cut}");
    }

    #[test]
    fn one_agent_on_several_tasks_is_told_whose_worktree_is_whose() {
        let first = task();
        let mut other = task();
        other.display_key = "LIN-7".into();
        other.title = "Fix the login".into();
        other.url = "https://linear.app/x/issue/LIN-7".into();
        other.description = Some("Login fails on Safari.".into());
        let listed = [
            (
                &first,
                "chore/lin-42-ship-the-harness",
                vec![("okena".to_string(), "/wt/lin-42".to_string())],
            ),
            (
                &other,
                "fix/lin-7-fix-the-login",
                vec![("okena".to_string(), "/wt/lin-7".to_string())],
            ),
        ];
        let shape = super::BriefShape::Group {
            key: super::join_keys(&["LIN-42", "LIN-7"]),
            tasks: super::list_group(&listed, &Vec::new()),
        };
        let brief = super::task_brief(
            &first,
            "chore/lin-42-ship-the-harness",
            &[],
            None,
            Some(&shape),
            &[],
            false,
            &Vec::new(),
            true,
        );
        for needle in [
            "Work on LIN-42 and LIN-7, together.",
            "## LIN-42: Ship the harness",
            "Branch: chore/lin-42-ship-the-harness",
            "- okena (/wt/lin-42)",
            "## LIN-7: Fix the login",
            "Branch: fix/lin-7-fix-the-login",
            "- okena (/wt/lin-7)",
            "Login fails on Safari.",
            "that task's worktrees",
            "okena_test_plan",
        ] {
            assert!(
                brief.contains(needle),
                "brief is missing {needle}:\n{brief}"
            );
        }
        // Each section names only its own worktree.
        let lin42 = &brief[brief.find("## LIN-42").unwrap()..brief.find("## LIN-7").unwrap()];
        assert!(!lin42.contains("/wt/lin-7"), "{brief}");
        assert!(!brief.contains("{verify}"), "{brief}");
        assert_eq!(brief.matches("okena_report_status").count(), 1, "{brief}");
    }

    #[test]
    fn a_picked_sibling_is_named_with_its_branch_and_worktrees() {
        let line = super::picked_sibling(
            "LIN-7",
            "fix/lin-7",
            &["/wt/okena-fix-lin-7".into(), "/wt/web-fix-lin-7".into()],
            &Vec::new(),
        );
        let note = super::compose_note(None, &[], &[line], true, &task(), &Vec::new()).expect("a note");
        assert!(
            note.contains("- LIN-7 on `fix/lin-7`: /wt/okena-fix-lin-7, /wt/web-fix-lin-7"),
            "{note}"
        );
        assert!(!note.contains("parent"), "{note}");
    }

    #[test]
    fn keys_read_as_one_phrase() {
        assert_eq!(super::join_keys(&[]), "");
        assert_eq!(super::join_keys(&["A"]), "A");
        assert_eq!(super::join_keys(&["A", "B"]), "A and B");
        assert_eq!(super::join_keys(&["A", "B", "C"]), "A, B and C");
    }

    #[test]
    fn a_siblings_worktree_is_where_its_own_start_puts_it() {
        let dir = std::env::temp_dir().join(format!("okena-sibling-path-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let project = dir.to_string_lossy().into_owned();
        let s = AppSettings::default();
        let (root, subdir) = okena_git::resolve_git_root_and_subdir(&dir);
        let (_, expected) =
            okena_git::compute_target_paths(&root, &subdir, &s.worktree.path_template, "fix/lin-7");
        std::fs::remove_dir_all(&dir).ok();
        let got = super::worktree_path_for(&project, "fix/lin-7", &s);
        assert_eq!(got, expected);
        assert!(got.contains("fix-lin-7"), "{got}");
    }

    #[test]
    fn a_coordinator_runs_in_its_one_project_or_above_several() {
        let mut s = AppSettings::default();
        s.harness.agent_root = Some("/configured".into());
        let one = ["/p/okena".to_string()];
        let two = ["/p/okena".to_string(), "/p/web".to_string()];
        // One project: its own checkout, whatever the configured root says.
        assert_eq!(
            super::coordinator_root(None, &one, &s).as_deref(),
            Some("/p/okena")
        );
        assert_eq!(
            super::coordinator_root(None, &two, &s).as_deref(),
            Some("/configured")
        );
        s.harness.agent_root = None;
        assert_eq!(
            super::coordinator_root(None, &two, &s).as_deref(),
            Some("/p")
        );
        assert_eq!(
            super::coordinator_root(Some("/explicit".into()), &one, &s).as_deref(),
            Some("/explicit")
        );
    }

    #[test]
    fn a_coordinators_group_keeps_the_sub_task_wording() {
        let grouped = super::compose_note(None, &["LIN-7".into()], &[], false, &task(), &Vec::new())
            .expect("a note");
        assert!(grouped.contains("verified apart"), "{grouped}");
    }
}

// ─── Agent reporting ─────────────────────────────────────────────────────────

/// Current wall-clock in Unix millis, or 0 if the clock is before the epoch.
pub(super) fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Record an asset an agent produced.
///
/// Appended as reported. Branches and PRs okena detects for the same work are
/// not stored at all; the two are merged when the session's list is read.
#[allow(clippy::too_many_arguments)]
pub(super) fn register_asset(
    ws: &mut Workspace,
    project_id: String,
    kind: String,
    title: String,
    url: Option<String>,
    project: Option<String>,
    branch: Option<String>,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let title = title.trim().to_string();
    if title.is_empty() {
        return ActionResult::Err("asset title is empty".into());
    }
    // An unrecognized kind decodes to `Other` rather than failing: an agent
    // reporting something this build doesn't model should still be visible.
    let kind: okena_core::harness::AgentAssetKind =
        serde_json::from_value(serde_json::Value::String(kind))
            .unwrap_or(okena_core::harness::AgentAssetKind::Other);

    let asset = okena_core::harness::AgentAsset {
        kind,
        title,
        url: url.filter(|u| !u.trim().is_empty()),
        project: project.filter(|p| !p.trim().is_empty()),
        branch: branch
            .map(|b| b.trim().to_string())
            .filter(|b| !b.is_empty()),
        created_at: now_millis(),
        task: None,
    };

    let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == project_id) else {
        return ActionResult::Err(format!("project not found: {project_id}"));
    };
    let state = p.agent.get_or_insert_with(Default::default);
    // Stored as reported. Two rows about the same thing are matched when the
    // list is built, where the match can change later; merging here could not.
    state.assets.push(asset);
    let count = state.assets.len();
    ws.notify_data(cx);

    ActionResult::Ok(Some(serde_json::json!({
        "project_id": project_id,
        "asset_count": count,
    })))
}

/// Set the status an agent reports for its session.
pub(super) fn report_status(
    ws: &mut Workspace,
    project_id: String,
    status: String,
    reported: Option<okena_core::harness::AgentState>,
    question: Option<String>,
    suggestions: Vec<okena_core::harness::AgentSuggestion>,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == project_id) else {
        return ActionResult::Err(format!("project not found: {project_id}"));
    };
    let agent = p.agent.get_or_insert_with(Default::default);
    apply_report(agent, &status, reported, question, suggestions);
    agent.reported_at = Some(now_millis());
    ws.notify_data(cx);
    ActionResult::Ok(Some(serde_json::json!({ "project_id": project_id })))
}

/// Replace what an agent last said about itself.
///
/// A report replaces the previous one whole. Leaving an old question or old
/// suggestions in place when the agent reports that it is working again would
/// keep offering you answers to a question it has stopped asking.
fn apply_report(
    agent: &mut okena_core::harness::AgentSessionState,
    status: &str,
    reported: Option<okena_core::harness::AgentState>,
    question: Option<String>,
    suggestions: Vec<okena_core::harness::AgentSuggestion>,
) {
    let status = status.trim();
    // An empty status clears it rather than displaying a blank line.
    agent.status = (!status.is_empty()).then(|| status.to_string());
    agent.state = reported;
    agent.question = question
        .map(|q| q.trim().to_string())
        .filter(|q| !q.is_empty());
    agent.suggestions = suggestions
        .into_iter()
        .filter(|s| !s.label.trim().is_empty() && !s.instruction.trim().is_empty())
        .collect();
}

/// Type `text` into a session's agent and submit it.
///
/// The Enter goes separately, a moment after the text. A terminal agent reads
/// a burst of characters arriving together as a paste, and a carriage return
/// inside a paste is a newline in the prompt rather than a submit — sent in
/// one write, the instruction would sit typed but unsent.
pub(super) fn send_instruction(
    ws: &mut Workspace,
    project_id: String,
    text: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let text = text.trim().to_string();
    if text.is_empty() {
        return ActionResult::Err("say what to tell the agent first".into());
    }
    let Some(project) = ws.project(&project_id) else {
        return ActionResult::Err(format!("project not found: {project_id}"));
    };
    // The agent's own pane: the session's other panes are shells, and typing
    // an instruction into one would run it as a command.
    let Some(layout) = project.layout.as_ref() else {
        return ActionResult::Err("this session has no terminal to send to".into());
    };
    let terminal_id = if layout.agent_terminal_path().is_some() {
        match layout.agent_terminal_id() {
            Some(id) => id,
            None => return ActionResult::Err("the agent is stopped — start it first".into()),
        }
    } else {
        // A session started without an agent: whatever it runs is visible.
        match layout
            .visible_terminal_id()
            .or_else(|| layout.collect_terminal_ids().into_iter().next())
        {
            Some(id) => id,
            None => return ActionResult::Err("this session has no terminal to send to".into()),
        }
    };
    let Some(terminal) = super::ensure_terminal(&terminal_id, terminals, backend, ws, settings)
    else {
        return ActionResult::Err(format!("terminal not found: {terminal_id}"));
    };
    terminal.send_input(&text);
    let submit = terminal.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(SUBMIT_DELAY_MS));
        submit.send_bytes(b"\r");
    });

    // Answered: it is no longer waiting on you. Without this the attention
    // flag would stay until the agent next reported, as though the answer had
    // not arrived. The status line stays — it is still the last thing it said.
    if let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == project_id)
        && let Some(agent) = p.agent.as_mut()
    {
        agent.state = Some(okena_core::harness::AgentState::Working);
        agent.question = None;
        agent.suggestions.clear();
    }
    ws.notify_data(cx);
    ActionResult::Ok(Some(
        serde_json::json!({ "project_id": project_id, "terminal_id": terminal_id }),
    ))
}

/// How long after the text the Enter follows. Long enough that the two do not
/// arrive as one burst, short enough to feel like a single action.
const SUBMIT_DELAY_MS: u64 = 150;

#[cfg(test)]
mod report_tests {
    use super::apply_report;
    use okena_core::harness::{AgentSessionState, AgentState, AgentSuggestion};

    fn suggestion(label: &str) -> AgentSuggestion {
        AgentSuggestion {
            label: label.into(),
            instruction: format!("please {label}"),
        }
    }

    #[test]
    fn a_report_carries_why_the_agent_stopped() {
        let mut agent = AgentSessionState::default();
        apply_report(
            &mut agent,
            "changes ready",
            Some(AgentState::ReadyForReview),
            None,
            vec![suggestion("commit and open a PR")],
        );
        assert_eq!(agent.state, Some(AgentState::ReadyForReview));
        assert_eq!(agent.suggestions.len(), 1);
    }

    #[test]
    fn a_new_report_drops_the_old_question_and_suggestions() {
        // Otherwise okena keeps offering answers to a question the agent has
        // stopped asking.
        let mut agent = AgentSessionState::default();
        apply_report(
            &mut agent,
            "which db?",
            Some(AgentState::NeedsInput),
            Some("Postgres or SQLite?".into()),
            vec![suggestion("use postgres")],
        );
        apply_report(
            &mut agent,
            "migrating",
            Some(AgentState::Working),
            None,
            Vec::new(),
        );
        assert_eq!(agent.question, None);
        assert!(agent.suggestions.is_empty());
        assert_eq!(agent.status.as_deref(), Some("migrating"));
    }

    #[test]
    fn a_suggestion_with_nothing_to_send_is_dropped() {
        // A button that types nothing into the agent is a button that does
        // nothing.
        let mut agent = AgentSessionState::default();
        apply_report(
            &mut agent,
            "s",
            Some(AgentState::ReadyForReview),
            Some("   ".into()),
            vec![
                AgentSuggestion {
                    label: "Commit".into(),
                    instruction: "  ".into(),
                },
                suggestion("open a PR"),
            ],
        );
        assert_eq!(agent.suggestions.len(), 1);
        assert_eq!(agent.question, None, "a blank question is no question");
    }
}

#[cfg(test)]
mod agent_override_tests {
    use super::agent_shell;
    use super::agent_shell_tests::task;
    use crate::workspace::persistence::AppSettings;
    use okena_terminal::shell_config::ShellType;

    #[test]
    fn an_explicit_command_overrides_the_configured_one() {
        let mut s = AppSettings::default();
        s.harness.agent_command = Some("claude".into());
        match agent_shell(&s, Some("codex"), &task(), "b", &[], None, None, &Vec::new(), &[], true)
            .expect("override applies")
        {
            ShellType::Custom { path, .. } => assert_eq!(path, "codex"),
            other => panic!("expected a custom shell, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_override_means_no_agent_despite_a_default() {
        // "Worktrees only" must be expressible even when a default is set.
        let mut s = AppSettings::default();
        s.harness.agent_command = Some("claude".into());
        assert!(
            agent_shell(&s, Some(""), &task(), "b", &[], None, None, &Vec::new(), &[], true)
                .is_none()
        );
        assert!(
            agent_shell(&s, Some("   "), &task(), "b", &[], None, None, &Vec::new(), &[], true)
                .is_none()
        );
    }

    #[test]
    fn no_override_falls_back_to_the_setting() {
        let mut s = AppSettings::default();
        s.harness.agent_command = Some("claude".into());
        match agent_shell(&s, None, &task(), "b", &[], None, None, &Vec::new(), &[], true)
            .expect("falls back")
        {
            ShellType::Custom { path, .. } => assert_eq!(path, "claude"),
            other => panic!("expected a custom shell, got {other:?}"),
        }
    }
}

/// Start a free-form agent session the user configured themselves.
///
/// Shares every mechanism with the task and spec routes — a session project, an
/// agent launched with okena's MCP wired in — and differs only in where the
/// brief comes from. Kept here beside `start_work` so the three stay in step.
#[allow(clippy::too_many_arguments)]
pub(super) fn start_custom_session(
    ws: &mut Workspace,
    window_id: WindowId,
    goal: String,
    name: String,
    root: String,
    project_ids: Vec<String>,
    agent_command: Option<String>,
    task_draft: Option<String>,
    task: Option<okena_core::tasks::TaskRef>,
    purpose: Option<okena_core::harness::AgentPurpose>,
    context_refs: Vec<okena_core::context::ContextRef>,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let goal = goal.trim().to_string();
    if goal.is_empty() {
        return ActionResult::Err("describe what the agent should do first".into());
    }

    // Every selected project must exist before anything is created: pointing an
    // agent at a project that has since gone is worth saying, not ignoring.
    let mut context: Vec<(String, String)> = Vec::new();
    for id in &project_ids {
        match ws.project(id) {
            Some(p) => context.push((p.name.clone(), p.path.clone())),
            None => return ActionResult::Err(format!("project not found: {id}")),
        }
    }

    let Some(root) = resolve_session_root(&root, &context, settings) else {
        return ActionResult::Err(
            "pick a working directory, a project, or set a projects root in Settings → Harness"
                .into(),
        );
    };
    if !std::path::Path::new(&root).is_dir() {
        return ActionResult::Err(format!("working directory not found: {root}"));
    }

    let label = session_label(&name, &goal);
    let display = label.clone();
    // Resolved before the session exists, from this side's roots: a ref whose
    // item has gone since the user picked it is dropped, not trusted.
    let context_items =
        super::context::resolve_for_launch(&ws.data.projects, settings, &context_refs);

    let session_id = match ws.add_project(
        display.clone(),
        root.clone(),
        // With a terminal: this is where the agent runs.
        true,
        &settings.hooks,
        window_id,
        cx,
    ) {
        Ok(id) => id,
        Err(e) => return ActionResult::Err(format!("could not create the session: {e}")),
    };

    // Mark it before spawning, so it is recognizable as a session from the
    // first snapshot the client sees.
    if let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == session_id) {
        p.custom_session = Some(label.clone());
        // Both markers: it is a free-form session (so it gets no worktrees and
        // does not read as work in progress) that is nonetheless about a task,
        // so the task can list it and the agent's own MCP calls resolve it.
        p.task_ref = task;
        // Or about a task that does not exist yet, which the list shows as a
        // placeholder until the agent files it.
        p.task_draft = task_draft;
        // Which card started it, so that card — and only that one — lists it.
        p.agent_purpose = purpose;
        // What its own context lookups may see.
        p.context_projects = super::context::scope_projects(&project_ids, &context_items);
    }

    let command = super::agent_context::launch_command(settings, agent_command.as_deref());
    let install = super::agent_context::install(&command, &context_items);
    let brief = custom_brief(
        &goal,
        &context,
        &context_items,
        install.loaded(),
        &briefs::prompt_roots(&ws.data.projects, settings),
    );
    if let Some(shell) = custom_agent_shell(settings, agent_command.as_deref(), &brief, &install)
        && let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == session_id)
    {
        p.default_shell = Some(shell);
    }

    if let ActionResult::Err(e) =
        super::spawn_session_terminals(ws, &session_id, backend, terminals, settings, cx)
    {
        log::warn!("[agents] session terminal failed to spawn: {e}");
    }

    ws.notify_data(cx);

    ActionResult::Ok(Some(serde_json::json!({
        "project_id": session_id,
        "name": display,
        "root": root,
    })))
}

/// Where a custom session runs.
///
/// An explicit directory wins. Otherwise a single selected project runs in
/// itself, and several run at the configured projects root — an agent given
/// three repos needs a directory above all of them, the same reasoning
/// `start_work` uses for a multi-project task.
pub(super) fn resolve_session_root(
    root: &str,
    context: &[(String, String)],
    settings: &AppSettings,
) -> Option<String> {
    let explicit = root.trim();
    if !explicit.is_empty() {
        return Some(expand_home(explicit));
    }
    if context.len() == 1 {
        return Some(context[0].1.clone());
    }
    settings
        .harness
        .agent_root
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(expand_home)
        // With several projects and no root configured, fall back to the parent
        // of the first, which is right for the common `~/p/<repo>` layout.
        .or_else(|| {
            context.first().and_then(|(_, path)| {
                std::path::Path::new(path)
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
            })
        })
}

/// Expand a leading `~`. Settings and this dialog are both hand-typed, and
/// `~/p` is the form a person writes.
fn expand_home(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest).to_string_lossy().into_owned();
    }
    p.to_string()
}

/// A short label for the session.
///
/// The user's name if they gave one; otherwise the first few words of the goal,
/// because a session listed as the whole paragraph is unreadable in a sidebar.
fn session_label(name: &str, goal: &str) -> String {
    let name = name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    let mut label: String = goal
        .split_whitespace()
        .take(6)
        .collect::<Vec<_>>()
        .join(" ");
    if label.len() > 48 {
        label.truncate(48);
        label = label.trim_end().to_string();
    }
    if label.is_empty() {
        "agent".to_string()
    } else {
        label
    }
}

/// The opening prompt for a custom session.
///
/// The goal verbatim, plus the projects the user pointed the agent at. The
/// paths matter: the session may be rooted above them, where "the project" is
/// ambiguous until they are named.
pub(super) fn custom_brief(
    goal: &str,
    context: &[(String, String)],
    context_items: &[okena_core::context::ContextItem],
    loaded: bool,
    prompts: &PromptRoots,
) -> String {
    let mut vars = Vars::new();
    vars.insert("goal", goal.to_string());
    vars.insert(
        "context",
        briefs::context_block(context_items, loaded, prompts),
    );
    vars.insert(
        "projects",
        briefs::project_block("given-projects", context, prompts),
    );
    briefs::build(Flow::AgentSession, prompts, &vars)
        .rendered
        .text
}

/// The opening prompt for starting work on a task.
#[allow(clippy::too_many_arguments)]
fn task_brief(
    task: &okena_core::tasks::Task,
    branch: &str,
    context: &[(String, String)],
    note: Option<&str>,
    shape: Option<&BriefShape>,
    context_items: &[okena_core::context::ContextItem],
    loaded: bool,
    prompts: &PromptRoots,
    worktrees: bool,
) -> String {
    let mut vars = Vars::new();
    vars.insert("key", task.display_key.clone());
    vars.insert("title", task.title.clone());
    vars.insert("url", task.url.clone());
    vars.insert("branch", branch.to_string());
    vars.insert(
        "description",
        briefs::block(task.description.as_deref().unwrap_or_default()),
    );
    // A coordinator over picked tasks has no worktrees, and neither has a start
    // whose projects were all plain directories; both are given repos.
    let heading = match shape {
        Some(BriefShape::Picked(_)) => "given-projects",
        _ if !worktrees => "given-projects",
        _ => "given-worktrees",
    };
    vars.insert(
        "projects",
        briefs::project_block(heading, context, prompts),
    );
    vars.insert("note", briefs::block(note.unwrap_or_default()));
    vars.insert(
        "context",
        briefs::context_block(context_items, loaded, prompts),
    );
    // A coordinator is briefed to split the work rather than do it, so it gets
    // that flow's template — not the work brief with the split tucked in.
    let flow = match shape {
        Some(BriefShape::Children(listed)) => {
            vars.insert("children", listed.clone());
            Flow::TaskCoordinate
        }
        Some(BriefShape::Picked(listed)) => {
            vars.insert("tasks", listed.clone());
            Flow::TasksCoordinate
        }
        // One agent on several tasks has no one branch: every task is listed
        // with its own, and it plans and verifies them all.
        Some(BriefShape::Group { key, tasks }) => {
            vars.insert("key", key.clone());
            vars.insert("tasks", tasks.clone());
            let verify = briefs::build(Flow::TaskVerify, prompts, &vars);
            vars.insert("verify", briefs::block(&verify.rendered.text));
            Flow::TasksStart
        }
        // Only the agent doing the work plans and verifies it; a coordinator
        // hands that on to the agents it starts.
        None => {
            let verify = briefs::build(Flow::TaskVerify, prompts, &vars);
            vars.insert("verify", briefs::block(&verify.rendered.text));
            Flow::TaskStart
        }
    };
    briefs::build(flow, prompts, &vars).rendered.text
}

/// Shell for a custom agent session.
pub(super) fn custom_agent_shell(
    settings: &AppSettings,
    override_command: Option<&str>,
    brief: &str,
    install: &super::agent_context::Install,
) -> Option<okena_terminal::shell_config::ShellType> {
    // An explicit empty string means "no agent, just a shell here", even when a
    // default agent is configured — same contract as the other two routes.
    let command = match override_command {
        Some(c) => c,
        None => settings.harness.agent_command.as_deref().unwrap_or(""),
    }
    .trim()
    .to_string();
    if command.is_empty() {
        return None;
    }
    // Named first, so a restart can resume this exact conversation; the
    // agent's own options before the brief, which they never replace.
    let mut args = super::agent_resume::session_args(&command);
    args.extend(super::agent_options::option_args(&command, settings));
    args.extend(super::briefs::brief_args(&command, brief));
    args.extend(super::agent_mcp::injection_args(&command, settings));
    args.extend(install.args.iter().cloned());
    Some(okena_terminal::shell_config::ShellType::Custom {
        path: command,
        args,
    })
}

/// What a teardown removes: checkouts on disk, and sessions that own only
/// terminals.
pub(super) struct TeardownPlan {
    /// `(id, name)` of each worktree to remove, checkout and all.
    pub worktrees: Vec<(String, String)>,
    /// `(id, name)` of each session project to drop.
    pub sessions: Vec<(String, String)>,
}

/// Decide what tearing down `project_id` should remove.
///
/// A task session owns the worktrees created for its task, in other repos, and
/// they go with it. A spec session owns nothing on disk — it runs *in* the
/// user's spec repository, which must survive — so only the session goes.
/// `None` for anything that is not a session: this route deletes checkouts, so
/// it refuses rather than guessing.
pub(super) fn teardown_plan(
    projects: &[okena_workspace::state::ProjectData],
    project_id: &str,
) -> Option<TeardownPlan> {
    let anchor = projects.iter().find(|p| p.id == project_id)?;

    // Neither a spec session nor a free-form one owns a checkout: the first
    // runs in the user's spec repository and the second in a directory they
    // chose, both of which must survive. Only the session goes.
    if anchor.is_spec_session() || anchor.is_custom_session() {
        return Some(TeardownPlan {
            worktrees: Vec::new(),
            sessions: vec![(anchor.id.clone(), anchor.name.clone())],
        });
    }

    // A worktree carries its task too, but it is a checkout, not a session —
    // deleting one must not take its siblings with it.
    let task = anchor
        .task_ref
        .as_ref()
        .filter(|_| anchor.worktree_info.is_none())?;

    // A coordinator over picked tasks has no checkout of its own. The
    // worktrees under its tasks are its agents', and go with theirs.
    if !anchor.repo_ids.is_empty() {
        return Some(TeardownPlan {
            worktrees: Vec::new(),
            sessions: vec![(anchor.id.clone(), anchor.name.clone())],
        });
    }

    // One agent on several tasks has worktrees under each of them.
    let covered: Vec<&str> = anchor
        .linked_tasks()
        .map(|t| t.id.external_id.as_str())
        .collect();
    let mut plan = TeardownPlan {
        worktrees: Vec::new(),
        sessions: Vec::new(),
    };
    for p in projects.iter() {
        let Some(own) = p.task_ref.as_ref() else {
            continue;
        };
        if p.worktree_info.is_some() {
            if covered.contains(&own.id.external_id.as_str()) {
                plan.worktrees.push((p.id.clone(), p.name.clone()));
            }
        } else if own.id.external_id == task.id.external_id {
            plan.sessions.push((p.id.clone(), p.name.clone()));
        }
    }
    Some(plan)
}

/// What the user chose to take down with a session.
#[derive(Clone, Copy, Debug)]
pub(super) struct TeardownChoice {
    /// Remove a dirty checkout rather than refusing it.
    pub force: bool,
    /// Off keeps each worktree as an ordinary worktree project, terminals
    /// closed.
    pub remove_worktrees: bool,
    /// `git branch -D` each removed worktree's local branch.
    pub delete_branches: bool,
}

/// Tear down a task's whole workspace.
///
/// Order is deliberate: worktrees first, session last. The session project is
/// how the user finds this workspace in the sidebar, so removing it first would
/// strand any worktree that failed to delete with no obvious route back to it.
///
/// Failures are collected rather than aborting: a dirty worktree that git
/// refuses to remove should not prevent the rest from being cleaned up, and the
/// result names exactly what survived and why.
#[allow(clippy::too_many_arguments)]
pub(super) fn delete_workspace(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    choice: TeardownChoice,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let Some(anchor) = ws.project(&project_id) else {
        return ActionResult::Err(format!("project not found: {project_id}"));
    };

    // What to tear down depends on the kind of session.
    //
    // A task session owns the worktrees created for its task, in other repos,
    // and they go with it. A spec session owns nothing on disk — it runs *in*
    // the user's spec repository, which must survive — so only the session
    // itself goes. Anything that is not a session is refused rather than
    // guessed at: this route deletes checkouts.
    let task = anchor.task_ref.clone();
    let Some(plan) = teardown_plan(&ws.data.projects, &project_id) else {
        return ActionResult::Err(
            "this project is not an agent session — nothing to tear down".into(),
        );
    };
    let (worktrees, sessions) = (plan.worktrees, plan.sessions);

    let mut removed: Vec<serde_json::Value> = Vec::new();
    let mut kept: Vec<serde_json::Value> = Vec::new();
    let mut failed: Vec<serde_json::Value> = Vec::new();
    // Worktrees whose uncommitted changes a forced removal took with it.
    let mut discarded: Vec<String> = Vec::new();
    let mut deleted_branches: Vec<serde_json::Value> = Vec::new();

    for (id, name) in worktrees {
        if !choice.remove_worktrees {
            // Kept as an ordinary worktree under its repo. Closing its
            // terminals ends their tmux sessions and any agent inside.
            let terminal_ids: Vec<String> = ws
                .project(&id)
                .and_then(|p| p.layout.as_ref())
                .map(|l| l.collect_terminal_ids())
                .unwrap_or_default();
            let closed = if terminal_ids.is_empty() {
                ActionResult::Ok(None)
            } else {
                super::terminal::close_many(
                    ws,
                    focus_manager,
                    id.clone(),
                    terminal_ids,
                    backend,
                    terminals,
                    cx,
                )
            };
            match closed {
                ActionResult::Ok(_) => kept.push(serde_json::json!({
                    "project": name,
                    "kind": "worktree",
                })),
                ActionResult::Err(error) => failed.push(serde_json::json!({
                    "project": name,
                    "kind": "worktree",
                    "error": error,
                })),
            }
            continue;
        }

        // Read before the checkout goes: both live in it.
        let (checkout, main_repo) = match ws.project(&id) {
            Some(p) => {
                let info = p.worktree_info.as_ref();
                let checkout = info
                    .map(|i| i.worktree_path.clone())
                    .filter(|path| !path.is_empty())
                    .unwrap_or_else(|| p.path.clone());
                let main_repo = info.map(|i| i.main_repo_path.clone()).unwrap_or_default();
                (std::path::PathBuf::from(checkout), main_repo)
            }
            None => (std::path::PathBuf::new(), String::new()),
        };
        // A status that cannot be read may hold changes too; naming it is
        // the honest report of what a forced removal may have taken.
        let dirty = choice.force
            && matches!(
                okena_git::uncommitted_changes(&checkout),
                okena_git::DirtyCheck::Known(true) | okena_git::DirtyCheck::Unknown
            );
        let branch = choice
            .delete_branches
            .then(|| okena_git::checked_out_branch(&checkout))
            .flatten();

        // Removes the checkout and closes the project's terminals, which ends
        // the tmux session and with it any agent running inside.
        match super::project::remove_worktree_project(
            ws,
            focus_manager,
            id.clone(),
            choice.force,
            settings,
            cx,
        ) {
            ActionResult::Ok(_) => {
                removed.push(serde_json::json!({
                    "project": name,
                    "kind": "worktree",
                }));
                if dirty {
                    discarded.push(name.clone());
                }
            }
            ActionResult::Err(error) => {
                failed.push(serde_json::json!({
                    "project": name,
                    "kind": "worktree",
                    "error": error,
                }));
                continue;
            }
        }

        // Only once the checkout is gone: git refuses to delete a branch a
        // worktree still has checked out.
        if let Some(branch) = branch {
            let repo = std::path::Path::new(&main_repo);
            // Asked before the delete, which leaves nothing to ask about.
            let merged = okena_git::is_branch_merged(repo, &branch).unwrap_or(false);
            match okena_git::force_delete_local_branch(repo, &branch) {
                Ok(()) => deleted_branches.push(serde_json::json!({
                    "branch": branch,
                    "project": name,
                    "merged": merged,
                })),
                Err(error) => failed.push(serde_json::json!({
                    "project": branch,
                    "kind": "branch",
                    "error": error.to_string(),
                })),
            }
        }
    }

    for (id, name) in sessions {
        match super::project::delete_project(ws, focus_manager, id.clone(), settings, cx) {
            ActionResult::Ok(_) => removed.push(serde_json::json!({
                "project": name,
                "kind": "agent session",
            })),
            ActionResult::Err(error) => failed.push(serde_json::json!({
                "project": name,
                "kind": "agent session",
                "error": error,
            })),
        }
    }

    ws.notify_data(cx);

    ActionResult::Ok(Some(serde_json::json!({
        "task": task,
        "removed": removed,
        "kept": kept,
        "failed": failed,
        "discarded": discarded,
        "deleted_branches": deleted_branches,
    })))
}

#[cfg(test)]
mod teardown_tests {
    use super::teardown_plan;
    use okena_workspace::state::ProjectData;

    fn project(json: serde_json::Value) -> ProjectData {
        serde_json::from_value(json).unwrap()
    }

    fn task_session(id: &str, external: &str) -> ProjectData {
        project(serde_json::json!({
            "id": id, "name": format!("{id} (agent)"), "path": "/p",
            "task_ref": {
                "id": { "provider": "linear", "external_id": external },
                "display_key": "QBL-1", "title": "t", "url": "http://x",
            },
        }))
    }

    fn worktree(id: &str, external: &str) -> ProjectData {
        project(serde_json::json!({
            "id": id, "name": format!("okena ({id})"), "path": format!("/p/wt/{id}"),
            "worktree_info": {
                "parent_project_id": "repo1",
                "main_repo_path": "/p/okena",
                "worktree_path": format!("/p/wt/{id}"),
                "branch_name": "feat/x",
            },
            "task_ref": {
                "id": { "provider": "linear", "external_id": external },
                "display_key": "QBL-1", "title": "t", "url": "http://x",
            },
        }))
    }

    fn spec_session(id: &str) -> ProjectData {
        project(serde_json::json!({
            "id": id, "name": format!("{id} (spec)"), "path": "/p/specs",
            "spec_change": "add-login",
        }))
    }

    fn repo(id: &str) -> ProjectData {
        project(serde_json::json!({ "id": id, "name": id, "path": format!("/p/{id}") }))
    }

    fn ids(v: &[(String, String)]) -> Vec<&str> {
        v.iter().map(|(id, _)| id.as_str()).collect()
    }

    #[test]
    fn a_task_session_takes_its_worktrees_with_it() {
        let projects = vec![
            repo("repo1"),
            task_session("s1", "u1"),
            worktree("wt1", "u1"),
            worktree("wt2", "u1"),
        ];
        let plan = teardown_plan(&projects, "s1").expect("a session");
        assert_eq!(ids(&plan.sessions), ["s1"]);
        assert_eq!(ids(&plan.worktrees), ["wt1", "wt2"]);
    }

    #[test]
    fn another_tasks_worktrees_are_left_alone() {
        let projects = vec![
            task_session("s1", "u1"),
            worktree("wt1", "u1"),
            worktree("other", "u9"),
        ];
        let plan = teardown_plan(&projects, "s1").expect("a session");
        assert_eq!(ids(&plan.worktrees), ["wt1"]);
    }

    #[test]
    fn a_spec_session_takes_nothing_on_disk() {
        // It runs *in* the user's spec repository. Removing a checkout here
        // would delete the repo they keep every spec in.
        let projects = vec![repo("repo1"), spec_session("s1")];
        let plan = teardown_plan(&projects, "s1").expect("a session");
        assert_eq!(ids(&plan.sessions), ["s1"]);
        assert!(plan.worktrees.is_empty(), "a spec session owns no checkout");
    }

    fn custom_session(id: &str) -> ProjectData {
        project(serde_json::json!({
            "id": id, "name": format!("{id} (agent)"), "path": "/p",
            "custom_session": "audit the unwraps",
        }))
    }

    #[test]
    fn a_custom_session_takes_nothing_on_disk() {
        // It runs in a directory the user chose — often a repo they work in.
        // Removing a checkout here would delete their project.
        let projects = vec![repo("repo1"), custom_session("s1")];
        let plan = teardown_plan(&projects, "s1").expect("a session");
        assert_eq!(ids(&plan.sessions), ["s1"]);
        assert!(
            plan.worktrees.is_empty(),
            "a custom session owns no checkout"
        );
    }

    #[test]
    fn an_ordinary_project_is_not_a_teardown_target() {
        // This route deletes checkouts, so it refuses rather than guessing.
        let projects = vec![repo("repo1")];
        assert!(teardown_plan(&projects, "repo1").is_none());
    }

    #[test]
    fn a_worktree_does_not_take_its_siblings_with_it() {
        // A worktree carries the task too. Treating it as the session would
        // let deleting one checkout delete every other checkout for that task.
        let projects = vec![
            task_session("s1", "u1"),
            worktree("wt1", "u1"),
            worktree("wt2", "u1"),
        ];
        assert!(teardown_plan(&projects, "wt1").is_none());
    }

    #[test]
    fn one_agent_on_several_tasks_takes_every_tasks_worktrees() {
        let mut session = task_session("s1", "u1");
        session.also_tasks = vec![
            serde_json::from_value(serde_json::json!({
                "id": { "provider": "linear", "external_id": "u2" },
                "display_key": "QBL-2", "title": "t", "url": "http://x",
            }))
            .unwrap(),
        ];
        let projects = vec![
            session,
            worktree("wt1", "u1"),
            worktree("wt2", "u2"),
            worktree("other", "u9"),
        ];
        let plan = teardown_plan(&projects, "s1").expect("a session");
        assert_eq!(ids(&plan.sessions), ["s1"]);
        assert_eq!(ids(&plan.worktrees), ["wt1", "wt2"]);
    }

    #[test]
    fn a_coordinator_over_picked_tasks_takes_nothing_on_disk() {
        // It has no worktree. Those under its tasks belong to the agents it
        // started, which are torn down on their own.
        let mut coordinator = task_session("c1", "u1");
        coordinator.repo_ids = vec!["repo1".into()];
        let projects = vec![repo("repo1"), coordinator, worktree("wt1", "u1")];
        let plan = teardown_plan(&projects, "c1").expect("a session");
        assert_eq!(ids(&plan.sessions), ["c1"]);
        assert!(plan.worktrees.is_empty(), "{:?}", plan.worktrees);
    }

    #[test]
    fn an_unknown_project_yields_no_plan() {
        assert!(teardown_plan(&[], "ghost").is_none());
    }
}

/// The teardown against real repositories: what goes, what stays, and what the
/// result tells the user about it.
#[cfg(test)]
mod delete_workspace_tests {
    use super::{TeardownChoice, delete_workspace};
    use crate::workspace::actions::execute::ActionResult;
    use crate::workspace::focus::FocusManager;
    use crate::workspace::persistence::AppSettings;
    use crate::workspace::state::{ProjectData, WindowState, Workspace, WorkspaceData};
    use okena_terminal::TerminalsRegistry;
    use okena_terminal::backend::TerminalBackend;
    use okena_terminal::shell_config::ShellType;
    use okena_terminal::terminal::TerminalTransport;
    use okena_workspace::context::WorkspaceCx;
    use okena_workspace::hook_monitor::HookMonitor;
    use okena_workspace::hooks::HookRunner;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::{Arc, Mutex};

    struct NoTransport;

    impl TerminalTransport for NoTransport {
        fn send_input(&self, _terminal_id: &str, _data: &[u8]) {}
        fn resize(&self, _terminal_id: &str, _cols: u16, _rows: u16) {}
        fn uses_mouse_backend(&self) -> bool {
            false
        }
    }

    /// Records which terminals were killed; spawns nothing.
    #[derive(Default)]
    struct KillRecorder {
        killed: Mutex<Vec<String>>,
    }

    impl TerminalBackend for KillRecorder {
        fn transport(&self) -> Arc<dyn TerminalTransport> {
            Arc::new(NoTransport)
        }
        fn create_terminal(
            &self,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            unreachable!("a teardown creates no terminals")
        }
        fn reconnect_terminal(
            &self,
            _terminal_id: &str,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            unreachable!("a teardown reconnects no terminals")
        }
        fn kill(&self, terminal_id: &str) {
            self.killed.lock().unwrap().push(terminal_id.to_string());
        }
        fn capture_buffer(&self, _terminal_id: &str) -> Option<PathBuf> {
            None
        }
        fn supports_buffer_capture(&self) -> bool {
            false
        }
        fn is_remote(&self) -> bool {
            false
        }
        fn get_shell_pid(&self, _terminal_id: &str) -> Option<u32> {
            None
        }
        fn get_service_pids(&self, _terminal_id: &str) -> Vec<u32> {
            Vec::new()
        }
    }

    struct TestCx {
        monitor: HookMonitor,
    }

    impl WorkspaceCx for TestCx {
        fn notify(&mut self) {}
        fn refresh_views(&mut self) {}
        fn hook_runner(&self) -> Option<HookRunner> {
            None
        }
        fn hook_monitor(&self) -> Option<HookMonitor> {
            Some(self.monitor.clone())
        }
    }

    fn git(dir: &Path, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("run git")
    }

    fn git_ok(dir: &Path, args: &[&str]) {
        let out = git(dir, args);
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn branch_exists(repo: &Path, branch: &str) -> bool {
        git(
            repo,
            &[
                "show-ref",
                "--verify",
                "-q",
                &format!("refs/heads/{branch}"),
            ],
        )
        .status
        .success()
    }

    fn task_ref() -> serde_json::Value {
        serde_json::json!({
            "id": { "provider": "linear", "external_id": "u1" },
            "display_key": "QBL-1", "title": "t", "url": "http://x",
        })
    }

    /// A repo, a session for task u1, and two worktrees for it:
    /// `wt-clean` on `feat/merged` (no commits of its own, so merged) and
    /// `wt-dirty` on `feat/unmerged` (one commit ahead, plus an uncommitted
    /// file). Each worktree runs one terminal.
    struct Fixture {
        _dir: tempfile::TempDir,
        repo: PathBuf,
        clean: PathBuf,
        dirty: PathBuf,
        ws: Workspace,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, &["init", "-q", "-b", "main"]);
        git_ok(&repo, &["config", "user.email", "okena@example.invalid"]);
        git_ok(&repo, &["config", "user.name", "Okena Test"]);
        git_ok(&repo, &["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.join("file.txt"), "base\n").unwrap();
        git_ok(&repo, &["add", "file.txt"]);
        git_ok(&repo, &["commit", "-q", "-m", "base"]);

        let clean = root.join("wt-clean");
        let dirty = root.join("wt-dirty");
        git_ok(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat/merged",
                clean.to_str().unwrap(),
            ],
        );
        git_ok(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat/unmerged",
                dirty.to_str().unwrap(),
            ],
        );
        std::fs::write(dirty.join("committed.txt"), "ahead\n").unwrap();
        git_ok(&dirty, &["add", "committed.txt"]);
        git_ok(&dirty, &["commit", "-q", "-m", "ahead of main"]);
        std::fs::write(dirty.join("uncommitted.txt"), "work in progress\n").unwrap();

        let parse = |v: serde_json::Value| -> ProjectData { serde_json::from_value(v).unwrap() };
        let worktree = |id: &str, path: &Path, branch: &str, terminal: &str| {
            parse(serde_json::json!({
                "id": id, "name": id, "path": path,
                "layout": { "type": "terminal", "terminal_id": terminal },
                "worktree_info": {
                    "parent_project_id": "repo",
                    "main_repo_path": repo,
                    "worktree_path": path,
                    "branch_name": branch,
                },
                "task_ref": task_ref(),
            }))
        };
        let projects = vec![
            parse(serde_json::json!({
                "id": "repo", "name": "repo", "path": repo,
                "worktree_ids": ["wt-clean", "wt-dirty"],
            })),
            parse(serde_json::json!({
                "id": "s1", "name": "s1 (agent)", "path": root,
                "task_ref": task_ref(),
            })),
            worktree("wt-clean", &clean, "feat/merged", "t-clean"),
            worktree("wt-dirty", &dirty, "feat/unmerged", "t-dirty"),
        ];
        let ws = Workspace::new(WorkspaceData {
            version: 1,
            projects,
            project_order: vec!["repo".into(), "s1".into()],
            service_panel_heights: Default::default(),
            hook_panel_heights: Default::default(),
            folders: Vec::new(),
            main_window: WindowState::default(),
            extra_windows: Vec::new(),
        });
        Fixture {
            _dir: dir,
            repo,
            clean,
            dirty,
            ws,
        }
    }

    fn run(f: &mut Fixture, choice: TeardownChoice, backend: &KillRecorder) -> serde_json::Value {
        let terminals: TerminalsRegistry = Default::default();
        let mut cx = TestCx {
            monitor: HookMonitor::new(),
        };
        match delete_workspace(
            &mut f.ws,
            &mut FocusManager::default(),
            "s1".into(),
            choice,
            backend,
            &terminals,
            &AppSettings::default(),
            &mut cx,
        ) {
            ActionResult::Ok(Some(v)) => v,
            other => panic!("delete failed: {:?}", other.err()),
        }
    }

    trait ResultErr {
        fn err(self) -> Option<String>;
    }

    impl ResultErr for ActionResult {
        fn err(self) -> Option<String> {
            match self {
                ActionResult::Err(e) => Some(e),
                ActionResult::Ok(_) => None,
            }
        }
    }

    #[test]
    fn a_partial_delete_stops_the_worktrees_and_keeps_them() {
        let mut f = fixture();
        let backend = KillRecorder::default();
        let result = run(
            &mut f,
            TeardownChoice {
                force: true,
                remove_worktrees: false,
                // Ignored while worktrees stay: a kept checkout holds its branch.
                delete_branches: true,
            },
            &backend,
        );

        assert_eq!(result["failed"], serde_json::json!([]), "{result}");
        assert!(f.ws.project("s1").is_none(), "the session goes");
        for id in ["wt-clean", "wt-dirty"] {
            let p = f.ws.project(id).unwrap_or_else(|| panic!("{id} kept"));
            let running = p
                .layout
                .as_ref()
                .map(|l| l.collect_terminal_ids())
                .unwrap_or_default();
            assert!(running.is_empty(), "{id} still runs {running:?}");
        }
        let mut killed = backend.killed.lock().unwrap().clone();
        killed.sort();
        assert_eq!(killed, ["t-clean", "t-dirty"]);
        assert!(f.clean.exists() && f.dirty.exists(), "checkouts stay");
        assert!(
            f.dirty.join("uncommitted.txt").exists(),
            "uncommitted work stays"
        );
        assert!(branch_exists(&f.repo, "feat/merged"));
        assert!(branch_exists(&f.repo, "feat/unmerged"));
        assert_eq!(result["discarded"], serde_json::json!([]));
        assert_eq!(result["deleted_branches"], serde_json::json!([]));
    }

    #[test]
    fn a_full_delete_removes_a_dirty_worktree_and_says_so() {
        let mut f = fixture();
        let result = run(
            &mut f,
            TeardownChoice {
                force: true,
                remove_worktrees: true,
                delete_branches: false,
            },
            &KillRecorder::default(),
        );

        assert_eq!(result["failed"], serde_json::json!([]), "{result}");
        for id in ["s1", "wt-clean", "wt-dirty"] {
            assert!(f.ws.project(id).is_none(), "{id} removed");
        }
        assert!(!f.clean.exists() && !f.dirty.exists(), "checkouts removed");
        assert_eq!(result["discarded"], serde_json::json!(["wt-dirty"]));
        assert!(branch_exists(&f.repo, "feat/merged"), "branches stay");
        assert!(branch_exists(&f.repo, "feat/unmerged"), "branches stay");
        assert_eq!(result["deleted_branches"], serde_json::json!([]));
    }

    #[test]
    fn a_full_delete_with_branches_deletes_unmerged_ones_and_names_them() {
        let mut f = fixture();
        let result = run(
            &mut f,
            TeardownChoice {
                force: true,
                remove_worktrees: true,
                delete_branches: true,
            },
            &KillRecorder::default(),
        );

        assert_eq!(result["failed"], serde_json::json!([]), "{result}");
        assert!(!branch_exists(&f.repo, "feat/merged"));
        assert!(!branch_exists(&f.repo, "feat/unmerged"), "-D, not -d");
        let mut deleted = result["deleted_branches"].as_array().unwrap().clone();
        deleted.sort_by_key(|b| b["branch"].as_str().unwrap().to_string());
        assert_eq!(
            deleted,
            [
                serde_json::json!({ "branch": "feat/merged", "project": "wt-clean", "merged": true }),
                serde_json::json!({ "branch": "feat/unmerged", "project": "wt-dirty", "merged": false }),
            ]
        );
        assert!(
            branch_exists(&f.repo, "main"),
            "the repo's own branch stays"
        );
    }

    #[test]
    fn without_force_a_dirty_worktree_is_kept_and_reported() {
        // What an older client that never asked to force still gets.
        let mut f = fixture();
        let result = run(
            &mut f,
            TeardownChoice {
                force: false,
                remove_worktrees: true,
                delete_branches: true,
            },
            &KillRecorder::default(),
        );

        let failed = result["failed"].as_array().unwrap();
        assert_eq!(failed.len(), 1, "{result}");
        assert_eq!(failed[0]["project"], "wt-dirty");
        assert!(f.dirty.join("uncommitted.txt").exists());
        assert!(branch_exists(&f.repo, "feat/unmerged"), "its branch stays");
        assert_eq!(result["discarded"], serde_json::json!([]));
    }
}

#[cfg(test)]
mod custom_session_tests {
    use super::{custom_brief, resolve_session_root, session_label};
    use crate::workspace::persistence::AppSettings;

    fn ctx(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(n, p)| (n.to_string(), p.to_string()))
            .collect()
    }

    fn settings_with_root(root: Option<&str>) -> AppSettings {
        let mut s = AppSettings::default();
        s.harness.agent_root = root.map(str::to_string);
        s
    }

    #[test]
    fn an_explicit_directory_wins() {
        let s = settings_with_root(Some("/configured"));
        let got = resolve_session_root("/explicit", &ctx(&[("a", "/p/a")]), &s);
        assert_eq!(got.as_deref(), Some("/explicit"));
    }

    #[test]
    fn one_project_runs_in_itself() {
        // Rooting a single-project session above the repo would make every
        // relative path in the brief wrong.
        let s = settings_with_root(Some("/configured"));
        let got = resolve_session_root("", &ctx(&[("a", "/p/a")]), &s);
        assert_eq!(got.as_deref(), Some("/p/a"));
    }

    #[test]
    fn several_projects_run_at_the_configured_root() {
        // An agent given three repos needs a directory above all of them.
        let s = settings_with_root(Some("/configured"));
        let got = resolve_session_root("", &ctx(&[("a", "/p/a"), ("b", "/p/b")]), &s);
        assert_eq!(got.as_deref(), Some("/configured"));
    }

    #[test]
    fn without_a_configured_root_several_projects_use_their_parent() {
        let s = settings_with_root(None);
        let got = resolve_session_root("", &ctx(&[("a", "/p/a"), ("b", "/p/b")]), &s);
        assert_eq!(got.as_deref(), Some("/p"));
    }

    #[test]
    fn nothing_to_go_on_yields_no_root() {
        // The caller turns this into "pick a directory" rather than guessing at
        // one and starting an agent somewhere unexpected.
        let s = settings_with_root(None);
        assert!(resolve_session_root("", &[], &s).is_none());
    }

    #[test]
    fn a_blank_configured_root_counts_as_unset() {
        let s = settings_with_root(Some("   "));
        assert!(resolve_session_root("", &[], &s).is_none());
    }

    #[test]
    fn a_name_is_used_verbatim() {
        assert_eq!(
            session_label("Refactor auth", "some long goal"),
            "Refactor auth"
        );
    }

    #[test]
    fn without_a_name_the_label_is_the_first_few_words() {
        // A session listed as a whole paragraph is unreadable in a sidebar.
        let goal = "Migrate the billing service off the legacy queue and delete the shim";
        let label = session_label("", goal);
        assert_eq!(label, "Migrate the billing service off the");
        assert!(label.len() <= 48);
    }

    #[test]
    fn an_empty_goal_still_gets_a_label() {
        assert_eq!(session_label("", ""), "agent");
    }

    #[test]
    fn the_brief_names_the_projects_it_was_given() {
        // The session may be rooted above them, where "the project" is
        // ambiguous until they are named.
        let b = custom_brief(
            "Do the thing",
            &ctx(&[("okena", "/p/okena")]),
            &[],
            false,
            &Vec::new(),
        );
        assert!(b.starts_with("Do the thing"));
        assert!(b.contains("/p/okena"), "the path is what disambiguates");
    }

    #[test]
    fn a_brief_without_projects_is_the_goal_and_the_reporting_rule() {
        // No project list and no context block when none were given — and,
        // like every brief, how to look context up and how to report when it
        // stops to wait.
        let b = custom_brief("Do the thing", &[], &[], false, &Vec::new());
        let lookup = okena_knowledge::prompts::defaults::partial_body("context-lookup")
            .expect("context-lookup partial");
        let reporting = okena_knowledge::prompts::defaults::partial_body("reporting")
            .expect("reporting partial");
        assert_eq!(b, format!("Do the thing\n\n{lookup}\n\n{reporting}"));
    }
}

#[cfg(test)]
mod kind_wire_tests {
    use super::parse_kind;
    use okena_core::tasks::TaskKind;

    #[test]
    fn every_kind_round_trips_through_its_wire_name() {
        // The UI and the MCP tools both send `wire_name`, so a kind that does
        // not survive the trip is silently created as a plain task.
        for kind in TaskKind::all() {
            assert_eq!(parse_kind(kind.wire_name()), kind, "{:?}", kind);
        }
    }

    #[test]
    fn a_providers_own_vocabulary_is_accepted() {
        // Agents and humans write "bug", not "defect".
        assert_eq!(parse_kind("bug"), TaskKind::Defect);
        assert_eq!(parse_kind("hotfix"), TaskKind::Defect);
        assert_eq!(parse_kind("initiative"), TaskKind::Epic);
        assert_eq!(parse_kind("user story"), TaskKind::Story);
    }

    #[test]
    fn matching_ignores_case_and_padding() {
        assert_eq!(parse_kind("  Epic "), TaskKind::Epic);
        assert_eq!(parse_kind("FEATURE"), TaskKind::Feature);
    }

    #[test]
    fn an_unknown_kind_becomes_a_task_rather_than_an_error() {
        // A newer client, or an agent guessing, should still get a task —
        // refusing the whole creation over a label would be the wrong trade.
        assert_eq!(parse_kind("spike"), TaskKind::Task);
        assert_eq!(parse_kind(""), TaskKind::Task);
    }
}

#[cfg(test)]
mod record_tests {
    use super::record_created_task;
    use crate::workspace::state::Workspace;
    use okena_core::harness::AgentAssetKind;
    use okena_workspace::context::WorkspaceCx;
    use serde_json::{Value, json};

    struct TestCx;

    impl WorkspaceCx for TestCx {
        fn notify(&mut self) {}
        fn refresh_views(&mut self) {}
        fn hook_runner(&self) -> Option<okena_hooks::HookRunner> {
            None
        }
        fn hook_monitor(&self) -> Option<okena_hooks::HookMonitor> {
            None
        }
    }

    fn workspace_with_session(id: &str) -> Workspace {
        let mut ws = Workspace::new(crate::workspace::state::WorkspaceData::empty());
        let session = serde_json::from_value(json!({
            "id": id, "name": id, "path": "/tmp/session", "layout": null,
        }))
        .expect("a minimal project");
        ws.data.projects.push(session);
        ws
    }

    /// The provider's answer to a create, as the daemon hands it on.
    fn created() -> Value {
        json!({
            "id": { "provider": "linear", "external_id": "uuid-9" },
            "display_key": "QBL-9",
            "title": "Split payments",
            "description": null,
            "state": "todo",
            "state_name": "Todo",
            "url": "https://linear.app/q/issue/QBL-9/split-payments",
            "branch_name": "chore/qbl-9-split-payments",
            "updated_at": "",
            "kind": "task",
            "parent_id": null,
            "parent_key": null,
            "labels": [],
            "groups": [],
        })
    }

    #[test]
    fn a_task_filed_for_a_session_is_recorded_on_it() {
        let mut ws = workspace_with_session("s1");
        assert!(record_created_task(&mut ws, "s1", &created(), &mut TestCx));
        let assets = &ws.data.projects[0]
            .agent
            .as_ref()
            .expect("the session now has agent state")
            .assets;
        assert_eq!(assets.len(), 1);
        let asset = &assets[0];
        assert_eq!(asset.kind, AgentAssetKind::Task);
        assert_eq!(asset.title, "Split payments");
        assert_eq!(
            asset.url.as_deref(),
            Some("https://linear.app/q/issue/QBL-9/split-payments")
        );
        let task = asset.task.as_ref().expect("the task rides on the asset");
        assert_eq!(task.display_key, "QBL-9");
        assert_eq!(task.id.external_id, "uuid-9");
    }

    #[test]
    fn a_session_that_is_gone_records_nothing_and_fails_nothing() {
        let mut ws = workspace_with_session("s1");
        assert!(!record_created_task(
            &mut ws,
            "gone",
            &created(),
            &mut TestCx
        ));
        assert!(ws.data.projects[0].agent.is_none());
    }

    #[test]
    fn a_choice_the_provider_needs_is_not_a_task_to_record() {
        let mut ws = workspace_with_session("s1");
        let choice = json!({ "needs_choice": "choose a team for the new task" });
        assert!(!record_created_task(&mut ws, "s1", &choice, &mut TestCx));
        assert!(ws.data.projects[0].agent.is_none());
    }

    #[test]
    fn every_filed_task_is_a_row_of_its_own_until_the_list_is_read() {
        // Stored as reported; matching repeats is the list's job.
        let mut ws = workspace_with_session("s1");
        for _ in 0..2 {
            record_created_task(&mut ws, "s1", &created(), &mut TestCx);
        }
        let assets = &ws.data.projects[0].agent.as_ref().expect("state").assets;
        assert_eq!(assets.len(), 2);
    }
}
