//! The daemon's launch-context index and the actions over it (QBL-406).
//!
//! One [`ContextIndex`] lives as long as the command loop. Every action is run
//! off the workspace lock on a snapshot of the projects and settings: a search
//! runs discovery and may read roots nobody has searched yet.

use okena_app_core::workspace::actions::execute::{context_catalog, read_in_scope, session_scope};
use okena_context::index::{ContextIndex, DEFAULT_LIMIT};
use okena_core::api::{ActionRequest, CommandResult};
use okena_workspace::settings::AppSettings;
use okena_workspace::state::ProjectData;

/// Most results one search may ask for.
const MAX_LIMIT: usize = 500;

/// The index, with frecency kept in the active profile's directory.
pub(crate) fn open_index() -> ContextIndex {
    let dir = okena_core::profiles::try_current().map(|p| p.root.join("context-frecency"));
    ContextIndex::open(dir.as_deref())
}

pub(crate) fn run(
    index: &ContextIndex,
    action: &ActionRequest,
    projects: &[ProjectData],
    settings: &AppSettings,
) -> CommandResult {
    match action {
        ActionRequest::ContextSearch {
            query,
            project_ids,
            terminal_id,
            limit,
        } => {
            // An agent's own lookup is scoped to its session, whatever it
            // claims to have chosen.
            let (chosen, scoped) = match terminal_id {
                Some(terminal) => match session_scope(projects, terminal) {
                    Ok(scope) => (scope, true),
                    Err(e) => return CommandResult::Err(e),
                },
                None => (project_ids.clone(), false),
            };
            let catalog = context_catalog(projects, settings);
            let limit = limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
            let result = index.search(&catalog, query, &chosen, scoped, limit);
            match serde_json::to_value(result) {
                Ok(v) => CommandResult::Ok(Some(v)),
                Err(e) => CommandResult::Err(e.to_string()),
            }
        }
        ActionRequest::ContextHit { item } => {
            index.record_hit(&context_catalog(projects, settings), item);
            CommandResult::Ok(None)
        }
        ActionRequest::ContextRead { terminal_id, path } => {
            match session_scope(projects, terminal_id) {
                Ok(scope) => read_in_scope(&context_catalog(projects, settings), &scope, path)
                    .into_command_result(),
                Err(e) => CommandResult::Err(e),
            }
        }
        _ => CommandResult::Err("not a context action".into()),
    }
}
