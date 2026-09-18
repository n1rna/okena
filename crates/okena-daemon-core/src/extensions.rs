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
use okena_core::extension::Invoker;
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
