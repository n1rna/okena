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

use crate::keybindings::{Cancel, shortcut_for_action};
use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text_md, ui_text_ms, ui_text_xl};
use crate::views::components::{SimpleInput, SimpleInputState};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
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

    /// Open or close the island. Opening puts the cursor in its box; closing
    /// hands focus back if the box had it. What it narrows stays narrowed.
    pub(super) fn toggle_overview_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agents) = self.shown_overview(cx) else {
            return;
        };
        self.set_overview_search_open(agents, !self.overview_search_open, window, cx);
    }

    fn set_overview_search_open(
        &mut self,
        agents: bool,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overview_search_open = open;
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

    /// What floats at the bottom of the grid: the island when open — the search box,
    /// then "N of M" and Clear while anything narrows it, and on Agents the
    /// state and role chips on offer — or the pill it closes to.
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
        // A full-width row with nothing to hit, so only the island itself
        // catches the mouse and the grid under the rest stays usable.
        h_flex()
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(16.0))
            .justify_center()
            .px(px(12.0))
            .child(content)
            .into_any_element()
    }

    fn render_island(
        &self,
        agents: bool,
        narrowed: &NarrowedOverview,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let search = self.overview_search(agents);
        let active = search.filter.is_active();

        let mut row = h_flex()
            .id("overview-island")
            .occlude()
            .max_w_full()
            .min_w_0()
            .items_center()
            .gap(px(6.0))
            .pl(px(10.0))
            .pr(px(4.0))
            .py(px(4.0))
            .rounded(px(10.0))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_secondary))
            .shadow_lg()
            .child(
                svg()
                    .path("icons/search.svg")
                    .flex_shrink_0()
                    .size(px(13.0))
                    .text_color(rgb(t.text_muted)),
            )
            .child(
                div()
                    .id("overview-search")
                    .w(px(240.0))
                    .flex_shrink(1.0)
                    .min_w(px(120.0))
                    .overflow_hidden()
                    .rounded(px(4.0))
                    .bg(rgb(t.bg_primary))
                    .child(SimpleInput::new(&search.input).text_size(ui_text_ms(cx)))
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

        if agents {
            let states = narrowed.facets.states.iter().map(|&s| {
                (
                    format!("overview-state-{s:?}"),
                    state_label(s).to_string(),
                    search.filter.state_selected(s),
                    Chip::State(s),
                )
            });
            let roles = narrowed.facets.roles.iter().map(|&r| {
                (
                    format!("overview-role-{r:?}"),
                    r.badge().to_string(),
                    search.filter.role_selected(r),
                    Chip::Role(r),
                )
            });
            let mut chips = h_flex().min_w_0().gap(px(4.0)).overflow_hidden();
            let mut first_role = true;
            for (id, label, on, chip) in states.chain(roles) {
                // A gap between the two groups, so they read as two.
                if matches!(chip, Chip::Role(_)) && std::mem::take(&mut first_role) {
                    chips = chips.child(div().w(px(6.0)).flex_shrink_0());
                }
                chips = chips.child(render_chip(id, label, on, chip, cx));
            }
            row = row.child(chips);
        }

        let shortcut = shortcut_for_action("ToggleOverviewSearch");
        let close_tip: SharedString = match &shortcut {
            Some(keys) => format!("Close search ({keys})").into(),
            None => "Close search".into(),
        };
        row.children(active.then(|| {
            div()
                .flex_shrink_0()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(format!("{} of {}", narrowed.shown.len(), narrowed.total))
                .into_any_element()
        }))
        .children(active.then(|| self.render_clear(agents, "overview-clear", cx)))
        .child(
            div()
                .id("overview-island-close")
                .flex_shrink_0()
                .cursor_pointer()
                .size(px(22.0))
                .rounded(px(6.0))
                .flex()
                .items_center()
                .justify_center()
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .child(
                    svg()
                        .path("icons/close.svg")
                        .size(px(11.0))
                        .text_color(rgb(t.text_secondary)),
                )
                .tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(close_tip.clone()).build(window, cx)
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_overview_search_open(agents, false, window, cx);
                })),
        )
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
        let t = theme(cx);
        let active = self.overview_search(agents).filter.is_active();
        let shortcut = shortcut_for_action("ToggleOverviewSearch");
        h_flex()
            .id("overview-island-pill")
            .occlude()
            .cursor_pointer()
            .items_center()
            .gap(px(6.0))
            .px(px(10.0))
            .py(px(3.0))
            .rounded_full()
            .border_1()
            .border_color(rgb(if active { t.border_active } else { t.border }))
            .bg(rgb(t.bg_secondary))
            .shadow_md()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .text_size(ui_text_ms(cx))
            .child(
                svg()
                    .path("icons/search.svg")
                    .size(px(11.0))
                    .text_color(rgb(t.text_muted)),
            )
            .child(
                div()
                    .text_color(rgb(t.text_secondary))
                    .child(if active {
                        format!("{} of {}", narrowed.shown.len(), narrowed.total)
                    } else {
                        "Search".to_string()
                    }),
            )
            .children(shortcut.map(|keys| {
                div()
                    .px(px(4.0))
                    .rounded(px(3.0))
                    .bg(with_alpha(t.border, 0.5))
                    .text_color(rgb(t.text_muted))
                    .child(keys)
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.set_overview_search_open(agents, true, window, cx);
            }))
            .into_any_element()
    }

    fn render_clear(&self, agents: bool, id: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .id(id)
            .cursor_pointer()
            .flex_shrink_0()
            .px(px(6.0))
            .py(px(1.0))
            .rounded(px(3.0))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_secondary))
            .hover(|s| s.bg(rgb(t.bg_hover)).text_color(rgb(t.text_primary)))
            .child("Clear")
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.clear_overview_filter(agents, cx);
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

fn render_chip(
    id: String,
    label: String,
    on: bool,
    chip: Chip,
    cx: &mut Context<WindowView>,
) -> AnyElement {
    let t = theme(cx);
    div()
        .id(SharedString::from(id))
        .cursor_pointer()
        .flex_shrink_0()
        .px(px(7.0))
        .py(px(1.0))
        .rounded(px(3.0))
        .border_1()
        .text_size(ui_text_ms(cx))
        .map(|el| {
            if on {
                el.bg(with_alpha(t.button_primary_bg, 0.22))
                    .border_color(rgb(t.border_active))
                    .text_color(rgb(t.text_primary))
            } else {
                el.border_color(rgb(t.border))
                    .text_color(rgb(t.text_secondary))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
            }
        })
        .child(label)
        .on_click(cx.listener(move |this, _, _window, cx| {
            let filter = &mut this.agents_search.filter;
            match chip {
                Chip::State(s) => filter.toggle_state(s),
                Chip::Role(r) => filter.toggle_role(r),
            }
            cx.notify();
        }))
        .into_any_element()
}
