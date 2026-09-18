//! Agent sessions launched from extension actions.

use std::collections::HashMap;

use gpui::*;
use okena_core::extension::{AgentMode, ExtActionOutcome};
use okena_views_extensions::AgentBadge;
use okena_workspace::extensions_state::ClientExtension;
use okena_workspace::request_broker::RequestBroker;
use okena_workspace::requests::{NewAgentPrefill, OverlayRequest};

use crate::views::panels::toast::ToastManager;
use crate::workspace::state::{WindowId, Workspace};

/// Acts on an action's agent launch: opens the launcher filled in, or
/// reports the session the daemon started.
pub fn launch(
    extension: &ClientExtension,
    outcome: &ExtActionOutcome,
    broker: &Entity<RequestBroker>,
    cx: &mut App,
) {
    let Some(agent) = &outcome.agent else { return };
    match outcome.agent_mode.unwrap_or(AgentMode::Prefill) {
        AgentMode::Prefill => {
            let prefill = NewAgentPrefill {
                heading: Some(format!("Agent from {}", extension.ext.name)),
                goal: agent.goal.clone(),
                name: agent.name.clone().unwrap_or_default(),
                task: None,
            };
            broker.update(cx, |broker, cx| {
                broker.push_overlay_request(OverlayRequest::NewAgentDialog(Box::new(prefill)), cx);
            });
        }
        AgentMode::Start => {
            ToastManager::success(
                format!(
                    "{} started an agent{}",
                    extension.ext.name,
                    agent.item_label.as_deref().map(|l| format!(" on {l}")).unwrap_or_default()
                ),
                cx,
            );
        }
    }
}

/// Focuses an agent session and leaves the extension's view.
pub fn open_session(
    project_id: &str,
    window_id: WindowId,
    workspace: &Entity<Workspace>,
    focus_manager: &Entity<okena_workspace::focus::FocusManager>,
    cx: &mut App,
) {
    crate::views::components::project_nav::focus_project(workspace, focus_manager, window_id, project_id, cx);
}

/// The badges on an extension's items, by item id.
pub fn badges(
    _extension: &ClientExtension,
    _workspace: &Entity<Workspace>,
    _cx: &App,
) -> HashMap<String, AgentBadge> {
    HashMap::new()
}
