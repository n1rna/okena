//! The extensions each connected daemon runs, as clients see them.
//!
//! Every daemon's snapshot carries its extensions; the window copies them
//! here, tagged with the connection, so the sidebar, the status bar and the
//! views all read one list — local and remote alike.

use std::sync::Arc;

use gpui::*;
use okena_core::extension::ApiExtension;
use okena_transport::remote_action::RemoteActionClient;

/// One extension, and the daemon that runs it.
#[derive(Clone, Debug, PartialEq)]
pub struct ClientExtension {
    pub connection_id: String,
    /// The connection's name, shown beside a remote daemon's extensions.
    pub connection_name: String,
    /// Run by this machine's own daemon.
    pub local: bool,
    pub ext: ApiExtension,
}

impl ClientExtension {
    /// Unique across connections.
    pub fn key(&self) -> String {
        extension_key(&self.connection_id, &self.ext.id)
    }
}

pub fn extension_key(connection_id: &str, extension_id: &str) -> String {
    format!("{connection_id}/{extension_id}")
}

/// `(connection id, extension id)` of a key made by [`extension_key`].
pub fn split_key(key: &str) -> Option<(&str, &str)> {
    key.rsplit_once('/')
}

/// A daemon the client is connected to, where extensions can be installed.
#[derive(Clone)]
pub struct ExtensionConnection {
    pub id: String,
    pub name: String,
    pub local: bool,
    /// `None` while it has no token.
    pub client: Option<RemoteActionClient>,
}

#[derive(Default)]
pub struct ExtensionsState {
    /// Shared so views clone an entry cheaply on every render.
    list: Vec<Arc<ClientExtension>>,
    connections: Vec<ExtensionConnection>,
}

impl ExtensionsState {
    pub fn list(&self) -> &[Arc<ClientExtension>] {
        &self.list
    }

    /// The connected daemons, this machine's first.
    pub fn connections(&self) -> &[ExtensionConnection] {
        &self.connections
    }

    /// The action client for the daemon `connection_id`.
    pub fn client(&self, connection_id: &str) -> Option<RemoteActionClient> {
        self.connections
            .iter()
            .find(|c| c.id == connection_id)
            .and_then(|c| c.client.clone())
    }

    /// Replaces the connections. Clients carry tokens that change without
    /// anything to draw, so this never notifies on its own.
    pub fn set_connections(&mut self, mut connections: Vec<ExtensionConnection>) {
        connections.sort_by_key(|c| !c.local);
        self.connections = connections;
    }

    pub fn get(&self, key: &str) -> Option<Arc<ClientExtension>> {
        self.list.iter().find(|e| e.key() == key).cloned()
    }

    /// The enabled extensions that have a view, in the order they arrived.
    pub fn with_views(&self) -> impl Iterator<Item = &Arc<ClientExtension>> {
        self.list
            .iter()
            .filter(|e| e.ext.enabled && e.ext.view_title.is_some())
    }

    /// Replaces the list, notifying observers only when it changed — the
    /// snapshot arrives on every state bump, most of which change nothing here.
    pub fn replace(&mut self, list: Vec<ClientExtension>, cx: &mut Context<Self>) {
        let unchanged = self.list.len() == list.len()
            && self.list.iter().zip(&list).all(|(a, b)| **a == *b);
        if !unchanged {
            self.list = list.into_iter().map(Arc::new).collect();
            cx.notify();
        }
    }
}

pub struct GlobalExtensions(pub Entity<ExtensionsState>);

impl Global for GlobalExtensions {}

pub fn extensions_entity(cx: &App) -> Option<Entity<ExtensionsState>> {
    cx.try_global::<GlobalExtensions>().map(|g| g.0.clone())
}

#[cfg(test)]
mod tests {
    use super::split_key;

    #[test]
    fn keys_split_back_into_connection_and_extension() {
        assert_eq!(split_key("local-daemon/cli-table"), Some(("local-daemon", "cli-table")));
        assert_eq!(split_key("nope"), None);
    }
}
