//! Native views for extensions run in the daemon.
//!
//! okena draws everything an extension shows: the extension returns a
//! declarative view (`okena_core::extension::ExtView`) and okena renders it
//! with its own components. Nothing here runs extension code.
//!
//! - [`pane::ExtensionPane`] is an extension's own view in the main area.
//! - [`table`] arranges tables: grouping, sorting, filtering, selection.
//! - [`manage::ExtensionsManager`] installs, approves, configures, updates
//!   and removes them, in Settings → Extensions.

#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

pub mod manage;
pub mod pane;
mod render;
pub mod table;

pub use manage::ExtensionsManager;
pub use pane::{ExtensionPane, ExtensionPaneEvent, extension_by_key};

/// The Settings page extensions are managed on; its section is an
/// extension's key, to open with its details showing.
pub const SETTINGS_PAGE: &str = "extensions";
pub use render::tone_color;

/// The badges on an extension's items, by item id, which the app works out
/// from its agent sessions.
pub type AgentBadgesFn = std::sync::Arc<
    dyn Fn(&okena_workspace::extensions_state::ClientExtension, &gpui::App)
        -> std::collections::HashMap<String, AgentBadge>,
>;

/// The live badge on an item an agent session was started from.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentBadge {
    /// The session, to open on click.
    pub project_id: String,
    /// "working", "needs you", "done"…
    pub label: String,
    pub color: u32,
}
