#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

//! Status: whether the services you work with are up, side by side on the
//! status bar. Which services show is chosen in its settings.

mod poller;
pub mod selection;
pub mod services;
mod settings;
mod widget;

use gpui::{AnyView, AppContext as _};
use okena_extensions::{ExtensionInstance, ExtensionManifest, ExtensionRegistration};
use std::sync::Arc;

pub fn register() -> ExtensionRegistration {
    ExtensionRegistration {
        manifest: ExtensionManifest {
            id: selection::EXTENSION_ID,
            name: "Status",
            default_enabled: false,
        },
        activate: Arc::new(|app| {
            let group = app.new(widget::StatusGroup::new);
            ExtensionInstance {
                status_bar_widgets: vec![group.into()],
                status_bar_right_widgets: vec![],
            }
        }),
        settings_view: Some(Arc::new(|app| {
            AnyView::from(app.new(settings::StatusSettingsView::new))
        })),
    }
}
