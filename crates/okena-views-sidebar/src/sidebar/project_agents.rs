//! Agent sessions in the Projects list, under the worktrees they work in.
//!
//! Which session sits where is `okena_state::agent_links`, the same relation
//! the session panel lists its worktrees by; this only draws it. A session
//! sits under every worktree it works in, and one with no worktree under
//! every repo it was given. Rows reuse the Agents list's own card and closed
//! row, so an agent reads the same in both lists.

use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use okena_ui::theme::theme;
use okena_ui::tokens::ui_text_ms;
use okena_workspace::state::agent_links::{SessionPlacement, place_sessions};
use std::collections::HashMap;

use super::agents_section::{ClosedRow, SessionRow, role_color};
use super::{Sidebar, SidebarCursorItem};

/// One agent row's contents, built once per render and drawn under each row
/// the session is placed under.
pub(super) enum ProjectAgentRow {
    Live(SessionRow),
    Closed(ClosedRow),
}

/// Where the agents go and what each one shows.
#[derive(Default)]
pub(super) struct ProjectAgents {
    pub(super) placement: SessionPlacement,
    rows: HashMap<String, ProjectAgentRow>,
}

impl Sidebar {
    /// Where agent sessions sit in this window's Projects list, or an empty
    /// placement when its Show agents option is off.
    pub(super) fn agent_placement(&self, cx: &App) -> SessionPlacement {
        let workspace = self.workspace.read(cx);
        let Some(window) = workspace.data().window(self.window_id) else {
            return SessionPlacement::default();
        };
        if !window.projects_show_agents {
            return SessionPlacement::default();
        }
        // Only the space showing: a session nests under the project whose
        // task it shares, and a project in another space is not on screen to
        // nest under.
        let in_space: Vec<_> = workspace.projects_in_active_space().cloned().collect();
        place_sessions(&in_space, window.agent_sort_mode)
    }

    /// The placement plus a row for every session it places.
    pub(super) fn collect_project_agents(&self, cx: &App) -> ProjectAgents {
        let placement = self.agent_placement(cx);
        if placement.is_empty() {
            return ProjectAgents::default();
        }
        let workspace = self.workspace.read(cx);
        let focused_id = self.focus_manager.read(cx).focused_project_id().cloned();
        let now = okena_ui::ago::now_millis();
        let mut rows = HashMap::new();
        for p in workspace.projects_in_active_space() {
            if !placement.contains(&p.id) {
                continue;
            }
            let Some(role) = p.agent_role() else { continue };
            let row = if p.is_closed() {
                ProjectAgentRow::Closed(self.closed_session_row(
                    p,
                    role,
                    workspace,
                    focused_id.as_deref(),
                    now,
                ))
            } else {
                ProjectAgentRow::Live(self.live_session_row(
                    p,
                    role,
                    workspace,
                    focused_id.as_deref(),
                ))
            };
            rows.insert(p.id.clone(), row);
        }
        ProjectAgents { placement, rows }
    }

    /// Cursor items for the agents `ids`, one per row `render_project_agents`
    /// draws for them.
    pub(super) fn push_agent_cursor_items(
        ids: &[String],
        cursor_items: &mut Vec<SidebarCursorItem>,
    ) {
        cursor_items.extend(ids.iter().map(|id| SidebarCursorItem::Agent {
            project_id: id.clone(),
        }));
    }

    /// Draw the agents `ids` under the row `owner`, one flat element each,
    /// advancing `flat_idx` as `push_agent_cursor_items` does.
    // GPUI render helper: params are render inputs (indent, cursor state).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_project_agents(
        &self,
        agents: &ProjectAgents,
        ids: &[String],
        owner: &str,
        indent: f32,
        cursor_index: Option<usize>,
        flat_idx: &mut usize,
        flat_elements: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        for id in ids {
            let is_cursor = cursor_index == Some(*flat_idx);
            *flat_idx += 1;
            let row = match agents.rows.get(id) {
                Some(row) => self.render_project_agent_row(row, owner, indent, is_cursor, cx),
                // Placed but gone between the two reads: keep the row count
                // the cursor expects.
                None => div().into_any_element(),
            };
            flat_elements.push(row);
        }
    }

    /// One agent as a single line among the project rows: state dot, kind
    /// badge, name, and its state — or when it was closed — at the end.
    ///
    /// A line rather than the Agents list's card: here the agent is a detail
    /// of the worktree above it, and a card's border and report would make
    /// it heavier than the repo it belongs to. The agent's last report is left
    /// to the Agents list.
    fn render_project_agent_row(
        &self,
        row: &ProjectAgentRow,
        owner: &str,
        indent: f32,
        is_cursor: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let (info, role, focused) = match row {
            ProjectAgentRow::Live(r) => (&r.info, r.role, r.focused),
            ProjectAgentRow::Closed(r) => (&r.info, r.role, r.focused),
        };
        let closed = matches!(row, ProjectAgentRow::Closed(_));
        let kind_color = role_color(role, &t);
        // The state, coloured as the card colours it; a closed agent says
        // when instead, in the muted colour of history.
        let (dot, trailing, trailing_color, attention) = match row {
            ProjectAgentRow::Live(r) => {
                let c = Sidebar::card_color(r.card, &t);
                (
                    Some(c),
                    r.card.label().to_string(),
                    c,
                    r.card.wants_attention(),
                )
            }
            ProjectAgentRow::Closed(r) => (None, r.when.clone(), t.text_muted, false),
        };
        let id = info.id.clone();
        // Unique per owner: one session can sit under several rows.
        let element_id = format!("project-agent-{owner}-{id}");

        h_flex()
            .id(SharedString::from(element_id.clone()))
            .h(px(24.0))
            .pl(px(indent))
            .pr(px(8.0))
            .gap(px(4.0))
            .items_center()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .when(focused, |d| d.bg(rgb(t.bg_hover)))
            .when(is_cursor, |d| {
                d.border_l_2().border_color(rgb(t.border_active))
            })
            // Dimmed: history, not work in progress.
            .when(closed, |d| d.opacity(0.6))
            // The dot sits in the 14px indicator box the project rows use, so
            // it lines up with the worktree's own dot above.
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(14.0))
                    .h(px(16.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(match dot {
                        Some(color) => div().size(px(6.0)).rounded_full().bg(rgb(color)),
                        None => div()
                            .size(px(6.0))
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(t.text_muted)),
                    }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .px(px(4.0))
                    .rounded(px(3.0))
                    .bg(okena_ui::theme::with_alpha(kind_color, 0.15))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(kind_color))
                    .child(role.badge()),
            )
            .child(
                crate::item_widgets::sidebar_name_label(
                    SharedString::from(format!("{element_id}-name")),
                    info.name.clone(),
                    &t,
                    cx,
                )
                .when(closed, |d| d.text_color(rgb(t.text_secondary))),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .px(px(4.0))
                    .rounded(px(3.0))
                    .when(attention, |d| {
                        d.bg(okena_ui::theme::with_alpha(trailing_color, 0.2))
                    })
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(trailing_color))
                    .child(trailing),
            )
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.focus_project_from_sidebar(id.clone(), true, cx);
            }))
            .into_any_element()
    }
}
