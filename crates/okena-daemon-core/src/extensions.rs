//! The daemon's extension host: extensions installed from git, run as WASM
//! components here so every client — local, remote, mobile — sees the same
//! thing through the snapshot.
//!
//! The host itself lives in `okena-extension-host` and blocks; everything
//! here runs its calls on the blocking pool and turns them into command
//! results. Enabling and configuring an extension go through the ordinary
//! settings (`enabled_extensions`, `extension_settings`); installing enables
//! it and removing drops both.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use okena_core::api::{ActionRequest, CommandResult};
use okena_core::extension::{AgentMode, ExtAgentCall, ExtAgentLaunch, ExtAgentTools, Invoker};
use okena_core::harness::AgentPurpose;
use okena_extension_host::exec::SearchPath;
use okena_extension_host::host::{ExtensionHost, HostConfig, HostSettings};
use okena_extension_host::runtime::HostProject;
use okena_extension_host::store::Dirs;
use okena_workspace::settings::AppSettings;
use okena_workspace::state::Workspace;
use parking_lot::Mutex;
use tokio::sync::watch;

/// Ids of the extensions compiled into okena, which one from git cannot take.
pub const BUILT_IN_IDS: &[&str] = &["claude-code", "codex", "github", "updater"];

/// How often installed extensions are checked for a newer commit.
const UPDATE_CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// The first check waits for startup to settle.
const FIRST_UPDATE_CHECK_AFTER: Duration = Duration::from_secs(60);

/// Builds the host over the profile's extensions folder and starts what the
/// settings enable. `None` (logged) when the WASM runtime cannot start: okena
/// runs on without extensions.
pub fn start_host(
    workspace: Arc<Mutex<Workspace>>,
    settings: &Arc<Mutex<AppSettings>>,
    state_version: Arc<watch::Sender<u64>>,
) -> Option<Arc<ExtensionHost>> {
    let root = okena_core::profiles::try_current()?.extensions_dir();
    let projects_workspace = workspace;
    let host = ExtensionHost::new(HostConfig {
        dirs: Dirs::new(root),
        search_path: search_path(),
        projects: Arc::new(move || {
            projects_workspace
                .lock()
                .data()
                .projects
                .iter()
                // Agent sessions are projects too, but not ones to work in.
                .filter(|p| p.custom_session.is_none() && p.closed_at.is_none())
                .map(|p| HostProject {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    path: p.path.clone(),
                })
                .collect()
        }),
        on_change: Arc::new(move || {
            state_version.send_modify(|v| *v = v.wrapping_add(1));
        }),
        reserved_ids: BUILT_IN_IDS.iter().map(|s| s.to_string()).collect(),
    });
    match host {
        Ok(host) => {
            let host = Arc::new(host);
            host.apply_settings(host_settings(&settings.lock()));
            Some(host)
        }
        Err(e) => {
            log::error!("extensions are unavailable: {e}");
            None
        }
    }
}

#[cfg(not(windows))]
fn search_path() -> SearchPath {
    SearchPath(Some(okena_terminal::session_backend::get_extended_path().into()))
}

#[cfg(windows)]
fn search_path() -> SearchPath {
    SearchPath::from_env()
}

/// What the host needs from the settings.
pub fn host_settings(settings: &AppSettings) -> HostSettings {
    HostSettings {
        enabled: settings.enabled_extensions.clone(),
        configs: settings.extension_settings.clone(),
    }
}

/// Checks for updates now and then, for as long as the daemon runs.
pub async fn run_update_checks(host: Arc<ExtensionHost>, runtime: tokio::runtime::Handle) {
    tokio::time::sleep(FIRST_UPDATE_CHECK_AFTER).await;
    loop {
        let checking = host.clone();
        let _ = runtime.spawn_blocking(move || checking.check_updates()).await;
        tokio::time::sleep(UPDATE_CHECK_EVERY).await;
    }
}

pub fn is_extension_action(action: &ActionRequest) -> bool {
    matches!(
        action,
        ActionRequest::ExtensionPreview { .. }
            | ActionRequest::ExtensionInstall { .. }
            | ActionRequest::ExtensionPreviewUpdate { .. }
            | ActionRequest::ExtensionUpdate { .. }
            | ActionRequest::ExtensionCheckUpdates
            | ActionRequest::ExtensionReload { .. }
            | ActionRequest::ExtensionRemove { .. }
            | ActionRequest::ExtensionRefresh { .. }
            | ActionRequest::ExtensionRecheck { .. }
            | ActionRequest::ExtensionRunAction { .. }
            | ActionRequest::ExtensionQuery { .. }
            | ActionRequest::ExtensionAgentCall { .. }
            | ActionRequest::ExtensionAgentTools { .. }
            | ActionRequest::ExtensionConfirm { .. }
    )
}

fn json(value: serde_json::Result<serde_json::Value>) -> CommandResult {
    match value {
        Ok(v) => CommandResult::Ok(Some(v)),
        Err(e) => CommandResult::Err(e.to_string()),
    }
}

fn done(result: Result<(), String>) -> CommandResult {
    match result {
        Ok(()) => CommandResult::Ok(None),
        Err(e) => CommandResult::Err(e),
    }
}

/// What the extension actions need beyond the host.
#[derive(Clone)]
pub struct Context {
    pub settings: Arc<Mutex<AppSettings>>,
    pub state_version: Arc<watch::Sender<u64>>,
    pub workspace: Arc<Mutex<Workspace>>,
    /// Into the daemon's own command loop: starting an agent session needs
    /// the workspace, which only the loop mutates.
    pub bridge: okena_remote_server::bridge::BridgeSender,
    pub runtime: tokio::runtime::Handle,
}

/// Runs one extension action, off the command loop.
pub async fn run(host: Arc<ExtensionHost>, action: ActionRequest, cx: Context) -> CommandResult {
    match action {
        ActionRequest::ExtensionRunAction {
            id,
            action_id,
            items,
            inputs,
        } => run_action(host, id, action_id, items, inputs, cx).await,
        ActionRequest::ExtensionAgentCall { terminal_id, call } => {
            agent_call(host, terminal_id, call, cx).await
        }
        ActionRequest::ExtensionAgentTools { terminal_id } => {
            match calling_session(&cx.workspace, &terminal_id)
                .and_then(|session| agent_tools(&host, &session))
            {
                Ok(tools) => json(serde_json::to_value(tools)),
                Err(e) => CommandResult::Err(e),
            }
        }
        ActionRequest::ExtensionConfirm {
            id,
            confirmation,
            approve,
        } => done(host.confirm(&id, &confirmation, approve)),
        other => {
            let settings = cx.settings.clone();
            let state_version = cx.state_version.clone();
            cx.runtime
                .spawn_blocking(move || {
                    execute(&host, other, &settings, &|| {
                        state_version.send_modify(|v| *v = v.wrapping_add(1));
                    })
                })
                .await
                .unwrap_or_else(|e| CommandResult::Err(format!("extension worker failed: {e}")))
        }
    }
}

async fn blocking<T: Send + 'static>(
    cx: &Context,
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    cx.runtime
        .spawn_blocking(f)
        .await
        .map_err(|e| format!("extension worker failed: {e}"))?
}

/// Runs a user's action. An action that launches an agent reports how: a
/// prefill goes back to the client for its launcher; a start is started here,
/// if the user approved starting agents.
async fn run_action(
    host: Arc<ExtensionHost>,
    id: String,
    action_id: String,
    items: Vec<String>,
    inputs: Vec<(String, String)>,
    cx: Context,
) -> CommandResult {
    let def = host.action_def(&id, &action_id);
    let mode = def.as_ref().and_then(|d| d.agent);
    if mode == Some(AgentMode::Start) && !host.approved(&id).is_some_and(|p| p.start_agents) {
        return CommandResult::Err(format!(
            "`{action_id}` starts an agent, which {id} was not approved to do"
        ));
    }
    let worker = host.clone();
    let (ext_id, action) = (id.clone(), action_id.clone());
    let outcome = blocking(&cx, move || {
        worker.run_action(&ext_id, &action, items, inputs, Invoker::User)
    })
    .await;
    let mut outcome = match outcome {
        Ok(outcome) => outcome,
        Err(e) => return CommandResult::Err(e),
    };
    if outcome.agent.is_some() {
        outcome.agent_mode = Some(mode.unwrap_or(AgentMode::Prefill));
    }
    if outcome.agent_mode == Some(AgentMode::Start)
        && let Some(agent) = &outcome.agent
    {
        match start_session(&id, agent, &cx).await {
            Ok(project_id) => outcome.session_project_id = Some(project_id),
            Err(e) => return CommandResult::Err(format!("starting the agent failed: {e}")),
        }
    }
    json(serde_json::to_value(outcome))
}

/// Starts the agent session an extension asked for, through the command
/// loop like any client's `AgentStartSession`, tagged with the extension and
/// its item. Returns the session's project id.
async fn start_session(id: &str, agent: &ExtAgentLaunch, cx: &Context) -> Result<String, String> {
    let (reply, answer) = tokio::sync::oneshot::channel();
    let command = okena_remote_server::bridge::RemoteCommand::Action(ActionRequest::AgentStartSession {
        goal: agent.goal.clone(),
        name: agent.name.clone().unwrap_or_default(),
        root: agent.root.clone().unwrap_or_default(),
        project_ids: agent.project_ids.clone(),
        agent_command: None,
        model: None,
        task_draft: None,
        task: None,
        purpose: Some(extension_purpose(id, agent)),
        context: agent.context.clone(),
    });
    cx.bridge
        .send(okena_remote_server::bridge::BridgeMessage {
            command,
            reply: Some(reply),
        })
        .await
        .map_err(|_| "the daemon is shutting down".to_string())?;
    match answer.await.map_err(|_| "no answer from the daemon".to_string())? {
        CommandResult::Ok(Some(value)) => value
            .get("project_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| "the session started without an id".to_string()),
        CommandResult::Err(e) => Err(e),
        _ => Err("the session started without an id".to_string()),
    }
}

/// How a session an extension launched is tagged, so its item can show the
/// session's badge and the panel can name the extension.
pub fn extension_purpose(id: &str, agent: &ExtAgentLaunch) -> AgentPurpose {
    AgentPurpose::Extension {
        extension: id.to_string(),
        item: agent.item.clone(),
        item_label: agent.item_label.clone(),
    }
}

/// The session holding `terminal_id`, when an extension started it.
#[derive(Debug)]
struct CallingSession {
    project_id: String,
    extension: String,
    item: Option<String>,
}

fn calling_session(workspace: &Arc<Mutex<Workspace>>, terminal_id: &str) -> Result<CallingSession, String> {
    let ws = workspace.lock();
    let project = ws
        .data()
        .projects
        .iter()
        .find(|p| {
            p.layout
                .as_ref()
                .is_some_and(|l| l.collect_terminal_ids().iter().any(|t| t == terminal_id))
        })
        .ok_or_else(|| format!("no okena session has terminal `{terminal_id}`"))?;
    match &project.agent_purpose {
        Some(AgentPurpose::Extension { extension, item, .. }) => Ok(CallingSession {
            project_id: project.id.clone(),
            extension: extension.clone(),
            item: item.clone(),
        }),
        _ => Err("this session was not started by an extension, so it has no extension to call".into()),
    }
}

fn agent_tools(host: &ExtensionHost, session: &CallingSession) -> Result<ExtAgentTools, String> {
    let ext = host
        .extension(&session.extension)
        .ok_or_else(|| format!("extension `{}` is no longer installed", session.extension))?;
    Ok(ExtAgentTools {
        extension: ext.id.clone(),
        name: ext.name.clone(),
        item: session.item.clone(),
        actions: ext.actions.into_iter().filter(|a| a.agent_callable).collect(),
        queries: ext.queries,
    })
}

/// An agent's call to the extension that started it. Only its own
/// extension, only agent-callable actions, and a destructive one only after
/// the user confirms it in okena.
async fn agent_call(host: Arc<ExtensionHost>, terminal_id: String, call: ExtAgentCall, cx: Context) -> CommandResult {
    let session = match calling_session(&cx.workspace, &terminal_id) {
        Ok(session) => session,
        Err(e) => return CommandResult::Err(e),
    };
    let id = session.extension.clone();
    match call {
        ExtAgentCall::Query { query, args } => {
            let args = if args.is_null() { "{}".to_string() } else { args.to_string() };
            match blocking(&cx, move || host.query(&id, &query, &args)).await {
                Ok(answer) => CommandResult::Ok(Some(
                    serde_json::from_str(&answer).unwrap_or(serde_json::Value::String(answer)),
                )),
                Err(e) => CommandResult::Err(e),
            }
        }
        ExtAgentCall::Action {
            action_id,
            items,
            inputs,
        } => {
            let Some(def) = host.action_def(&id, &action_id) else {
                return CommandResult::Err(format!("{id} has no action `{action_id}`"));
            };
            if !def.agent_callable {
                return CommandResult::Err(format!(
                    "{id} does not let agents run `{action_id}`; ask the user to run it in okena"
                ));
            }
            if def.agent.is_some() {
                return CommandResult::Err("agents cannot launch other agents through an extension".into());
            }
            if def.destructive {
                let waiting = host.clone();
                let (ext_id, action, confirm_items, session) =
                    (id.clone(), def.clone(), items.clone(), session.project_id.clone());
                let approved = blocking(&cx, move || {
                    Ok(waiting.await_confirmation(&ext_id, &action, &confirm_items, Some(session)))
                })
                .await
                .unwrap_or(false);
                if !approved {
                    return CommandResult::Err(format!(
                        "the user did not confirm `{}` (declined, or no answer within {} minutes)",
                        def.label,
                        okena_extension_host::host::CONFIRM_TIMEOUT.as_secs() / 60
                    ));
                }
            }
            match blocking(&cx, move || host.run_action(&id, &action_id, items, inputs, Invoker::Agent)).await {
                Ok(outcome) => json(serde_json::to_value(outcome)),
                Err(e) => CommandResult::Err(e),
            }
        }
    }
}

/// Runs one extension action. Blocks: call it on the blocking pool.
///
/// `settings` is the daemon's shared settings cell. Install and remove
/// change `enabled_extensions` / `extension_settings` in it, save it, and
/// call `settings_changed` so clients pick the change up.
pub fn execute(
    host: &ExtensionHost,
    action: ActionRequest,
    settings: &Arc<Mutex<AppSettings>>,
    settings_changed: &dyn Fn(),
) -> CommandResult {
    match action {
        ActionRequest::ExtensionPreview { source } => match host.preview_install(&source) {
            Ok(preview) => json(serde_json::to_value(&preview)),
            Err(e) => CommandResult::Err(e),
        },
        ActionRequest::ExtensionInstall {
            source,
            commit,
            approved,
        } => match host.install(&source, commit.as_deref(), &approved) {
            Ok(record) => {
                let id = record.id.clone();
                if let Err(e) = update_settings(settings, |s| {
                    s.enabled_extensions.insert(id.clone());
                }) {
                    return CommandResult::Err(format!("installed, but enabling it failed: {e}"));
                }
                host.apply_settings(host_settings(&settings.lock()));
                settings_changed();
                json(serde_json::to_value(&record))
            }
            Err(e) => CommandResult::Err(e),
        },
        ActionRequest::ExtensionPreviewUpdate { id } => match host.preview_update(&id) {
            Ok(preview) => json(serde_json::to_value(&preview)),
            Err(e) => CommandResult::Err(e),
        },
        ActionRequest::ExtensionUpdate {
            id,
            commit,
            approved,
        } => match host.update(&id, commit.as_deref(), approved.as_ref()) {
            Ok(record) => json(serde_json::to_value(&record)),
            Err(e) => CommandResult::Err(e),
        },
        ActionRequest::ExtensionCheckUpdates => json(serde_json::to_value(host.check_updates())),
        ActionRequest::ExtensionReload { id, approved } => match host.reload(&id, approved.as_ref()) {
            Ok(record) => json(serde_json::to_value(&record)),
            Err(e) => CommandResult::Err(e),
        },
        ActionRequest::ExtensionRemove { id } => {
            if let Err(e) = host.remove(&id) {
                return CommandResult::Err(e);
            }
            let result = update_settings(settings, |s| {
                s.enabled_extensions.remove(&id);
                s.extension_settings.remove(&id);
            });
            host.apply_settings(host_settings(&settings.lock()));
            settings_changed();
            done(result.map_err(|e| format!("removed, but clearing its settings failed: {e}")))
        }
        ActionRequest::ExtensionRefresh { id } => done(host.refresh(&id)),
        ActionRequest::ExtensionRecheck { id } => done(host.recheck(&id)),
        ActionRequest::ExtensionRunAction {
            id,
            action_id,
            items,
            inputs,
        } => match host.run_action(&id, &action_id, items, inputs, Invoker::User) {
            Ok(outcome) => json(serde_json::to_value(&outcome)),
            Err(e) => CommandResult::Err(e),
        },
        ActionRequest::ExtensionQuery { id, query, args } => {
            let args = if args.is_null() { "{}".to_string() } else { args.to_string() };
            match host.query(&id, &query, &args) {
                Ok(answer) => CommandResult::Ok(Some(
                    serde_json::from_str(&answer).unwrap_or(serde_json::Value::String(answer)),
                )),
                Err(e) => CommandResult::Err(e),
            }
        }
        other => CommandResult::Err(format!("not an extension action: {other:?}")),
    }
}

/// Changes the shared settings and saves them — the same write
/// `DaemonConfig` makes, for the two settings extensions own.
fn update_settings(
    settings: &Arc<Mutex<AppSettings>>,
    change: impl FnOnce(&mut AppSettings),
) -> Result<(), String> {
    let mut guard = settings.lock();
    let before: HashSet<String> = guard.enabled_extensions.clone();
    let before_configs = guard.extension_settings.len();
    change(&mut guard);
    if guard.enabled_extensions == before && guard.extension_settings.len() == before_configs {
        return Ok(());
    }
    okena_workspace::settings::save_settings(&guard).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::calling_session;
    use crate::test_support::empty_workspace_data;
    use okena_workspace::state::Workspace;
    use parking_lot::Mutex;
    use std::sync::Arc;

    fn project(id: &str, terminal: &str, purpose: serde_json::Value) -> okena_state::ProjectData {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "name": id,
            "path": "/tmp",
            "layout": { "type": "terminal", "terminal_id": terminal },
            "agent_purpose": purpose,
        }))
        .expect("project")
    }

    #[test]
    fn only_a_session_an_extension_started_can_call_it() {
        let mut data = empty_workspace_data();
        data.projects.push(project(
            "session",
            "t-agent",
            serde_json::json!({ "kind": "extension", "extension": "cli-table", "item": "job-7" }),
        ));
        data.projects.push(project("plain", "t-plain", serde_json::Value::Null));
        let workspace = Arc::new(Mutex::new(Workspace::new(data)));

        let session = calling_session(&workspace, "t-agent").expect("an extension's session");
        assert_eq!(session.extension, "cli-table");
        assert_eq!(session.item.as_deref(), Some("job-7"));
        assert_eq!(session.project_id, "session");

        let err = calling_session(&workspace, "t-plain").expect_err("refused");
        assert!(err.contains("not started by an extension"), "{err}");
        assert!(calling_session(&workspace, "t-unknown").is_err());
    }
}
