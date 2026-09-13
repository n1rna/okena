//! Harness views in the main content area.
//!
//! Exactly one view shows at a time and it takes the whole main area, replacing
//! the projects grid. Selecting the active entry again is a no-op rather than a
//! toggle — the view must not vanish under a user who clicks it twice. The way
//! back to the terminal workspace is the view's own close button.
//!
//! Pane entities are kept alive after being switched away from, so returning to
//! a view restores what it had loaded instead of refetching.

use crate::views::harness::HarnessPane;
use gpui::*;
use okena_core::harness::HarnessSection;

use super::WindowView;

impl WindowView {
    /// Show `section` full-width. A no-op if it is already showing.
    pub(crate) fn show_harness_view(&mut self, section: HarnessSection, cx: &mut Context<Self>) {
        if okena_workspace::harness_state::active_harness(self.window_id, cx) == Some(section) {
            return;
        }
        if !self.harness_panes.iter().any(|(s, _)| *s == section) {
            let client = match self.local_daemon_action_client(cx) {
                Ok(client) => client,
                Err(error) => {
                    crate::views::panels::toast::ToastManager::error(error, cx);
                    return;
                }
            };
            let ctx = crate::views::harness::PaneContext {
                client,
                request_broker: self.request_broker.clone(),
                workspace: self.workspace.clone(),
                focus_manager: self.focus_manager.clone(),
                window_id: self.window_id,
                terminals: self.terminals.clone(),
                active_drag: self.active_drag.clone(),
            };
            let pane = cx.new(|cx| HarnessPane::new(section, ctx, cx));
            self.harness_panes.push((section, pane));
        }
        okena_workspace::harness_state::set_active_harness(self.window_id, Some(section), cx);
        cx.notify();
    }

    /// Reopen the harness view this window showed when okena last closed.
    ///
    /// A no-op until the local daemon connection is known: a pane cannot be
    /// built without its client, and trying earlier would toast an error about
    /// a view nobody just asked for.
    pub(crate) fn restore_harness_view(&mut self, cx: &mut Context<Self>) {
        let Some(section) = self.pending_harness_restore else {
            return;
        };
        if self.local_daemon_action_client(cx).is_err() {
            return;
        }
        self.pending_harness_restore = None;
        self.show_harness_view(section, cx);
    }

    /// The pane filling the main area, if a harness view is showing.
    pub(crate) fn active_harness_pane(&self, cx: &App) -> Option<Entity<HarnessPane>> {
        let section = okena_workspace::harness_state::active_harness(self.window_id, cx)?;
        self.harness_panes
            .iter()
            .find(|(s, _)| *s == section)
            .map(|(_, pane)| pane.clone())
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use crate::workspace::state::{WindowId, WorkspaceData};
    use okena_core::harness::HarnessSection;

    #[test]
    fn every_section_reaches_the_nav_the_views_and_the_saved_layout() {
        // The nav, the pane's views and the window layout each read
        // `HarnessSection::all()` on their own. The pane's render is an
        // exhaustive match, so a new section cannot compile without a view;
        // this covers the two that would drift at runtime instead.
        let nav = okena_views_sidebar::sidebar::harness_nav_entries();
        assert_eq!(
            nav.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            HarnessSection::all().to_vec(),
            "the nav lists every section, in order"
        );
        let mut ids: Vec<_> = nav.iter().map(|(_, id)| id.clone()).collect();
        ids.dedup();
        assert_eq!(ids.len(), nav.len(), "nav entry ids are distinct");

        for section in HarnessSection::all() {
            // Selected in a window, saved, and read back after a restart.
            let mut data = WorkspaceData::empty();
            data.set_harness_section(WindowId::Main, Some(section));
            let saved = serde_json::to_string(&data.main_window).expect("save layout");
            let restored: crate::workspace::state::WindowState =
                serde_json::from_str(&saved).expect("load layout");
            assert_eq!(restored.harness_section, Some(section), "{section:?}");
        }
    }
}
