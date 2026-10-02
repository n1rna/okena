//! AGENTS section — agent sessions, separated from the repos.
//!
//! An agent session is a project rooted at the configured projects directory
//! rather than a repo, created when a task spans several of them. Listing it
//! among the repos makes it look like one; it isn't, so it gets its own
//! section with the task it belongs to.
//!
//! Both kinds — sessions working a task and sessions writing a spec — share one
//! list, told apart by a coloured kind badge on each row. Separate sections
//! made the list taller than it needed to be and forced a heading onto a group
//! that was often a single row; the badge carries the same information without
//! spending a line on it.

use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_ui::theme::theme;
use okena_ui::tokens::ui_text_ms;

use super::{Sidebar, SidebarProjectInfo};

use crate::agent_card::{AgentColor, CardState, agent_color, card_state, subtree_summary};
use crate::drag::{AgentDrag, AgentDragView};
use crate::item_widgets::sidebar_rename_input;
use okena_core::api::ActionRequest;
use okena_ui::rename_state::is_renaming;
use okena_workspace::state::AgentRole;
use okena_workspace::state::agent_order::{self, AgentDrop, OrderedAgent};
use std::collections::HashMap;

/// Colour for a role's badge.
///
/// Distinct hues rather than shades of one, so the kinds are told apart at a
/// glance in a mixed list. Implementing is the primary colour because it is
/// the one that means work is actually happening.
pub(super) fn role_color(role: AgentRole, t: &okena_ui::theme::ThemeColors) -> u32 {
    match role {
        AgentRole::Implement => t.button_primary_bg,
        AgentRole::Task => t.term_cyan,
        AgentRole::Spec => t.success,
        AgentRole::Knowledge => t.term_magenta,
        AgentRole::Scan => t.term_blue,
        AgentRole::Custom => t.warning,
    }
}

/// The entries of `project_order` that belong to the daemon owning `id`, in
/// order: the list a `MoveProject` for `id` indexes into.
///
/// The sidebar's `project_order` is a mirror, with every daemon's entries in
/// it under a `remote:<connection>:` prefix. The action goes to one daemon and
/// is applied to that daemon's own order, so an index counted across all of
/// them would land in the wrong place.
fn order_of_owner(project_order: &[String], id: &str) -> Vec<String> {
    let owner = |id: &str| {
        id.strip_prefix("remote:")
            .and_then(|rest| rest.split_once(':'))
            .map(|(connection, _)| connection.to_string())
    };
    let wanted = owner(id);
    project_order
        .iter()
        .filter(|entry| owner(entry) == wanted)
        .cloned()
        .collect()
}

/// A session row: the project, its kind, and what it is working on.
pub(super) struct SessionRow {
    pub(super) info: SidebarProjectInfo,
    pub(super) role: AgentRole,
    /// Provider ids of the session's task and its task's parent, for placing
    /// it in the hierarchy.
    pub(super) task_id: Option<String>,
    pub(super) parent_task_id: Option<String>,
    /// How deep the ticket hierarchy puts it. Filled in after sorting.
    pub(super) depth: usize,
    /// Whether it is in the pinned tier: a pinned top-level agent, or a
    /// sub-agent of one. Filled in after sorting, like `depth`.
    pub(super) pinned: bool,
    /// Whether this is the session currently open in the main area.
    pub(super) focused: bool,
    /// Task key for a task session, change name for a spec session.
    pub(super) subtitle: Option<String>,
    /// How it is doing: the terminal's state and the agent's report combined.
    pub(super) card: CardState,
    /// What the agent last said it was doing.
    pub(super) status: Option<String>,
    /// The tokens it has used, pre-formatted; `None` when none are known.
    pub(super) tokens: Option<String>,
}

/// A closed session's row: who it was and when it was closed.
pub(super) struct ClosedRow {
    pub(super) info: SidebarProjectInfo,
    pub(super) role: AgentRole,
    pub(super) subtitle: Option<String>,
    /// "closed 3h ago".
    pub(super) when: String,
    /// Whether this is the session currently open in the main area.
    pub(super) focused: bool,
}

/// The closed agent sessions, most recently closed first.
fn closed_sessions(
    projects: &[okena_workspace::state::ProjectData],
) -> Vec<&okena_workspace::state::ProjectData> {
    let mut closed: Vec<_> = projects
        .iter()
        .filter(|p| p.is_closed() && p.agent_role().is_some())
        .collect();
    closed.sort_by(|a, b| {
        b.closed_at
            .cmp(&a.closed_at)
            .then_with(|| a.name.cmp(&b.name))
    });
    closed
}

impl Sidebar {
    /// How a session row names itself: its project info with a tidied name,
    /// and what it is working on when that is not already the name.
    ///
    /// One place for both lists, so a closed session reads exactly as it did
    /// while it was live.
    fn session_identity(
        &self,
        p: &okena_workspace::state::ProjectData,
        role: AgentRole,
        workspace: &okena_workspace::state::Workspace,
    ) -> (SidebarProjectInfo, Option<String>) {
        // The subtitle used to repeat the name — a row read "add-login (spec)"
        // over "add-login" — which cost a line to say nothing.
        let subtitle = match role {
            // A session refining a spec document has no change; its goal
            // names the document instead.
            AgentRole::Spec => p.spec_change.clone().or_else(|| p.custom_session.clone()),
            AgentRole::Knowledge => p.custom_session.clone(),
            AgentRole::Task | AgentRole::Implement => {
                p.task_ref.as_ref().map(|t| t.display_key.clone())
            }
            AgentRole::Custom => p.custom_session.clone(),
            // The repository it maps, or the repositories it links.
            AgentRole::Scan => p.project_scan.clone(),
        };
        let mut info = SidebarProjectInfo::from_project(p, workspace, self.window_id);
        // The badge says "spec"; the name saying "(spec)" as well says it
        // twice. Stripped at display rather than left to a migration: the
        // name is the user's to rename, and rewriting it under them would be
        // worse than showing it tidily.
        if let Some(suffix) = role.legacy_name_suffix()
            && let Some(trimmed) = info.name.strip_suffix(suffix)
        {
            info.name = trimmed.to_string();
        }
        let subtitle = subtitle.filter(|s| s != &info.name);
        (info, subtitle)
    }

    /// Colour for a card state: the attention states stand out, the rest
    /// recede so the ones that need you are the ones you see.
    pub(super) fn card_color(state: CardState, t: &okena_ui::theme::ThemeColors) -> u32 {
        match state {
            CardState::NeedsInput | CardState::ReadyForReview | CardState::Unknown => t.warning,
            CardState::Blocked => t.error,
            CardState::Working => t.success,
            CardState::Waiting | CardState::Done | CardState::Stopped => t.text_muted,
        }
    }

    /// A live session as its row in the Agents list shows it: identity,
    /// state and last report, read from the workspace and the terminals.
    ///
    /// Shared with the Projects list, so an agent under a worktree carries
    /// the same badge and state as it does here.
    pub(super) fn live_session_row(
        &self,
        p: &okena_workspace::state::ProjectData,
        role: AgentRole,
        workspace: &okena_workspace::state::Workspace,
        focused_id: Option<&str>,
    ) -> SessionRow {
        let (info, subtitle) = self.session_identity(p, role, workspace);
        // Whether anything runs, live from the registry like the project
        // rows; what the agent is doing, from the daemon.
        let (running, activity) = {
            let registry = self.terminals.lock();
            let live: Vec<&String> = info
                .terminal_ids
                .iter()
                .filter(|tid| registry.contains_key(*tid))
                .collect();
            (
                !live.is_empty(),
                live.iter()
                    .find_map(|tid| workspace.agent_activity(&p.id, tid)),
            )
        };
        let status = p
            .agent
            .as_ref()
            .and_then(|a| a.status.clone())
            .filter(|s| !s.trim().is_empty());
        SessionRow {
            info,
            role,
            task_id: p.task_ref.as_ref().map(|t| t.id.external_id.clone()),
            parent_task_id: p.task_ref.as_ref().and_then(|t| t.parent_id.clone()),
            depth: 0,
            pinned: false,
            focused: focused_id == Some(p.id.as_str()),
            subtitle,
            card: card_state(running, activity),
            status,
            tokens: p.agent_usage.as_ref().and_then(|usage| usage.tokens_short()),
        }
    }

    /// A closed session as the history shows it: its identity as it was
    /// while live, and when it was closed.
    pub(super) fn closed_session_row(
        &self,
        p: &okena_workspace::state::ProjectData,
        role: AgentRole,
        workspace: &okena_workspace::state::Workspace,
        focused_id: Option<&str>,
        now: u64,
    ) -> ClosedRow {
        let (info, subtitle) = self.session_identity(p, role, workspace);
        let when = p
            .closed_at
            .map(|at| format!("closed {}", okena_ui::ago::format_ago(at, now)))
            .unwrap_or_default();
        ClosedRow {
            info,
            role,
            subtitle,
            when,
            focused: focused_id == Some(p.id.as_str()),
        }
    }

    /// The colour a session's card is drawn in, standing on its own.
    ///
    /// Keyed by the ticket family the tasks view colours by, so an agent
    /// matches the task it works on: the parent ticket when there is one,
    /// then its own ticket, and only a session without a ticket by its id.
    fn session_color_key(row: &SessionRow) -> &str {
        row.parent_task_id
            .as_deref()
            .or(row.task_id.as_deref())
            .unwrap_or(&row.info.id)
    }

    /// One agent as a card: kind, name and state, then what it last said.
    ///
    /// The agent's own status line is the body because it is the most useful
    /// thing to read about an agent at a glance — more than the task key,
    /// which the name usually already is. An agent that wants you gets an
    /// accent edge and a filled pill, so a list of ten reads as "these two".
    fn render_agent_card(
        &self,
        row: &SessionRow,
        nested: bool,
        color: AgentColor,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let id = row.info.id.clone();
        let renaming = is_renaming(&self.project_rename, &id);
        let name = row.info.name.clone();
        let role = row.role;
        let kind_color = role_color(role, &t);
        let state_color = Self::card_color(row.card, &t);
        let attention = row.card.wants_attention();
        let focused = row.focused;
        let body = row.status.clone().or_else(|| row.subtitle.clone());
        // What its terminals hold, split and tab shells included; nothing
        // while no process runs in it.
        let memory = okena_workspace::process_memory::project_memory(&row.info.id, cx)
            .map(okena_workspace::process_memory::format_memory);
        // Flat and neutral, so a list of ten is calm. The agent's own colour
        // is only the thin bar on its left edge, enough to tell neighbours
        // apart without every card shouting. The selected card stands out by
        // a primary border over a faint primary wash, and stays exactly that
        // under the pointer: hovering what is already selected should not
        // look like a second, different state.
        let AgentColor { hue, lightness } = color;
        let accent = hsla(hue, 0.55, lightness, 0.9);
        let border = if focused {
            okena_ui::theme::with_alpha(t.button_primary_bg, 0.7)
        } else if attention {
            okena_ui::theme::with_alpha(state_color, 0.45)
        } else {
            okena_ui::theme::with_alpha(t.border, if nested { 0.5 } else { 0.7 })
        };
        let fill = if focused {
            okena_ui::theme::with_alpha(t.button_primary_bg, 0.06)
        } else {
            okena_ui::theme::with_alpha(t.bg_secondary, 1.0)
        };
        // Unselected cards answer the pointer with the list's usual hover
        // fill and a firmer edge; the selected one does not change.
        let hover_fill = okena_ui::theme::with_alpha(t.bg_hover, 1.0);
        let hover_border = okena_ui::theme::with_alpha(t.border_active, 0.8);

        v_flex()
            .id(SharedString::from(format!("agent-session-{id}")))
            .relative()
            .cursor_pointer()
            .w_full()
            .min_w_0()
            .gap(px(3.0))
            .pl(px(11.0))
            .pr(px(8.0))
            .py(px(6.0))
            .rounded(px(7.0))
            .border_1()
            .border_color(border)
            .bg(fill)
            .when(!focused, |d| {
                d.hover(move |s| s.bg(hover_fill).border_color(hover_border))
            })
            .child(
                div()
                    .absolute()
                    .left(px(3.0))
                    .top(px(7.0))
                    .bottom(px(7.0))
                    .w(px(3.0))
                    .rounded(px(2.0))
                    .bg(accent),
            )
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex_shrink_0()
                            .size(px(7.0))
                            .rounded_full()
                            .bg(rgb(state_color)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(px(5.0))
                            .rounded(px(3.0))
                            .bg(okena_ui::theme::with_alpha(kind_color, 0.15))
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(kind_color))
                            .child(role.badge()),
                    )
                    .child(if renaming {
                        // Renamed in place, like a project row: a session's
                        // name is the user's, and the list is where it is read.
                        sidebar_rename_input(
                            ElementId::Name(format!("agent-rename-{id}").into()),
                            &self.project_rename,
                            &t,
                            cx,
                        )
                        .map(IntoElement::into_any_element)
                        .unwrap_or_else(|| div().flex_1().into_any_element())
                    } else {
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(okena_ui::tokens::ui_text(13.0, cx))
                            .text_color(rgb(t.text_primary))
                            .child(row.info.name.clone())
                            .into_any_element()
                    })
                    // The pin is the group's, so only its top-level card
                    // says so.
                    .when(row.pinned && !nested, |d| {
                        d.child(
                            svg()
                                .path("icons/bookmark.svg")
                                .flex_shrink_0()
                                .size(px(11.0))
                                .text_color(rgb(t.text_muted)),
                        )
                    })
                    .children(memory.map(|memory| {
                        div()
                            .flex_shrink_0()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(memory)
                    }))
                    // Beside the memory, in the same quiet type: tokens only,
                    // the cost is the Info tab's.
                    .children(row.tokens.clone().map(|tokens| {
                        div()
                            .flex_shrink_0()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(tokens)
                    }))
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(px(5.0))
                            .rounded(px(3.0))
                            .when(attention, |d| {
                                d.bg(okena_ui::theme::with_alpha(state_color, 0.2))
                            })
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(state_color))
                            .child(row.card.label()),
                    ),
            )
            .children(body.map(|text| {
                // Two lines: enough to say what it is doing, short enough that
                // a long report does not turn the list into a transcript.
                div()
                    .w_full()
                    .min_w_0()
                    .line_clamp(2)
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(text)
                    .into_any_element()
            }))
            .debug_selector(|| format!("agent-card-{}", row.info.id))
            // Picked up to be placed among the pinned agents, or back out of
            // them. A sub-agent is not: it goes where its parent goes.
            .when(!nested, |d| {
                d.on_drag(
                    AgentDrag {
                        project_id: row.info.id.clone(),
                        name: row.info.name.clone(),
                        pinned: row.pinned,
                    },
                    |drag, _position, _window, cx| {
                        cx.new(|_| AgentDragView {
                            name: drag.name.clone(),
                        })
                    },
                )
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener({
                    let id = id.clone();
                    move |this, event: &MouseDownEvent, _window, cx| {
                        this.agent_menu = Some((id.clone(), event.position));
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                // Second click on the card the user is already on: rename it,
                // the same gesture a project row takes.
                if this.check_project_double_click(&id) {
                    this.start_project_rename(id.clone(), name.clone(), window, cx);
                } else {
                    this.focus_project_from_sidebar(id.clone(), true, cx);
                }
            }))
            .into_any_element()
    }

    /// An agent and, when it has any, its sub-agents inside a container.
    ///
    /// A container rather than an indent: an epic's agent and the agents on
    /// its stories are one piece of work, and a boundary around them says so
    /// where a few pixels of whitespace only hinted at it. The header counts
    /// the sub-agents that want you, so a collapsed glance still tells you
    /// whether to look inside — and it is what folds them away: clicking it
    /// hides the sub-agents under the parent's card, which changes nothing
    /// about where the group sits or whether it is pinned.
    ///
    /// `rows` is in `agent_tree` order, so a node's descendants follow it with
    /// a greater depth; this consumes them and returns how many rows it used.
    fn render_agent_group(
        &self,
        rows: &[SessionRow],
        family: Option<AgentColor>,
        cx: &mut Context<Self>,
    ) -> (AnyElement, usize) {
        let root = &rows[0];
        // A sub-agent takes its family's colour, not one of its own.
        let color = agent_color(Self::session_color_key(root), family);
        let family = family.unwrap_or(color);
        let mut used = 1;
        let mut children: Vec<AnyElement> = Vec::new();
        let mut child_states: Vec<CardState> = Vec::new();
        while used < rows.len() && rows[used].depth > root.depth {
            if rows[used].depth == root.depth + 1 {
                child_states.push(rows[used].card);
            }
            let (child, consumed) = self.render_agent_group(&rows[used..], Some(family), cx);
            children.push(child);
            used += consumed;
        }
        let nested = root.depth > 0;
        let card = self.render_agent_card(root, nested, color, cx);
        if children.is_empty() {
            return (card, used);
        }

        let t = theme(cx);
        let attention = child_states.iter().any(|c| c.wants_attention());
        let folded = self.collapsed_agent_groups.contains(&root.info.id);
        let fold_id = root.info.id.clone();
        // A quiet frame: the cards inside carry the family's colour on their
        // accent bars, so the frame itself needs none.
        let group = v_flex()
            .w_full()
            .min_w_0()
            .gap(px(4.0))
            .p(px(4.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(if attention {
                okena_ui::theme::with_alpha(t.warning, 0.5)
            } else {
                okena_ui::theme::with_alpha(t.border, 0.5)
            })
            .bg(okena_ui::theme::with_alpha(t.bg_secondary, 0.35))
            .child(card)
            .child(
                h_flex()
                    .id(SharedString::from(format!("agent-group-fold-{fold_id}")))
                    .debug_selector(|| format!("agent-fold-{fold_id}"))
                    .px(px(4.0))
                    .gap(px(3.0))
                    .items_center()
                    .cursor_pointer()
                    .rounded(px(3.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .child(
                        svg()
                            .path(if folded {
                                "icons/chevron-right.svg"
                            } else {
                                "icons/chevron-down.svg"
                            })
                            .flex_shrink_0()
                            .size(px(10.0))
                            .text_color(rgb(t.text_muted)),
                    )
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(if attention { t.warning } else { t.text_muted }))
                            .child(subtree_summary(&child_states)),
                    )
                    .on_click(cx.listener({
                        let fold_id = fold_id.clone();
                        move |this, _, _window, cx| {
                            this.toggle_agent_group(&fold_id);
                            cx.stop_propagation();
                            cx.notify();
                        }
                    })),
            )
            .when(!folded, |d| {
                d.child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .gap(px(4.0))
                        .pl(px(6.0))
                        .children(children),
                )
            });
        (group.into_any_element(), used)
    }

    /// The live agent sessions of the space showing, in the order both lists
    /// draw them: pinned groups first, then the header's sort.
    pub(super) fn live_agent_order(
        &self,
        workspace: &okena_workspace::state::Workspace,
    ) -> Vec<OrderedAgent> {
        let live: Vec<&okena_workspace::state::ProjectData> = workspace
            .projects_in_active_space()
            // Closed sessions are the history's, behind the header's button.
            .filter(|p| p.agent_role().is_some() && !p.is_closed())
            .collect();
        let sort_mode = workspace
            .data()
            .window(self.window_id)
            .map(|w| w.agent_sort_mode)
            .unwrap_or_default();
        agent_order::order_live(&live, &workspace.data().project_order, sort_mode)
    }

    /// Fold or unfold the sub-agents under the agent `id`.
    pub(super) fn toggle_agent_group(&mut self, id: &str) {
        if !self.collapsed_agent_groups.remove(id) {
            self.collapsed_agent_groups.insert(id.to_string());
        }
    }

    /// Place the agent `dragged` at `drop`: pin it there, move it within the
    /// pinned agents, or unpin it. Used by a drop and by the menu's pin item,
    /// which is a drop at the end of the pinned tier.
    ///
    /// Daemon-owned: the move and the pin are dispatched and mirror back, the
    /// move first so the agent is already in its place when it becomes pinned.
    pub(super) fn drop_agent(&mut self, dragged: &str, drop: AgentDrop, cx: &mut Context<Self>) {
        let plan = {
            let workspace = self.workspace.read(cx);
            let roots: Vec<(String, bool)> = self
                .live_agent_order(workspace)
                .into_iter()
                .filter(|agent| agent.depth == 0)
                .map(|agent| (agent.id, agent.pinned))
                .collect();
            let order = order_of_owner(&workspace.data().project_order, dragged);
            agent_order::plan_drop(dragged, &drop, &roots, &order)
        };
        let Some(plan) = plan else { return };
        if let Some(new_index) = plan.move_to {
            self.dispatch_action_for_project(
                dragged,
                ActionRequest::MoveProject {
                    project_id: dragged.to_string(),
                    new_index,
                },
                cx,
            );
        }
        if plan.toggle_pinned {
            self.dispatch_action_for_project(
                dragged,
                ActionRequest::ToggleProjectPinned {
                    project_id: dragged.to_string(),
                },
                cx,
            );
        }
    }

    /// A strip of the Agents list an agent can be dropped on to pin it: above
    /// the first pinned agent, or below the last.
    fn render_pin_zone(
        &self,
        id: &'static str,
        drop: AgentDrop,
        invite: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let active = t.border_active;
        div()
            .id(id)
            .debug_selector(|| id.to_string())
            .w_full()
            .h(px(6.0))
            // With nothing pinned yet there is no tier to aim at, so while an
            // agent is being carried the strip opens up and says what it is.
            .when(invite, |d| {
                d.h(px(26.0))
                    .mb(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.0))
                    .border_1()
                    .border_dashed()
                    .border_color(okena_ui::theme::with_alpha(t.border, 0.9))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child("Drop here to pin")
            })
            .drag_over::<AgentDrag>(move |style, _, _, _| {
                style
                    .border_color(rgb(active))
                    .bg(okena_ui::theme::with_alpha(active, 0.25))
            })
            .on_drop(cx.listener(move |this, drag: &AgentDrag, _window, cx| {
                this.drop_agent(&drag.project_id, drop.clone(), cx);
            }))
            .into_any_element()
    }

    /// The agent sessions in this workspace: the pinned ones as arranged,
    /// then the rest ordered by the header's sort.
    pub(super) fn render_agents_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.read(cx);

        let focused_id = self.focus_manager.read(cx).focused_project_id().cloned();

        let by_id: HashMap<&str, &okena_workspace::state::ProjectData> = workspace
            .projects_in_active_space()
            .map(|p| (p.id.as_str(), p))
            .collect();
        let sessions: Vec<SessionRow> = self
            .live_agent_order(workspace)
            .into_iter()
            .filter_map(|placed| {
                let p = by_id.get(placed.id.as_str())?;
                let mut row =
                    self.live_session_row(p, p.agent_role()?, workspace, focused_id.as_deref());
                row.depth = placed.depth;
                row.pinned = placed.pinned;
                Some(row)
            })
            .collect();

        // The tab header already says AGENTS, so an empty list says why it is
        // empty rather than showing nothing at all.
        if sessions.is_empty() {
            return v_flex()
                .child(
                    div()
                        .px(px(12.0))
                        .py(px(10.0))
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child(
                            "No agent sessions. Start work on a task, draft a change \
                             in the Library, or use + above.",
                        ),
                )
                .into_any_element();
        }

        // Each top-level agent with its sub-agents is one group, and the unit
        // that is pinned and arranged.
        let mut pinned: Vec<AnyElement> = Vec::new();
        let mut rest: Vec<AnyElement> = Vec::new();
        let mut first_pinned: Option<String> = None;
        let active = t.border_active;
        let mut i = 0;
        while i < sessions.len() {
            let root = &sessions[i];
            let (group, used) = self.render_agent_group(&sessions[i..], None, cx);
            i += used.max(1);
            if !root.pinned {
                rest.push(group);
                continue;
            }
            // Dropped on a pinned agent, another lands just above it.
            let target = root.info.id.clone();
            first_pinned.get_or_insert_with(|| target.clone());
            pinned.push(
                div()
                    .id(SharedString::from(format!("agent-pinned-{target}")))
                    .w_full()
                    .drag_over::<AgentDrag>(move |style, _, _, _| {
                        style.border_t_2().border_color(rgb(active))
                    })
                    .on_drop(cx.listener(move |this, drag: &AgentDrag, _window, cx| {
                        this.drop_agent(
                            &drag.project_id,
                            AgentDrop::BeforePinned(target.clone()),
                            cx,
                        );
                    }))
                    .child(group)
                    .into_any_element(),
            );
        }

        let has_pinned = !pinned.is_empty();
        let head = self.render_pin_zone(
            "agent-pin-head",
            match first_pinned {
                Some(first) => AgentDrop::BeforePinned(first),
                None => AgentDrop::EndOfPinned,
            },
            !has_pinned && cx.has_active_drag(),
            cx,
        );
        v_flex()
            .w_full()
            .px(px(8.0))
            .pb(px(4.0))
            .child(head)
            .when(has_pinned, |d| {
                d.child(v_flex().w_full().gap(px(6.0)).children(pinned))
                    // Below the last pinned agent: still pinned, at the end.
                    .child(self.render_pin_zone(
                        "agent-pin-tail",
                        AgentDrop::EndOfPinned,
                        false,
                        cx,
                    ))
            })
            // Let go anywhere among the rest, a pinned agent is unpinned and
            // the sort places it; an unpinned one has nowhere new to go.
            .child(
                v_flex()
                    .id("agent-unpinned")
                    .debug_selector(|| "agent-unpinned".to_string())
                    .w_full()
                    .gap(px(6.0))
                    .rounded(px(8.0))
                    .can_drop(|drag, _, _| {
                        drag.downcast_ref::<AgentDrag>()
                            .is_some_and(|drag| drag.pinned)
                    })
                    .drag_over::<AgentDrag>(move |style, _, _, _| {
                        style.bg(okena_ui::theme::with_alpha(active, 0.12))
                    })
                    .on_drop(cx.listener(|this, drag: &AgentDrag, _window, cx| {
                        this.drop_agent(&drag.project_id, AgentDrop::Unpinned, cx);
                    }))
                    .children(rest),
            )
            .into_any_element()
    }

    /// The menu a right-click on an agent opens: pin it or unpin it, and
    /// rename it.
    pub(super) fn render_agent_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let Some((id, at)) = self.agent_menu.clone() else {
            return div().into_any_element();
        };
        let (name, placed) = {
            let workspace = self.workspace.read(cx);
            (
                workspace.project(&id).map(|p| p.name.clone()),
                self.live_agent_order(workspace)
                    .into_iter()
                    .find(|agent| agent.id == id),
            )
        };
        let Some(name) = name else {
            return div().into_any_element();
        };
        let mut panel = okena_ui::menu::context_menu_panel("agent-menu", &t)
            .min_w(px(180.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.agent_menu = None;
                cx.notify();
            }));
        // Only a live, top-level agent has a place of its own to pin: a
        // sub-agent goes where its parent goes, and a closed one is history.
        if let Some(placed) = placed.filter(|agent| agent.depth == 0) {
            let for_pin = id.clone();
            let (label, drop) = if placed.pinned {
                ("Unpin", AgentDrop::Unpinned)
            } else {
                ("Pin to top", AgentDrop::EndOfPinned)
            };
            panel = panel.child(
                okena_ui::menu::menu_item("agent-menu-pin", "icons/bookmark.svg", label, &t)
                    .debug_selector(|| "agent-menu-pin".to_string())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.agent_menu = None;
                            this.drop_agent(&for_pin, drop.clone(), cx);
                            cx.notify();
                        }),
                    ),
            );
        }
        panel = panel.child(
            okena_ui::menu::menu_item("agent-menu-rename", "icons/edit.svg", "Rename", &t)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        this.agent_menu = None;
                        this.start_project_rename(id.clone(), name.clone(), window, cx);
                    }),
                ),
        );
        // Deferred so it paints over the cards it was opened from, which come
        // later in the sidebar than this does.
        deferred(anchored().position(at).snap_to_window().child(panel))
            .with_priority(1)
            .into_any_element()
    }

    /// The closed agents: the history the header's button swaps in.
    ///
    /// Newest closed first, each row named as it was while live and saying
    /// when it was closed. A row opens the session like a live one does,
    /// where it can be resumed.
    pub(super) fn render_closed_agents_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.read(cx);
        let focused_id = self.focus_manager.read(cx).focused_project_id().cloned();
        let now = okena_ui::ago::now_millis();

        // A space's closed-agent history is its own, like its live agents'.
        let in_space: Vec<_> = workspace.projects_in_active_space().cloned().collect();
        let closed: Vec<ClosedRow> = closed_sessions(&in_space)
            .into_iter()
            .filter_map(|p| {
                let role = p.agent_role()?;
                Some(self.closed_session_row(p, role, workspace, focused_id.as_deref(), now))
            })
            .collect();
        let rows: Vec<AnyElement> = closed
            .iter()
            .map(|row| self.render_closed_row(row, cx))
            .collect();

        // What this list is, at its top. Not a button: the history toggle in
        // the header is the one way back to the live agents.
        let heading = h_flex()
            .mx(px(8.0))
            .px(px(6.0))
            .py(px(4.0))
            .gap(px(6.0))
            .items_center()
            .child(
                div()
                    .flex_1()
                    .text_size(ui_text_ms(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_secondary))
                    .child("Closed agents"),
            )
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(format!("{}", rows.len())),
            );

        let body = if rows.is_empty() {
            div()
                .px(px(12.0))
                .py(px(10.0))
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(
                    "No closed agents. Close one with Close on its session panel; \
                     it stays here, ready to resume, until you delete it.",
                )
                .into_any_element()
        } else {
            v_flex()
                .w_full()
                .gap(px(6.0))
                .px(px(8.0))
                .children(rows)
                .into_any_element()
        };
        v_flex()
            .w_full()
            .gap(px(4.0))
            .py(px(4.0))
            .child(heading)
            .child(body)
            .into_any_element()
    }

    /// One closed session: kind, name and when it was closed, then what it
    /// worked on.
    fn render_closed_row(&self, row: &ClosedRow, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let ClosedRow {
            info,
            role,
            subtitle,
            when: closed,
            focused,
        } = row;
        let (role, focused) = (*role, *focused);
        let id = info.id.clone();
        let renaming = is_renaming(&self.project_rename, &id);
        let name = info.name.clone();
        let kind_color = role_color(role, &t);
        v_flex()
            .id(SharedString::from(format!("closed-agent-{id}")))
            .cursor_pointer()
            .w_full()
            .min_w_0()
            .gap(px(3.0))
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(7.0))
            .border_1()
            .border_color(if focused {
                okena_ui::theme::with_alpha(t.button_primary_bg, 0.7)
            } else {
                okena_ui::theme::with_alpha(t.border, 0.7)
            })
            .bg(if focused {
                okena_ui::theme::with_alpha(t.button_primary_bg, 0.06)
            } else {
                okena_ui::theme::with_alpha(t.bg_secondary, 1.0)
            })
            .when(!focused, |d| {
                d.hover(|s| s.bg(okena_ui::theme::with_alpha(t.bg_hover, 1.0)))
            })
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(px(5.0))
                            .rounded(px(3.0))
                            .bg(okena_ui::theme::with_alpha(kind_color, 0.15))
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(kind_color))
                            .child(role.badge()),
                    )
                    .child(if renaming {
                        sidebar_rename_input(
                            ElementId::Name(format!("closed-agent-rename-{id}").into()),
                            &self.project_rename,
                            &t,
                            cx,
                        )
                        .map(IntoElement::into_any_element)
                        .unwrap_or_else(|| div().flex_1().into_any_element())
                    } else {
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(okena_ui::tokens::ui_text(13.0, cx))
                            .text_color(rgb(t.text_secondary))
                            .child(info.name.clone())
                            .into_any_element()
                    })
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(closed.clone()),
                    ),
            )
            .children(subtitle.clone().map(|text| {
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(text)
                    .into_any_element()
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.check_project_double_click(&id) {
                    this.start_project_rename(id.clone(), name.clone(), window, cx);
                } else {
                    this.focus_project_from_sidebar(id.clone(), true, cx);
                }
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod closed_tests {
    use super::{closed_sessions, order_of_owner};
    use okena_workspace::state::agent_order::{AgentDrop, plan_drop};

    #[test]
    fn a_move_is_indexed_in_the_owning_daemons_own_order() {
        // The mirror holds two daemons' entries. The local daemon's order is
        // repo, a, b; the index sent to it must count only those.
        let mirror: Vec<String> = [
            "remote:far:x",
            "remote:local:repo",
            "remote:far:y",
            "remote:local:a",
            "remote:local:b",
        ]
        .map(String::from)
        .to_vec();
        let local = order_of_owner(&mirror, "remote:local:b");
        assert_eq!(
            local,
            ["remote:local:repo", "remote:local:a", "remote:local:b"]
        );
        // Dropping b on the pinned a: index 1 of the daemon's own
        // [repo, a, b], where a sits — not 3, where it sits in the mirror.
        let roots = vec![
            ("remote:local:a".to_string(), true),
            ("remote:local:b".to_string(), false),
        ];
        let plan = plan_drop(
            "remote:local:b",
            &AgentDrop::BeforePinned("remote:local:a".into()),
            &roots,
            &local,
        )
        .unwrap();
        assert_eq!((plan.move_to, plan.toggle_pinned), (Some(1), true));
        // Ids with no prefix — a sidebar over its own workspace — are one
        // owner of their own.
        assert_eq!(
            order_of_owner(&["a".to_string(), "remote:far:x".to_string()], "a"),
            ["a"]
        );
    }
    use okena_workspace::state::ProjectData;

    fn session(id: &str, closed_at: Option<u64>) -> ProjectData {
        let mut p: ProjectData = serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "path": "/p", "custom_session": "goal",
        }))
        .unwrap();
        p.closed_at = closed_at;
        p
    }

    #[test]
    fn the_history_lists_closed_sessions_newest_closed_first() {
        let repo: ProjectData = serde_json::from_value(serde_json::json!({
            "id": "repo", "name": "repo", "path": "/p/repo", "closed_at": 9,
        }))
        .unwrap();
        let projects = vec![
            session("old", Some(100)),
            session("live", None),
            session("new", Some(300)),
            repo,
            session("mid", Some(200)),
        ];
        let ids: Vec<&str> = closed_sessions(&projects)
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        // Live sessions stay in the Agents list; a project that is not an
        // agent session has no place in either.
        assert_eq!(ids, ["new", "mid", "old"]);
    }
}
