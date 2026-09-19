//! Which services the bar shows, kept in the extension's settings as
//! `{ "services": ["claude", "github"] }`.

use crate::services::ServiceId;
use gpui::{App, BorrowAppContext};
use okena_extensions::ExtensionSettingsStore;

/// The extension's id, and its settings namespace.
pub const EXTENSION_ID: &str = "status";

/// Shown before anything has been chosen.
const DEFAULT: [ServiceId; 2] = [ServiceId::Claude, ServiceId::GitHub];

/// The chosen services, in the bar's order.
pub fn selected(cx: &App) -> Vec<ServiceId> {
    let chosen = cx
        .try_global::<ExtensionSettingsStore>()
        .and_then(|store| store.get(EXTENSION_ID, cx))
        .and_then(|settings| settings.get("services").cloned());
    match chosen.as_ref().and_then(|v| v.as_array()) {
        Some(slugs) => {
            let slugs: Vec<&str> = slugs.iter().filter_map(|s| s.as_str()).collect();
            ServiceId::ALL
                .into_iter()
                .filter(|s| slugs.contains(&s.slug()))
                .collect()
        }
        None => DEFAULT.to_vec(),
    }
}

/// Show or hide `service` on the bar.
pub fn set_selected(service: ServiceId, on: bool, cx: &mut App) {
    let mut chosen = selected(cx);
    chosen.retain(|s| *s != service);
    if on {
        chosen.push(service);
    }
    let slugs: Vec<&str> = ServiceId::ALL
        .into_iter()
        .filter(|s| chosen.contains(s))
        .map(ServiceId::slug)
        .collect();
    let mut settings = cx
        .try_global::<ExtensionSettingsStore>()
        .and_then(|store| store.get(EXTENSION_ID, cx))
        .filter(|v| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    settings["services"] = serde_json::json!(slugs);
    ExtensionSettingsStore::update(EXTENSION_ID, settings, cx);
    // The store does not announce writes; tell whoever observes it.
    cx.update_global::<ExtensionSettingsStore, _>(|_, _| {});
}
