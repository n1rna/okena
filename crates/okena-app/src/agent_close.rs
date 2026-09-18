//! Asking before an agent session's agent pane is closed.
//!
//! Closing that pane stops the agent, which closing any other terminal does
//! not, so every close that would take it — its tab's close button, a
//! middle-click, the pane menu, the shortcut, "Close Others" — is held here
//! behind a confirmation toast. The request goes on to the daemon unchanged
//! when the user confirms.

use crate::action_dispatch::ActionDispatcher;
use crate::workspace::toast::{Toast, ToastAction, ToastActionStyle, ToastManager};
use okena_core::api::ActionRequest;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// Action id for the "Close" button on the confirmation toast.
pub const CONFIRM_ACTION: &str = "agent_close_confirm";
/// Action id for the "Cancel" button on the confirmation toast.
pub const CANCEL_ACTION: &str = "agent_close_cancel";

thread_local! {
    /// Closes waiting on an answer, by the id of the toast asking.
    static PENDING: RefCell<HashMap<String, (ActionDispatcher, ActionRequest)>> =
        RefCell::new(HashMap::new());
    static NEXT: Cell<u64> = const { Cell::new(0) };
}

/// Hold `action` and ask whether to go ahead, naming the agent it would stop.
pub fn ask(
    dispatcher: ActionDispatcher,
    action: ActionRequest,
    agent: Option<String>,
    cx: &mut impl gpui::AppContext,
) {
    let toast_id = format!("agent-close-{}", NEXT.replace(NEXT.get() + 1));
    PENDING.with(|p| {
        p.borrow_mut()
            .insert(toast_id.clone(), (dispatcher, action))
    });
    let agent = agent
        .map(|a| okena_core::agents::display_name(&a))
        .unwrap_or_else(|| "the agent".to_string());
    let toast = Toast::warning("Close the agent's terminal?")
        .with_id(&toast_id)
        .with_detail(format!(
            "This stops {agent}. The session's other terminals keep running."
        ))
        .with_ttl(std::time::Duration::from_secs(30))
        .with_actions(vec![
            ToastAction::new(CONFIRM_ACTION, "Close", ToastActionStyle::Danger),
            ToastAction::new(CANCEL_ACTION, "Cancel", ToastActionStyle::Default),
        ]);
    cx.read_global::<ToastManager, _>(|_, cx| ToastManager::post(toast, cx));
}

/// The user answered the toast `toast_id`: send the held close on `confirmed`,
/// drop it otherwise.
pub fn answer(toast_id: &str, confirmed: bool, cx: &mut gpui::App) {
    ToastManager::dismiss(toast_id, cx);
    let Some((dispatcher, action)) = PENDING.with(|p| p.borrow_mut().remove(toast_id)) else {
        return;
    };
    if confirmed {
        dispatcher.dispatch_confirmed(action, cx);
    }
}
