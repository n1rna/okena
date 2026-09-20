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
use okena_workspace::state::AgentSortMode;

use super::{Sidebar, SidebarProjectInfo};

use crate::agent_card::{AgentColor, CardState, agent_color, card_state, subtree_summary};
use crate::item_widgets::sidebar_rename_input;
use okena_ui::rename_state::is_renaming;
use okena_workspace::state::AgentRole;
use okena_workspace::state::agent_tree::{self, AgentNode};
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

/// Reorder `sessions` so each sits under the agent on its task's parent.
///
/// The arrangement itself is `okena_state::agent_tree`, which knows nothing
/// about the sidebar; this only carries the rows through it and copies the
/// depth back on.
fn nest_by_ticket(sessions: Vec<SessionRow>) -> Vec<SessionRow> {
    let nodes: Vec<AgentNode> = sessions
        .iter()
        .map(|row| AgentNode {
            id: row.info.id.clone(),
            task_id: row.task_id.clone(),
            parent_task_id: row.parent_task_id.clone(),
        })
        .collect();
    let placed = agent_tree::arrange(&nodes);

    let mut by_id: HashMap<String, SessionRow> = sessions
        .into_iter()
        .map(|r| (r.info.id.clone(), r))
        .collect();
    placed
        .into_iter()
        .filter_map(|p| {
            let mut row = by_id.remove(&p.id)?;
            row.depth = p.depth;
            Some(row)
        })
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
    /// Whether this is the session currently open in the main area.
    pub(super) focused: bool,
    /// Task key for a task session, change name for a spec session.
    pub(super) subtitle: Option<String>,
    /// Last time anything ran in this session, for activity ordering. `None`
    /// for a session that has not run anything yet.
    pub(super) last_activity_at: Option<u64>,
    /// How it is doing: the terminal's state and the agent's report combined.
    pub(super) card: CardState,
    /// What the agent last said it was doing.
    pub(super) status: Option<String>,
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
            focused: focused_id == Some(p.id.as_str()),
            subtitle,
            last_activity_at: p.last_activity_at,
            card: card_state(running, activity),
            status,
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
                    .children(memory.map(|memory| {
                        div()
                            .flex_shrink_0()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(memory)
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
    /// whether to look inside.
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
                div()
                    .px(px(6.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(if attention { t.warning } else { t.text_muted }))
                    .child(subtree_summary(&child_states)),
            )
            .child(
                v_flex()
                    .w_full()
                    .min_w_0()
                    .gap(px(4.0))
                    .pl(px(6.0))
                    .children(children),
            );
        (group.into_any_element(), used)
    }

    /// The agent sessions in this workspace, ordered by the header's sort.
    pub(super) fn render_agents_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.read(cx);

        let focused_id = self.focus_manager.read(cx).focused_project_id().cloned();

        let mut sessions: Vec<SessionRow> = Vec::new();
        for p in workspace.data().projects.iter() {
            let Some(role) = p.agent_role() else {
                continue;
            };
            // Closed sessions are the history's, behind the header's button.
            if p.is_closed() {
                continue;
            }
            sessions.push(self.live_session_row(p, role, workspace, focused_id.as_deref()));
        }

        let sort_mode = workspace
            .data()
            .window(self.window_id)
            .map(|w| w.agent_sort_mode)
            .unwrap_or_default();
        match sort_mode {
            // Most recent first. A session that has never run anything has no
            // activity stamp and sorts last rather than first, which is where a
            // just-created-but-idle session belongs.
            AgentSortMode::Activity => sessions.sort_by(|a, b| {
                b.last_activity_at
                    .cmp(&a.last_activity_at)
                    .then_with(|| a.info.name.cmp(&b.info.name))
            }),
            AgentSortMode::Name => sessions.sort_by_key(|r| r.info.name.to_lowercase()),
        }

        // The tickets' own hierarchy, applied after sorting so the chosen
        // order survives inside each level. An epic's agent gathers its
        // stories' agents under it however each was started.
        sessions = nest_by_ticket(sessions);

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
                             in Specs, or use + above.",
                        ),
                )
                .into_any_element();
        }

        let mut groups: Vec<AnyElement> = Vec::new();
        let mut i = 0;
        while i < sessions.len() {
            let (group, used) = self.render_agent_group(&sessions[i..], None, cx);
            groups.push(group);
            i += used.max(1);
        }
        v_flex()
            .w_full()
            .gap(px(6.0))
            .px(px(8.0))
            .py(px(4.0))
            .children(groups)
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

        let closed: Vec<ClosedRow> = closed_sessions(&workspace.data().projects)
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
    use super::closed_sessions;
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
