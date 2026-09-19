#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

//! Usage: how much of each coding agent's limits you have used, side by side
//! on the status bar. Which agents show is chosen in its settings.

mod bar;
pub mod claude;
mod codex;
mod copilot;
pub mod selection;
mod settings;
mod util;

pub use claude::resolve_claude_dir;

use gpui::{AnyView, AppContext as _};
use okena_extensions::{ExtensionInstance, ExtensionManifest, ExtensionRegistration};
use std::sync::Arc;

pub fn register() -> ExtensionRegistration {
    ExtensionRegistration {
        manifest: ExtensionManifest {
            id: selection::EXTENSION_ID,
            name: "Usage",
            default_enabled: false,
        },
        activate: Arc::new(|app| {
            let bar = app.new(bar::UsageBar::new);
            ExtensionInstance {
                status_bar_widgets: vec![bar.into()],
                status_bar_right_widgets: vec![],
            }
        }),
        settings_view: Some(Arc::new(|app| {
            AnyView::from(app.new(settings::UsageSettingsView::new))
        })),
    }
}
