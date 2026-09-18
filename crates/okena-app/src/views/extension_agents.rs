//! Agent sessions launched from extension actions: the launcher an action
//! fills in, the badges on the items sessions were started from, and the
//! confirmations agents' destructive calls wait on.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use gpui::*;
use okena_core::api::ActionRequest;
use okena_core::extension::{AgentMode, ExtActionOutcome};
use okena_core::harness::AgentPurpose;
use okena_views_extensions::AgentBadge;
use okena_workspace::extensions_state::{ClientExtension, ExtensionsState, extensions_entity};
use okena_workspace::request_broker::RequestBroker;
use okena_workspace::requests::{NewAgentPrefill, OverlayRequest};

use crate::views::agent_session::model::AgentSessionInfo;
use crate::views::panels::toast::ToastManager;
use crate::workspace::state::{WindowId, Workspace};
use crate::workspace::toast::{Toast, ToastAction, ToastActionStyle};

const CONFIRM: &str = "ext_confirm_yes";
const DENY: &str = "ext_confirm_no";
/// The toast stays as long as the daemon waits for an answer.
const CONFIRM_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

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
                heading: Some(match &agent.item_label {
                    Some(item) => format!("{}: {item}", extension.ext.name),
                    None => format!("Agent from {}", extension.ext.name),
                }),
                goal: agent.goal.clone(),
                name: agent.name.clone().unwrap_or_default(),
                task: None,
                root: agent.root.clone().unwrap_or_default(),
                project_ids: agent.project_ids.clone(),
                context: agent.context.clone(),
                purpose: Some(AgentPurpose::Extension {
                    extension: extension.ext.id.clone(),
                    item: agent.item.clone(),
                    item_label: agent.item_label.clone(),
                }),
                connection_id: Some(extension.connection_id.clone()),
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

/// Whether the project `id` (as this client names it) is on `extension`'s daemon.
fn on_connection(project_id: &str, extension: &ClientExtension) -> bool {
    if extension.local {
        !project_id.starts_with("remote:")
    } else {
        project_id.starts_with(&format!("remote:{}:", extension.connection_id))
    }
}

/// The live badges on an extension's items: the latest open session started
/// from each item, with what it is doing.
pub fn badges(
    extension: &ClientExtension,
    workspace: &Entity<Workspace>,
    terminals: &okena_terminal::TerminalsRegistry,
    cx: &App,
) -> HashMap<String, AgentBadge> {
    let t = crate::theme::theme(cx);
    let ws = workspace.read(cx);
    let mut out = HashMap::new();
    for project in ws.projects() {
        let Some(AgentPurpose::Extension {
            extension: id,
            item: Some(item),
            ..
        }) = &project.agent_purpose
        else {
            continue;
        };
        if id != &extension.ext.id || project.is_closed() || !on_connection(&project.id, extension) {
            continue;
        }
        let Some(info) = AgentSessionInfo::collect(ws, terminals, &project.id) else {
            continue;
        };
        let activity = info.activity();
        // Later sessions replace earlier ones: projects list in creation order.
        out.insert(
            item.clone(),
            AgentBadge {
                project_id: project.id.clone(),
                label: activity.label().to_string(),
                color: activity.color(&t),
            },
        );
    }
    out
}

thread_local! {
    /// Confirmations already put to the user, by confirmation id, so every
    /// window and every snapshot asks once.
    static ASKED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// What each confirmation toast answers: (connection, extension, confirmation).
    static TOASTS: RefCell<HashMap<String, (String, String, String)>> = RefCell::new(HashMap::new());
}

/// Asks the user about every destructive action an agent is waiting on,
/// once each. Called whenever the extensions change.
pub fn ask_pending_confirmations(state: &Entity<ExtensionsState>, cx: &mut App) {
    let list = state.read(cx).list().to_vec();
    for ext in &list {
        for request in &ext.ext.pending_confirmations {
            let new = ASKED.with(|asked| asked.borrow_mut().insert(request.id.clone()));
            if !new {
                continue;
            }
            let toast_id = format!("ext-confirm-{}", request.id);
            TOASTS.with(|toasts| {
                toasts.borrow_mut().insert(
                    toast_id.clone(),
                    (ext.connection_id.clone(), ext.ext.id.clone(), request.id.clone()),
                )
            });
            let target = match request.items.len() {
                0 => String::new(),
                1 => format!(" on {}", request.items[0]),
                n => format!(" on {n} items"),
            };
            let toast = Toast::warning(format!(
                "An agent asks {} to run {}{target}",
                ext.ext.name, request.action_label
            ))
            .with_id(&toast_id)
            .with_detail("It is marked destructive, so it waits for you. Declining tells the agent no.")
            .with_ttl(CONFIRM_TTL)
            .with_actions(vec![
                ToastAction::new(CONFIRM, format!("Run {}", request.action_label), ToastActionStyle::Danger),
                ToastAction::new(DENY, "Decline", ToastActionStyle::Default),
            ]);
            ToastManager::post(toast, cx);
        }
    }
}

pub fn is_confirm_action(action_id: &str) -> bool {
    action_id == CONFIRM || action_id == DENY
}

/// The user answered a confirmation toast.
pub fn answer(toast_id: &str, action_id: &str, cx: &mut App) {
    ToastManager::dismiss(toast_id, cx);
    let Some((connection, extension, confirmation)) =
        TOASTS.with(|toasts| toasts.borrow_mut().remove(toast_id))
    else {
        return;
    };
    confirm(&connection, extension, confirmation, action_id == CONFIRM, cx);
}

/// Sends the user's answer to the daemon waiting on it.
pub fn confirm(connection: &str, extension: String, confirmation: String, approve: bool, cx: &mut App) {
    let Some(client) = extensions_entity(cx).and_then(|e| e.read(cx).client(connection)) else {
        ToastManager::error("The daemon asking is no longer connected", cx);
        return;
    };
    cx.spawn(async move |cx| {
        let result = smol::unblock(move || {
            client.post_action(ActionRequest::ExtensionConfirm {
                id: extension,
                confirmation,
                approve,
            })
        })
        .await;
        if let Err(e) = result {
            cx.update(|cx| ToastManager::error(e, cx));
        }
    })
    .detach();
}
