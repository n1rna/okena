//! The search island under the Projects and Agents overviews.
//!
//! Narrows the list `Workspace::visible_projects` already produced, in the
//! view only: what counts as visible — for git polling, focus and scroll
//! restore — is unchanged. The rules live in `overview_filter`; this is the
//! state per window and the drawing.
//!
//! The island sits centred at the bottom of the overview and closes, from its
//! own button or `ToggleOverviewSearch`, to a pill naming the shortcut that
//! opens it again. It floats over the grid and takes no space of its own.
//! `FocusIslandSearch` (Cmd/Ctrl+F outside a terminal) puts the cursor in it
//! here and on the harness pages, whose islands `views/harness/island.rs`
//! holds.

use crate::keybindings::{Cancel, shortcut_for_action};
use crate::theme::theme;
use crate::ui::tokens::{ui_text_md, ui_text_xl};
use crate::views::components::SimpleInputState;
use crate::views::components::island::{
    island_anchor, island_bar, island_button, island_chip, island_close, island_count,
    island_filters_button, island_menu, island_menu_group, island_pill, island_search_box,
    island_stack, pill_label,
};
use gpui::prelude::*;
use gpui::*;
use gpui_component::v_flex;
use okena_core::agent_activity::AgentActivity;
use okena_views_sidebar::agent_card::card_state;

use super::WindowView;
use super::overview_filter::{
    AgentFacts, Candidate, Facets, OverviewFilter, apply, collect_facets, state_label,
};
use crate::workspace::state::AgentRole;

/// One overview's search box and what it is narrowed to. Each window holds
/// one per overview, so switching between them keeps both. Never saved: a
/// restart starts empty.
pub(super) struct OverviewSearch {
    pub input: Entity<SimpleInputState>,
    pub filter: OverviewFilter,
}

impl OverviewSearch {
    /// A box that narrows the grid as you type — the rows are all there is to
    /// search, so there is nothing to ask the daemon.
    pub fn new(agents: bool, cx: &mut Context<WindowView>) -> Self {
        let placeholder = if agents {
            "Search agents"
        } else {
            "Search projects"
        };
        let input = cx.new(|cx| SimpleInputState::new(cx).placeholder(placeholder));
        cx.subscribe(
            &input,
            move |this: &mut WindowView,
                  input,
                  _: &okena_ui::simple_input::InputChangedEvent,
                  cx| {
                let text = input.read(cx).value().to_string();
                this.overview_search_mut(agents).filter.set_search(&text);
                cx.notify();
            },
        )
        .detach();
        Self {
            input,
            filter: OverviewFilter::default(),
        }
    }
}

/// The grid's list after the bar has had its say.
pub(super) struct NarrowedOverview {
    /// What the grid draws, in order.
    pub shown: Vec<String>,
    /// How many there were before the bar narrowed them.
    pub total: usize,
    pub facets: Facets,
}

impl WindowView {
    fn overview_search(&self, agents: bool) -> &OverviewSearch {
        if agents {
            &self.agents_search
        } else {
            &self.projects_search
        }
    }

    fn overview_search_mut(&mut self, agents: bool) -> &mut OverviewSearch {
        if agents {
            &mut self.agents_search
        } else {
            &mut self.projects_search
        }
    }

    /// Which overview the grid is showing, `Some(true)` for Agents — or `None`
    /// while a project is focused or fullscreen, which is not an overview and
    /// has no bar.
    pub(super) fn shown_overview(&self, cx: &App) -> Option<bool> {
        let fm = self.focus_manager.read(cx);
        if fm.focused_project_id().is_some() || fm.fullscreen_project_id().is_some() {
            return None;
        }
        Some(
            self.workspace
                .read(cx)
                .data()
                .window(self.window_id)
                .is_some_and(|w| w.agents_overview),
        )
    }

    /// Narrow `ids` — the grid's list, in order — by this overview's bar.
    ///
    /// Selections no session has any more are ignored here and dropped by the
    /// next render (`prune_overview_filter`), so a chip that went stale cannot
    /// empty the grid in between.
    pub(super) fn narrow_overview(
        &self,
        agents: bool,
        ids: &[String],
        cx: &App,
    ) -> NarrowedOverview {
        let workspace = self.workspace.read(cx);
        let registry = self.terminals.lock();
        let candidates: Vec<Candidate> = ids
            .iter()
            .filter_map(|id| workspace.project(id))
            .map(|p| Candidate {
                project: p,
                branch: workspace
                    .remote_snapshot(&p.id)
                    .and_then(|s| s.git_status.as_ref())
                    .and_then(|g| g.branch.as_deref()),
                agent: agents.then(|| {
                    // What the agent's card says: stopped when nothing runs,
                    // otherwise what the daemon decided.
                    let live: Vec<String> = p
                        .layout
                        .as_ref()
                        .map(|l| l.collect_terminal_ids())
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|tid| registry.contains_key(tid))
                        .collect();
                    let activity = live
                        .iter()
                        .find_map(|tid| workspace.agent_activity(&p.id, tid));
                    AgentFacts {
                        state: card_state(!live.is_empty(), activity),
                        role: p.agent_role(),
                    }
                }),
            })
            .collect();
        let facets = collect_facets(&candidates);
        let mut filter = self.overview_search(agents).filter.clone();
        filter.prune(&facets);
        NarrowedOverview {
            shown: apply(&filter, &candidates),
            total: candidates.len(),
            facets,
        }
    }

    /// Drop chips nothing on offer has, so a finished agent does not leave a
    /// selection behind.
    pub(super) fn prune_overview_filter(&mut self, agents: bool, facets: &Facets) {
        self.overview_search_mut(agents).filter.prune(facets);
    }

    fn clear_overview_filter(&mut self, agents: bool, cx: &mut Context<Self>) {
        let search = self.overview_search_mut(agents);
        search.filter.clear();
        let input = search.input.clone();
        input.update(cx, |input, cx| input.set_value("", cx));
        cx.notify();
    }

    /// The overview whose island is on screen — `None` when a harness view,
    /// an extension's view or a single project fills the main area instead.
    fn overview_with_island(&self, cx: &App) -> Option<bool> {
        if self.active_harness_pane(cx).is_some()
            || self.active_extension_pane(cx).is_some()
            || self.active_extensions_page(cx).is_some()
        {
            return None;
        }
        self.shown_overview(cx)
    }

    /// Open or close the island of the page on screen. Opening puts the
    /// cursor in its box; closing hands focus back if the box had it. What it
    /// narrows stays narrowed.
    pub(super) fn toggle_overview_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = self.active_harness_pane(cx) {
            pane.update(cx, |pane, cx| pane.toggle_island(window, cx));
            return;
        }
        let Some(agents) = self.overview_with_island(cx) else {
            return;
        };
        self.set_overview_search_open(agents, !self.overview_search_open, window, cx);
    }

    /// Cmd/Ctrl+F: put the cursor in the island's search box on whichever
    /// page has one — Tasks, Specs, Knowledge, Projects or Agents — opening
    /// the island first if it is closed to its pill.
    ///
    /// A focused terminal never gets here: the binding is scoped outside
    /// terminal panes, so its Cmd/Ctrl+F stays its own search. Anywhere the
    /// key is not the island's to take — a modal is up, or the page has no
    /// island — it is let through, so a viewer's own find still hears it.
    pub(super) fn focus_island_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.focus_manager.read(cx).is_modal() {
            cx.propagate();
            return;
        }
        if let Some(pane) = self.active_harness_pane(cx) {
            if !pane.update(cx, |pane, cx| pane.focus_island_search(window, cx)) {
                cx.propagate();
            }
            return;
        }
        match self.overview_with_island(cx) {
            Some(agents) => self.set_overview_search_open(agents, true, window, cx),
            None => cx.propagate(),
        }
    }

    fn set_overview_search_open(
        &mut self,
        agents: bool,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overview_search_open = open;
        if !open {
            self.overview_menu_open = false;
        }
        let input = self.overview_search(agents).input.clone();
        if open {
            input.update(cx, |input, cx| {
                input.focus(window, cx);
                input.select_all(cx);
            });
        } else if input.read(cx).focus_handle(cx).is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }
        cx.notify();
    }

    /// What floats at the bottom of the grid: the island when open — the
    /// search box, on Agents the Filters button whose menu holds the state and
    /// role chips on offer, then "N of M" and Clear while anything narrows it —
    /// or the pill it closes to.
    pub(super) fn render_overview_island(
        &self,
        agents: bool,
        narrowed: &NarrowedOverview,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let content = if self.overview_search_open {
            self.render_island(agents, narrowed, cx)
        } else {
            self.render_island_pill(agents, narrowed, cx)
        };
        island_anchor(content).into_any_element()
    }

    fn render_island(
        &self,
        agents: bool,
        narrowed: &NarrowedOverview,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let search = self.overview_search(agents);
        let active = search.filter.is_active();

        let mut row = island_bar("overview-island", cx).child(
            island_search_box("overview-search", &search.input, cx)
                // Esc clears the text and hands focus back; on an empty
                // box it closes the island.
                .on_action(cx.listener(move |this, _: &Cancel, window, cx| {
                    let input = this.overview_search(agents).input.clone();
                    if input.read(cx).value().is_empty() {
                        this.set_overview_search_open(agents, false, window, cx);
                        return;
                    }
                    input.update(cx, |input, cx| input.set_value("", cx));
                    window.focus(&this.focus_handle, cx);
                })),
        );

        // Only Agents has filters, and only while some agent offers a value:
        // a button that opens an empty menu is worse than no button.
        let has_filters =
            agents && !(narrowed.facets.states.is_empty() && narrowed.facets.roles.is_empty());
        if has_filters {
            row = row.child(
                island_filters_button(
                    "overview-filters",
                    search.filter.selected_count(),
                    self.overview_menu_open,
                    cx,
                )
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.overview_menu_open = !this.overview_menu_open;
                    cx.notify();
                })),
            );
        }

        let bar = row
            .children(active.then(|| island_count(narrowed.shown.len(), narrowed.total, cx)))
            .children(active.then(|| {
                island_button("overview-clear", "Clear", cx).on_click(cx.listener(
                    move |this, _, _window, cx| {
                        this.clear_overview_filter(agents, cx);
                    },
                ))
            }))
            .child(
                island_close(
                    "overview-island-close",
                    shortcut_for_action("ToggleOverviewSearch"),
                    cx,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_overview_search_open(agents, false, window, cx);
                })),
            );

        // The menu the button opens: the states and roles on offer, each
        // under its own heading.
        let menu = (has_filters && self.overview_menu_open).then(|| {
            let chip = |id: String, label: String, on: bool, chip: Chip, cx: &mut Context<Self>| {
                island_chip(SharedString::from(id), label, on, cx).on_click(cx.listener(
                    move |this, _, _window, cx| {
                        let filter = &mut this.agents_search.filter;
                        match chip {
                            Chip::State(s) => filter.toggle_state(s),
                            Chip::Role(r) => filter.toggle_role(r),
                        }
                        cx.notify();
                    },
                ))
            };
            let mut menu = island_menu("overview-island-menu", cx);
            if !narrowed.facets.states.is_empty() {
                let states: Vec<_> = narrowed
                    .facets
                    .states
                    .iter()
                    .map(|&s| {
                        chip(
                            format!("overview-state-{s:?}"),
                            state_label(s).to_string(),
                            search.filter.state_selected(s),
                            Chip::State(s),
                            cx,
                        )
                    })
                    .collect();
                menu = menu.child(island_menu_group("State", states, cx));
            }
            if !narrowed.facets.roles.is_empty() {
                let roles: Vec<_> = narrowed
                    .facets
                    .roles
                    .iter()
                    .map(|&r| {
                        chip(
                            format!("overview-role-{r:?}"),
                            r.badge().to_string(),
                            search.filter.role_selected(r),
                            Chip::Role(r),
                            cx,
                        )
                    })
                    .collect();
                menu = menu.child(island_menu_group("Role", roles, cx));
            }
            menu
        });
        let open = menu.is_some();
        island_stack(menu, bar)
            // A click anywhere outside the menu and the bar shuts the menu.
            .when(open, |d| {
                d.on_mouse_down_out(cx.listener(|this, _, _window, cx| {
                    this.overview_menu_open = false;
                    cx.notify();
                }))
            })
            .into_any_element()
    }

    /// The closed island: a pill naming the shortcut that opens it, and — so
    /// a narrowed grid never passes for the whole one — how much it shows.
    fn render_island_pill(
        &self,
        agents: bool,
        narrowed: &NarrowedOverview,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self.overview_search(agents).filter.is_active();
        island_pill(
            "overview-island-pill",
            pill_label(active, narrowed.shown.len(), narrowed.total),
            active,
            shortcut_for_action("ToggleOverviewSearch"),
            cx,
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            this.set_overview_search_open(agents, true, window, cx);
        }))
        .into_any_element()
    }

    /// What the grid shows when the bar, not the workspace, emptied it.
    pub(super) fn render_overview_no_match(
        &self,
        agents: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        v_flex()
            .id("projects-grid-empty")
            .flex_1()
            .h_full()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(ui_text_xl(cx))
                    .text_color(rgb(t.text_muted))
                    .child(if agents {
                        "No agents match these filters"
                    } else {
                        "No projects match this search"
                    }),
            )
            .child(
                div()
                    .id("overview-no-match-clear")
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(t.border_active))
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .child("Clear")
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.clear_overview_filter(agents, cx);
                    })),
            )
            .into_any_element()
    }
}

/// Which half of the filter a chip toggles.
#[derive(Clone, Copy)]
enum Chip {
    State(AgentActivity),
    Role(AgentRole),
}
