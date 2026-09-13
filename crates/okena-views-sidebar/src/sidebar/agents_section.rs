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
use okena_workspace::state::AgentRole;
use okena_workspace::state::agent_tree::{self, AgentNode};
use std::collections::HashMap;

/// Colour for a role's badge.
///
/// Distinct hues rather than shades of one, so the kinds are told apart at a
/// glance in a mixed list. Implementing is the primary colour because it is
/// the one that means work is actually happening.
fn role_color(role: AgentRole, t: &okena_ui::theme::ThemeColors) -> u32 {
    match role {
        AgentRole::Implement => t.button_primary_bg,
        AgentRole::Task => t.term_cyan,
        AgentRole::Spec => t.success,
        AgentRole::Knowledge => t.term_magenta,
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
struct SessionRow {
    info: SidebarProjectInfo,
    role: AgentRole,
    /// Provider ids of the session's task and its task's parent, for placing
    /// it in the hierarchy.
    task_id: Option<String>,
    parent_task_id: Option<String>,
    /// How deep the ticket hierarchy puts it. Filled in after sorting.
    depth: usize,
    /// Whether this is the session currently open in the main area.
    focused: bool,
    /// Task key for a task session, change name for a spec session.
    subtitle: Option<String>,
    /// Last time anything ran in this session, for activity ordering. `None`
    /// for a session that has not run anything yet.
    last_activity_at: Option<u64>,
    /// How it is doing: the terminal's state and the agent's report combined.
    card: CardState,
    /// What the agent last said it was doing.
    status: Option<String>,
}

impl Sidebar {
    /// Colour for a card state: the attention states stand out, the rest
    /// recede so the ones that need you are the ones you see.
    fn card_color(state: CardState, t: &okena_ui::theme::ThemeColors) -> u32 {
        match state {
            CardState::NeedsInput | CardState::ReadyForReview => t.warning,
            CardState::Blocked => t.error,
            CardState::Working => t.success,
            CardState::Waiting | CardState::Done | CardState::Stopped => t.text_muted,
        }
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
        let role = row.role;
        let kind_color = role_color(role, &t);
        let state_color = Self::card_color(row.card, &t);
        let attention = row.card.wants_attention();
        let focused = row.focused;
        let body = row.status.clone().or_else(|| row.subtitle.clone());
        // Each agent's own colour, faint enough that the state and role colours
        // on top of it still read. The border carries more of it than the fill
        // so neighbouring cards separate at a glance.
        let AgentColor { hue, lightness } = color;
        let tint = hsla(hue, 0.65, lightness, 0.09);
        let tint_hover = hsla(hue, 0.65, lightness, 0.16);
        let edge = hsla(hue, 0.65, lightness, if nested { 0.35 } else { 0.45 });

        v_flex()
            .id(SharedString::from(format!("agent-session-{id}")))
            .cursor_pointer()
            .w_full()
            .min_w_0()
            .gap(px(3.0))
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(if focused {
                hsla(hue, 0.65, lightness, 1.0)
            } else if attention {
                okena_ui::theme::with_alpha(state_color, 0.6)
            } else {
                edge
            })
            .bg(tint)
            .hover(move |s| s.bg(tint_hover))
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
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(okena_ui::tokens::ui_text(13.0, cx))
                            .text_color(rgb(t.text_primary))
                            .child(row.info.name.clone()),
                    )
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
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.focus_project_from_sidebar(id.clone(), true, cx);
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
        // Keyed by the ticket family the tasks view colours by, so an agent
        // matches the task it works on: the parent ticket when there is one,
        // then its own ticket, and only a session without a ticket by its id.
        let key = root
            .parent_task_id
            .as_deref()
            .or(root.task_id.as_deref())
            .unwrap_or(&root.info.id);
        let color = agent_color(key, family);
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
        // The container takes its parent agent's colour, so the group reads as
        // that agent's work.
        let AgentColor { hue, lightness } = color;
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
                hsla(hue, 0.65, lightness, 0.3)
            })
            .bg(hsla(hue, 0.65, lightness, 0.04))
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
            // Spec first: a session can only be one kind, and checking the
            // narrower marker first keeps that obvious.
            let Some(role) = p.agent_role() else {
                continue;
            };
            // What it is working on, when that is not already the name. The
            // subtitle used to repeat it — a row read "add-login (spec)" over
            // "add-login" — which cost a line to say nothing.
            let subtitle = match role {
                AgentRole::Spec => p.spec_change.clone(),
                AgentRole::Knowledge => p.custom_session.clone(),
                AgentRole::Task | AgentRole::Implement => {
                    p.task_ref.as_ref().map(|t| t.display_key.clone())
                }
                AgentRole::Custom => p.custom_session.clone(),
            };
            let mut info = SidebarProjectInfo::from_project(p, workspace, self.window_id);
            // The badge says "spec"; the name saying "(spec)" as well says it
            // twice. Stripped at display rather than left to a migration: the
            // name is the user's to rename, and rewriting it under them would
            // be worse than showing it tidily.
            if let Some(suffix) = role.legacy_name_suffix()
                && let Some(trimmed) = info.name.strip_suffix(suffix)
            {
                info.name = trimmed.to_string();
            }
            let subtitle = subtitle.filter(|s| s != &info.name);
            // Live from the registry, the same signal the project rows use.
            let (running, waiting) = {
                let registry = self.terminals.lock();
                let live: Vec<_> = info
                    .terminal_ids
                    .iter()
                    .filter_map(|tid| registry.get(tid))
                    .collect();
                (
                    !live.is_empty(),
                    live.iter().any(|t| t.is_waiting_for_input()),
                )
            };
            let reported = p.agent.as_ref().and_then(|a| a.state);
            let status = p
                .agent
                .as_ref()
                .and_then(|a| a.status.clone())
                .filter(|s| !s.trim().is_empty());
            sessions.push(SessionRow {
                info,
                role,
                task_id: p.task_ref.as_ref().map(|t| t.id.external_id.clone()),
                parent_task_id: p.task_ref.as_ref().and_then(|t| t.parent_id.clone()),
                depth: 0,
                focused: focused_id.as_deref() == Some(p.id.as_str()),
                subtitle,
                last_activity_at: p.last_activity_at,
                card: card_state(running, waiting, reported),
                status,
            });
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
}
