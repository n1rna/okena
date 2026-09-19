//! Extensions' own views in the main content area, and the list of
//! extensions every client-side surface reads.
//!
//! An extension's view replaces the projects grid the way a harness view
//! does, and one excludes the other. Panes are kept after being switched
//! away from, so a table keeps its grouping and selection. The Extensions
//! page shows in the same place, under a key no extension can have.

use std::sync::Arc;

use gpui::*;
use okena_views_extensions::{AgentBadgesFn, ExtensionPane, ExtensionPaneEvent};
use okena_workspace::harness_state::EXTENSIONS_PAGE;
use okena_workspace::extensions_state::{
    ClientExtension, ExtensionConnection, extensions_entity,
};

use super::WindowView;
use crate::views::extensions_page::ExtensionsPage;

impl WindowView {
    /// Show the extension `key` full-width.
    pub(crate) fn show_extension_view(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.extension_panes.contains_key(&key) {
            let broker = self.request_broker.clone();
            let badges = agent_badges_fn(self.workspace.clone(), self.terminals.clone());
            let pane = cx.new(|cx| ExtensionPane::new(key.clone(), broker, badges, cx));
            cx.subscribe(&pane, |this, _pane, event: &ExtensionPaneEvent, cx| {
                this.on_extension_pane_event(event, cx);
            })
            .detach();
            self.extension_panes.insert(key.clone(), pane);
        }
        okena_workspace::harness_state::set_active_extension(self.window_id, Some(key), cx);
        cx.notify();
    }

    /// Show the Extensions page full-width, with `open`'s details showing.
    pub(crate) fn show_extensions_page(&mut self, open: Option<String>, cx: &mut Context<Self>) {
        match &self.extensions_page {
            Some(page) => {
                if let Some(key) = open {
                    page.update(cx, |page, cx| page.open(key, cx));
                }
            }
            None => {
                self.extensions_page = Some(cx.new(|cx| ExtensionsPage::new(open, cx)));
            }
        }
        okena_workspace::harness_state::set_active_extension(
            self.window_id,
            Some(EXTENSIONS_PAGE.to_string()),
            cx,
        );
        cx.notify();
    }

    /// The Extensions page, if it is what the main area shows.
    pub(crate) fn active_extensions_page(&self, cx: &App) -> Option<Entity<ExtensionsPage>> {
        let key = okena_workspace::harness_state::active_extension(self.window_id, cx)?;
        (key == EXTENSIONS_PAGE).then(|| self.extensions_page.clone()).flatten()
    }

    /// The extension pane filling the main area, if one is showing.
    pub(crate) fn active_extension_pane(&self, cx: &App) -> Option<Entity<ExtensionPane>> {
        let key = okena_workspace::harness_state::active_extension(self.window_id, cx)?;
        self.extension_panes.get(&key).cloned()
    }

    fn on_extension_pane_event(&mut self, event: &ExtensionPaneEvent, cx: &mut Context<Self>) {
        match event {
            ExtensionPaneEvent::AgentLaunch { extension, outcome } => {
                crate::views::extension_agents::launch(
                    extension,
                    outcome,
                    &self.request_broker,
                    cx,
                );
            }
            ExtensionPaneEvent::OpenSession { project_id } => {
                crate::views::extension_agents::open_session(
                    project_id,
                    self.window_id,
                    &self.workspace,
                    &self.focus_manager,
                    cx,
                );
            }
        }
    }
}

/// Copies every connection's extensions out of their snapshots into the
/// shared list, and each connection's action client beside them.
pub(super) fn sync_extensions(
    connections: &[(
        okena_transport::client::RemoteConnectionConfig,
        Option<okena_core::api::StateResponse>,
    )],
    cx: &mut App,
) {
    let Some(entity) = extensions_entity(cx) else {
        return;
    };
    let mut list = Vec::new();
    let mut infos = Vec::new();
    for (config, state) in connections {
        let local = config.id == okena_transport::client::LOCAL_DAEMON_CONNECTION_ID;
        let client = config.effective_auth_token().map(|token| {
            okena_transport::remote_action::RemoteActionClient::new(config.clone(), token)
        });
        infos.push(ExtensionConnection {
            id: config.id.clone(),
            name: config.name.clone(),
            local,
            client,
        });
        for ext in state.iter().flat_map(|s| s.extensions.iter()) {
            list.push(ClientExtension {
                connection_id: config.id.clone(),
                connection_name: config.name.clone(),
                local,
                ext: ext.clone(),
            });
        }
    }
    entity.update(cx, |state, cx| {
        state.set_connections(infos);
        state.replace(list, cx);
    });
}

/// Badges for the rows agent sessions were started from.
fn agent_badges_fn(
    workspace: Entity<crate::workspace::state::Workspace>,
    terminals: okena_terminal::TerminalsRegistry,
) -> AgentBadgesFn {
    Arc::new(move |ext, cx| crate::views::extension_agents::badges(ext, &workspace, &terminals, cx))
}
