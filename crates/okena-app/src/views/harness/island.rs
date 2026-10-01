//! The search island on the harness pages (QBL-436).
//!
//! Tasks, Specs and Knowledge each float the same island the Projects and
//! Agents overviews do (`views/components/island.rs` draws it): a search box
//! and that page's filters, closing to a pill. What each page's island holds
//! is drawn by the page; this is what they share — whether it is open, where
//! the cursor goes, and what Esc does.

use crate::keybindings::shortcut_for_action;
use crate::views::components::SimpleInputState;
use crate::views::components::island::{
    island_anchor, island_close, island_filters_button, island_pill, island_stack, pill_label,
};
use gpui::prelude::*;
use gpui::*;

use super::{HarnessPane, HarnessSection};

impl HarnessPane {
    /// This page's search box — `None` when the page has no island, or
    /// nothing loaded for one to narrow.
    fn island_input(&self) -> Option<&Entity<SimpleInputState>> {
        match self.section {
            HarnessSection::Tasks => (!self.tasks.tasks.is_empty()).then_some(&self.tasks.search),
            // With no roots there is an empty state where the sidebar would
            // be, and nothing to search.
            HarnessSection::Specs => self
                .specs
                .stores
                .as_ref()
                .is_some_and(|s| !s.roots.is_empty())
                .then_some(&self.spec_search.input),
            HarnessSection::Knowledge => self
                .knowledge
                .stores
                .as_ref()
                .is_some_and(|s| !s.roots.is_empty())
                .then_some(&self.knowledge_search.input),
            HarnessSection::Testing => None,
        }
    }

    /// Whether anything narrows the page, how much it shows and how much
    /// there is.
    fn island_counts(&self) -> (bool, usize, usize) {
        match self.section {
            HarnessSection::Tasks => {
                let (shown, total) = self.tasks_shown_of_total();
                (!self.tasks.filter.is_empty(), shown, total)
            }
            section => self.doc_search(section).map_or((false, 0, 0), |search| {
                let (shown, total) = search.state.shown_of_total();
                (search.state.filter.is_active(), shown, total)
            }),
        }
    }

    /// Where the keyboard goes when the island gives it up.
    fn island_rest_focus(&self) -> FocusHandle {
        match self.section {
            // The Tasks view tracks a handle of its own.
            HarnessSection::Tasks => self.tasks.focus.clone(),
            _ => self.window_focus.clone(),
        }
    }

    /// Put the cursor in the island's search box, opening the island first if
    /// it is closed to its pill. `false` when this page has no island to
    /// focus, so the caller can let the key through.
    pub(crate) fn focus_island_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.island_input().is_none() {
            return false;
        }
        self.set_island_open(true, window, cx);
        true
    }

    /// Open or close the island. `false` when this page has none.
    pub(crate) fn toggle_island(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.island_input().is_none() {
            return false;
        }
        self.set_island_open(!self.island_open, window, cx);
        true
    }

    /// Opening puts the cursor in the box; closing hands focus back if the box
    /// had it. What the island narrows stays narrowed.
    pub(super) fn set_island_open(
        &mut self,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.island_open = open;
        if !open {
            self.island_menu_open = false;
        }
        if let Some(input) = self.island_input().cloned() {
            if open {
                input.update(cx, |input, cx| {
                    input.focus(window, cx);
                    input.select_all(cx);
                });
            } else if input.read(cx).focus_handle(cx).is_focused(window) {
                window.focus(&self.island_rest_focus(), cx);
            }
        }
        cx.notify();
    }

    /// Esc in the box: clear the text and give up focus, as on the overviews
    /// (QBL-420). On an empty box it closes the island instead.
    pub(super) fn cancel_island_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.island_input().cloned() else {
            return;
        };
        if input.read(cx).value().is_empty() {
            self.set_island_open(false, window, cx);
            return;
        }
        input.update(cx, |input, cx| input.set_value("", cx));
        window.focus(&self.island_rest_focus(), cx);
    }

    /// The Filters button, wired: it opens and shuts the filter menu.
    pub(super) fn island_filters_toggle(
        &self,
        selected: usize,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        island_filters_button("island-filters", selected, self.island_menu_open, cx).on_click(
            cx.listener(|this, _, _window, cx| {
                this.island_menu_open = !this.island_menu_open;
                cx.notify();
            }),
        )
    }

    /// The bar under its filter menu. A click anywhere outside the two shuts
    /// the menu, the way a menu is expected to go away.
    pub(super) fn island_with_menu(
        &self,
        menu: Option<AnyElement>,
        bar: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = menu.is_some();
        island_stack(menu, bar)
            .when(open, |d| {
                d.on_mouse_down_out(cx.listener(|this, _, _window, cx| {
                    this.island_menu_open = false;
                    cx.notify();
                }))
            })
            .into_any_element()
    }

    /// The island's close button, wired.
    pub(super) fn island_close_button(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        island_close(id, shortcut_for_action("ToggleOverviewSearch"), cx).on_click(cx.listener(
            |this, _, window, cx| {
                this.set_island_open(false, window, cx);
            },
        ))
    }

    /// What floats at the bottom of the page: the island when open, or the
    /// pill it closes to. Nothing on a page with no island.
    pub(super) fn render_island(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.island_input()?;
        let content = if self.island_open {
            match self.section {
                HarnessSection::Tasks => self.render_tasks_island(cx),
                section => self.render_doc_island(section, cx)?,
            }
        } else {
            let (active, shown, total) = self.island_counts();
            island_pill(
                "harness-island-pill",
                pill_label(active, shown, total),
                active,
                shortcut_for_action("FocusIslandSearch"),
                cx,
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.set_island_open(true, window, cx);
            }))
            .into_any_element()
        };
        Some(island_anchor(content).into_any_element())
    }
}

#[cfg(test)]
pub(super) mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::super::{HarnessPane, HarnessSection, PaneContext};
    use crate::settings::{GlobalSettings, SettingsState};
    use crate::workspace::focus::FocusManager;
    use crate::workspace::request_broker::RequestBroker;
    use crate::workspace::state::{WindowId, Workspace, WorkspaceData};
    use gpui::AppContext as _;
    use gpui::{Entity, FocusHandle, TestAppContext, VisualTestContext, Window};
    use okena_core::tasks::{Task, TaskId, TaskKind, TaskState};

    /// A window to focus things in, a pane of `section` in it, and the handle
    /// standing in for the window's own focus.
    pub(in super::super) fn pane_in_window(
        section: HarnessSection,
        cx: &mut TestAppContext,
    ) -> (Entity<HarnessPane>, FocusHandle, &mut VisualTestContext) {
        cx.update(|cx| {
            let settings = cx.new(|_| SettingsState::new(Default::default()));
            cx.set_global(GlobalSettings(settings));
        });
        let (_root, cx) = cx.add_window_view(|_, _| gpui::Empty);
        let (pane, window_focus) = cx.update(|_window, cx| {
            let window_focus = cx.focus_handle();
            let config = okena_transport::RemoteConnectionConfig {
                id: "test".into(),
                name: "test".into(),
                host: "127.0.0.1".into(),
                port: 1,
                saved_token: None,
                token_obtained_at: None,
                tls: false,
                pinned_cert_sha256: None,
                local_endpoint: None,
            };
            let ctx = PaneContext {
                client: okena_transport::remote_action::RemoteActionClient::new(config, "t".into()),
                request_broker: cx.new(|_| RequestBroker::new()),
                workspace: cx.new(|_| Workspace::new(WorkspaceData::empty())),
                focus_manager: cx.new(|_| FocusManager::new()),
                window_id: WindowId::Main,
                terminals: Default::default(),
                active_drag: Default::default(),
                window_focus: window_focus.clone(),
            };
            (
                cx.new(|cx| HarnessPane::new(section, ctx, cx)),
                window_focus,
            )
        });
        (pane, window_focus, cx)
    }

    fn task(key: &str, title: &str) -> Task {
        Task {
            id: TaskId::new("linear", key),
            display_key: key.into(),
            title: title.into(),
            description: None,
            state: TaskState::Todo,
            state_name: "Todo".into(),
            url: String::new(),
            branch_name: String::new(),
            updated_at: String::new(),
            kind: TaskKind::Task,
            parent_id: None,
            parent_key: None,
            labels: Vec::new(),
            groups: Vec::new(),
        }
    }

    fn tasks_pane(cx: &mut TestAppContext) -> (Entity<HarnessPane>, &mut VisualTestContext) {
        let (pane, _, cx) = pane_in_window(HarnessSection::Tasks, cx);
        pane.update(cx, |p, _| {
            p.tasks.tasks = vec![task("QBL-1", "Search box"), task("QBL-2", "Refresh")];
        });
        (pane, cx)
    }

    fn search_focused(pane: &HarnessPane, window: &Window, cx: &gpui::App) -> bool {
        pane.tasks
            .search
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    }

    #[gpui::test]
    fn focusing_the_search_opens_a_closed_island_and_takes_the_cursor(cx: &mut TestAppContext) {
        let (pane, cx) = tasks_pane(cx);
        cx.update(|window, cx| {
            pane.update(cx, |p, cx| {
                p.set_island_open(false, window, cx);
                assert!(!p.island_open);
                assert!(!search_focused(p, window, cx));

                assert!(p.focus_island_search(window, cx));
                assert!(p.island_open, "the shortcut shows a closed island first");
                assert!(search_focused(p, window, cx));
            });
        });
    }

    #[gpui::test]
    fn typing_in_the_island_narrows_the_task_list(cx: &mut TestAppContext) {
        let (pane, cx) = tasks_pane(cx);
        pane.update(cx, |p, cx| {
            p.tasks
                .search
                .update(cx, |i, cx| i.set_value("  REFRESH ", cx));
        });
        cx.run_until_parked();
        pane.update(cx, |p, _| {
            assert_eq!(p.tasks_shown_of_total(), (1, 2));
            assert_eq!(p.island_counts(), (true, 1, 2));
        });
    }

    #[gpui::test]
    fn escape_clears_the_text_and_gives_up_focus_then_closes_when_empty(cx: &mut TestAppContext) {
        let (pane, cx) = tasks_pane(cx);
        cx.update(|window, cx| {
            pane.update(cx, |p, cx| {
                p.focus_island_search(window, cx);
                p.tasks.search.update(cx, |i, cx| i.set_value("qbl-1", cx));

                p.cancel_island_search(window, cx);
                assert_eq!(p.tasks.search.read(cx).value(), "");
                assert!(!search_focused(p, window, cx), "Esc gives up focus");
                assert!(p.tasks.focus.is_focused(window));
                assert!(p.island_open, "clearing text is not closing");

                // A second Esc, on the now empty box, closes it to the pill.
                p.focus_island_search(window, cx);
                p.cancel_island_search(window, cx);
                assert!(!p.island_open);
                assert!(!search_focused(p, window, cx));
            });
        });
        cx.run_until_parked();
        pane.update(cx, |p, _| assert!(p.tasks.filter.is_empty()));
    }

    #[gpui::test]
    fn the_filter_menu_starts_shut_and_goes_away_with_the_island(cx: &mut TestAppContext) {
        let (pane, cx) = tasks_pane(cx);
        cx.update(|window, cx| {
            pane.update(cx, |p, cx| {
                assert!(!p.island_menu_open, "filters are behind the button");
                p.island_menu_open = true;
                // Closing the island to its pill must not leave a menu
                // waiting to reappear with it.
                p.set_island_open(false, window, cx);
                assert!(!p.island_menu_open);
                p.focus_island_search(window, cx);
                assert!(p.island_open && !p.island_menu_open);
            });
        });
    }

    #[gpui::test]
    fn a_page_with_nothing_to_narrow_has_no_island(cx: &mut TestAppContext) {
        // No tasks loaded, and Testing never has one: the key is let through.
        let (pane, _, cx) = pane_in_window(HarnessSection::Tasks, cx);
        cx.update(|window, cx| {
            pane.update(cx, |p, cx| {
                assert!(!p.focus_island_search(window, cx));
                assert!(!p.toggle_island(window, cx));
            });
        });
    }
}
