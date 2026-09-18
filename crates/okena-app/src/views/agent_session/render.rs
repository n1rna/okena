//! Rendering for the agent-session panel.
//!
//! One implementation at two densities. The sections are the same in both and
//! in the same order — status, subject, where the work lands, what came out —
//! so a session reads identically whether you meet it in the sidebar or in the
//! overview.

use super::model::short_path;
use super::{
    AgentSessionInfo, AgentSessionKind, AgentSessionPanel, DeleteChoice, PanelTab, SessionActivity,
};
use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_ms};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};

impl AgentSessionPanel {
    fn chip(&self, text: String, color: u32, cx: &App) -> AnyElement {
        div()
            .flex_shrink_0()
            .px(px(6.0))
            .py(px(1.0))
            .rounded(px(3.0))
            .bg(with_alpha(color, 0.15))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(color))
            .child(text)
            .into_any_element()
    }

    fn section_heading(&self, label: &str, count: Option<usize>, cx: &App) -> AnyElement {
        let t = theme(cx);
        h_flex()
            .items_center()
            .justify_between()
            .pt(px(8.0))
            .pb(px(2.0))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(label.to_string()),
            )
            // The count is optional: a bare "0" beside a heading that is not a
            // list reads as "none found" rather than "not countable".
            .children(count.map(|n| {
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(format!("{n}"))
                    .into_any_element()
            }))
            .into_any_element()
    }

    fn note(&self, text: impl Into<String>, cx: &App) -> AnyElement {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(text.into())
            .into_any_element()
    }

    /// Restart, stop and start as icon buttons in one small segment, beside
    /// the session facts so they line up with them.
    ///
    /// Restart resumes the conversation; Start begins again from the brief.
    /// Both are offered while stopped because they mean different things.
    fn render_controls(&self, info: &AgentSessionInfo, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let button = |id: &'static str, icon: &'static str, tip: &'static str| {
            okena_ui::icon_button::icon_button_sized(id, icon, 24.0, 14.0, &t).tooltip(
                move |window, cx| gpui_component::tooltip::Tooltip::new(tip).build(window, cx),
            )
        };
        let mut row = h_flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(2.0))
            .p(px(2.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_secondary));
        if info.resumable {
            let tip = if info.running {
                "Restart agent, resuming its conversation"
            } else {
                "Resume session"
            };
            row = row.child(
                button("agent-panel-resume", "icons/refresh.svg", tip)
                    .on_click(cx.listener(|this, _, _window, cx| this.resume_agent(cx))),
            );
        }
        row = if info.running {
            row.child(
                button("agent-panel-stop", "icons/stop.svg", "Stop agent")
                    .on_click(cx.listener(|this, _, _window, cx| this.stop_agent(cx))),
            )
        } else {
            row.child(
                button(
                    "agent-panel-start",
                    "icons/play.svg",
                    "Start agent again from its brief",
                )
                .on_click(cx.listener(|this, _, _window, cx| this.restart_agent(cx))),
            )
        };
        row.into_any_element()
    }

    /// The working directory as a chip: its end on the chip, all of it on
    /// hover, and a click copies it.
    fn path_chip(&self, path: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let full = path.to_string();
        let tip = SharedString::from(format!("{path}\nClick to copy"));
        h_flex()
            .id(SharedString::from(format!(
                "agent-path-{}",
                self.project_id
            )))
            .min_w_0()
            .items_center()
            .gap(px(5.0))
            .px(px(7.0))
            .py(px(2.0))
            .rounded(px(10.0))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_secondary))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .child(
                svg()
                    .path("icons/folder.svg")
                    .flex_shrink_0()
                    .size(px(11.0))
                    .text_color(rgb(t.text_muted)),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(short_path(path, 2)),
            )
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .on_click(move |_, _window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(full.clone()));
                crate::workspace::toast::ToastManager::success("Copied the working directory", cx);
            })
            .into_any_element()
    }

    /// The task this agent works on, as a card: click to open it in Tasks,
    /// with its link one click away to copy or open in the provider.
    fn task_card(&self, task: &okena_core::tasks::TaskRef, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let external_id = task.id.external_id.clone();
        let task_provider = task.id.provider.clone();
        let open_url = task.url.clone();
        let copy_url = task.url.clone();
        let provider = crate::views::harness::provider_label(&task.id.provider).to_string();
        let open_tip = SharedString::from(format!("Open in {provider}"));
        let has_url = !task.url.is_empty();

        v_flex()
            .id(SharedString::from(format!(
                "agent-task-{}-{}",
                self.project_id, task.id.external_id
            )))
            .cursor_pointer()
            .w_full()
            .min_w_0()
            .gap(px(4.0))
            .px(px(10.0))
            .py(px(8.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_secondary))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap(px(6.0))
                    .child(self.chip(task.display_key.clone(), t.button_primary_bg, cx))
                    .children(task.parent_key.as_ref().map(|key| {
                        div()
                            .flex_shrink_0()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(format!("↳ {key}"))
                            .into_any_element()
                    }))
                    .child(div().flex_1().min_w_0())
                    .when(has_url, |row| {
                        row.child(
                            okena_ui::icon_button::icon_button_sized(
                                "agent-task-copy-link",
                                "icons/link.svg",
                                22.0,
                                13.0,
                                &t,
                            )
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new("Copy link").build(window, cx)
                            })
                            .on_click(move |_, _window, cx| {
                                // The card opens the task; this button only copies.
                                cx.stop_propagation();
                                cx.write_to_clipboard(ClipboardItem::new_string(copy_url.clone()));
                                crate::workspace::toast::ToastManager::success(
                                    "Copied the task link",
                                    cx,
                                );
                            }),
                        )
                        .child(
                            okena_ui::icon_button::icon_button_sized(
                                "agent-task-open-provider",
                                "icons/external-link.svg",
                                22.0,
                                13.0,
                                &t,
                            )
                            .tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(open_tip.clone())
                                    .build(window, cx)
                            })
                            .on_click(move |_, _window, cx| {
                                cx.stop_propagation();
                                okena_core::process::open_url(&open_url);
                            }),
                        )
                    }),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .line_clamp(2)
                    .text_size(ui_text(13.0, cx))
                    .text_color(rgb(t.text_primary))
                    .child(task.title.clone()),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child("Open in Tasks")
                    .child(
                        svg()
                            .path("icons/chevron-right.svg")
                            .size(px(10.0))
                            .text_color(rgb(t.text_muted)),
                    ),
            )
            .on_click(cx.listener(move |this, _, _window, cx| {
                let external_id = external_id.clone();
                let provider = task_provider.clone();
                this.request_broker.update(cx, |broker, cx| {
                    broker.push_workbench_request(
                        crate::workspace::requests::WorkbenchRequest::OpenTask {
                            provider,
                            external_id,
                        },
                        cx,
                    );
                });
            }))
            .into_any_element()
    }

    /// The status pill: what the terminal says, not what the agent claims.
    ///
    /// An agent that stopped reporting still shows as waiting when its prompt
    /// is waiting, which is the state you actually need to act on.
    fn status_pill(&self, info: &AgentSessionInfo, cx: &App) -> AnyElement {
        let t = theme(cx);
        let activity = info.activity();
        self.chip(activity.label(), activity.color(&t), cx)
    }

    fn asset_row(&self, asset: &okena_core::session_assets::SessionAsset, cx: &App) -> AnyElement {
        crate::views::components::render_asset_row(asset, theme(cx).bg_primary, cx)
    }

    /// The scrolling body: every section, in the same order at both densities.
    fn render_info(&self, info: &AgentSessionInfo, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);

        let mut body = v_flex()
            .id(SharedString::from(format!(
                "agent-panel-body-{}",
                info.project_id
            )))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(10.0))
            .pb(px(12.0))
            .gap(px(4.0));

        // ── Session ──────────────────────────────────────────────────────────
        let mut facts = vec![self.chip(info.kind.label().to_string(), t.button_primary_bg, cx)];
        if let Some(agent) = &info.agent {
            facts.push(self.chip(agent.clone(), t.text_secondary, cx));
        }
        if let Some(closed_at) = info.closed_at {
            let ago = okena_ui::ago::format_ago(closed_at, okena_ui::ago::now_millis());
            facts.push(self.chip(format!("closed {ago}"), t.text_muted, cx));
        } else {
            facts.push(if info.mcp {
                self.chip("okena mcp".to_string(), t.success, cx)
            } else {
                // Not an error: an agent started outside okena simply was not
                // handed the config, so it cannot report back.
                self.chip("no okena mcp".to_string(), t.text_muted, cx)
            });
        }

        body = body
            .child(self.section_heading("SESSION", None, cx))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .justify_between()
                    .gap(px(6.0))
                    .child(h_flex().min_w_0().gap(px(4.0)).flex_wrap().children(facts))
                    // A closed session has no agent to act on: bringing it
                    // back is the closed view's one button, beside this panel.
                    .when(!info.closed(), |row| {
                        row.child(self.render_controls(info, cx))
                    }),
            )
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .pt(px(4.0))
                    .child(self.path_chip(&info.root, cx)),
            );

        // What it needs from you, first: an agent waiting on a decision is
        // the one thing on this panel that is blocking work.
        let activity = info.activity();
        if let SessionActivity::NeedsAttention { reason } = activity {
            let accent = activity.color(&t);
            let mut attention = v_flex()
                .mt(px(4.0))
                .gap(px(4.0))
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(4.0))
                .border_l_2()
                .border_color(rgb(accent))
                .bg(with_alpha(accent, 0.1))
                .child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(accent))
                        .child(reason.label().to_uppercase()),
                );
            if let Some(question) = &info.question {
                attention = attention.child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_primary))
                        .child(question.clone()),
                );
            }
            body = body.child(attention);
        }

        // What a closed agent last said, since nothing is running to show it:
        // where it was when it was closed is what you reopen it to continue.
        if info.closed() {
            body = body.child(self.render_last_report(info, cx));
        }

        // The agent's own suggested next steps, as one-click instructions.
        // Only these: anything else you want to say, you say in its terminal,
        // where the conversation is — a second text box here was a worse copy
        // of that prompt.
        if info.running && !info.suggestions.is_empty() {
            body = body.child(self.render_suggestions(info, cx));
        }

        // ── What it is working on ────────────────────────────────────────────
        if !info.tasks.is_empty() {
            // Every task it was started on, the one it is named after first: a
            // session on several picked tasks is not a session on the first.
            body = body.child(match info.tasks.len() {
                1 => self.section_heading("TASK", None, cx),
                n => self.section_heading("TASKS", Some(n), cx),
            });
            for task in &info.tasks {
                body = body.child(self.task_card(task, cx));
            }
        } else if let Some(subject) = info.kind.subject() {
            let heading = match info.kind {
                AgentSessionKind::Spec { .. } => "CHANGE",
                _ => "GOAL",
            };
            body = body.child(self.section_heading(heading, None, cx)).child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(subject),
            );
        }

        // ── The breakdown around it ──────────────────────────────────────────
        if info.parent.is_some() || !info.children.is_empty() {
            if let Some(parent) = &info.parent {
                body = body
                    .child(self.section_heading("PARENT AGENT", None, cx))
                    .child(self.related_agent_row(parent, cx));
            }
            if !info.children.is_empty() {
                let waiting = info
                    .children
                    .iter()
                    .filter(|c| c.activity.wants_attention())
                    .count();
                body =
                    body.child(self.section_heading("SUB-AGENTS", Some(info.children.len()), cx));
                // Said once above the list: which of several sub-agents is
                // waiting is the question you open a parent's panel to answer.
                if waiting > 0 {
                    body = body.child(self.note(format!("{waiting} waiting on you",), cx));
                }
                for child in &info.children {
                    body = body.child(self.related_agent_row(child, cx));
                }
            }
        }

        // ── Where the work lands ─────────────────────────────────────────────
        body = body.child(self.section_heading(
            info.kind.workspace_heading(),
            Some(info.workspaces.len()),
            cx,
        ));
        if info.workspaces.is_empty() {
            body = body.child(self.note(info.kind.empty_workspace_note(), cx));
        }
        for w in &info.workspaces {
            // Read fresh rather than from the session's snapshot: the card
            // shows push and review state, which the session model does not
            // carry and which changes without the session changing.
            let Some(summary) = crate::views::components::WorktreeSummary::collect(
                self.workspace.read(cx),
                &w.project_id,
            ) else {
                continue;
            };
            body = body.child(crate::views::components::render_worktree_card(
                &summary,
                &self.request_broker,
                |this, id, cx| this.open_project(id.to_string(), cx),
                |this, id, mode, cx| this.open_diff(id, mode, cx),
                cx,
            ));
        }

        // ── What came out ────────────────────────────────────────────────────
        body = body.child(self.section_heading("PRODUCED", Some(info.assets.len()), cx));
        if info.assets.is_empty() {
            body = body.child(self.note(
                if info.mcp {
                    // Distinguishes "produced nothing" from "cannot tell us",
                    // which look identical without this.
                    "Nothing produced yet."
                } else {
                    "Nothing detected, and cannot report — no okena MCP."
                },
                cx,
            ));
        }
        for asset in &info.assets {
            body = body.child(self.asset_row(asset, cx));
        }

        // ── Teardown ─────────────────────────────────────────────────────────
        //
        // Close beside Delete: both end the session's place in the Agents
        // list, but Close keeps everything and Delete takes it down.
        if self.density.allows_delete() {
            body = body.child(match self.pending_delete {
                Some(choice) => self.render_delete_confirm(info, choice, cx),
                None => h_flex()
                    .mt(px(10.0))
                    .gap(px(4.0))
                    .when(!info.closed(), |row| {
                        row.child(
                            div()
                                .id("agent-panel-close")
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(px(4.0))
                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_secondary))
                                .child("Close")
                                .tooltip(|window, cx| {
                                    gpui_component::tooltip::Tooltip::new(
                                        "Stop the agent and move it to closed agents, \
                                         keeping its worktrees and everything it had",
                                    )
                                    .build(window, cx)
                                })
                                // No confirm card: nothing is lost, and the
                                // closed view resumes it.
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _window, cx| this.close_session(cx)),
                                ),
                        )
                    })
                    .child(
                        div()
                            .id("agent-panel-delete")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .hover(|s| s.bg(with_alpha(t.error, 0.1)))
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.error))
                            .child("Delete")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _window, cx| {
                                    // Two steps on purpose: this removes
                                    // checkouts. The defaults come back every
                                    // time it opens.
                                    this.pending_delete = Some(DeleteChoice::default());
                                    cx.notify();
                                }),
                            ),
                    )
                    .into_any_element(),
            });
        }

        body.into_any_element()
    }

    /// What a closed agent last reported: its state, its status line, its
    /// question and what it suggested — read-only, with nothing to send them to.
    fn render_last_report(&self, info: &AgentSessionInfo, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let mut report = v_flex().gap(px(4.0)).child(self.section_heading("LAST REPORT", None, cx));
        if info.reported.is_none() && info.status.is_none() && info.question.is_none() {
            return report
                .child(self.note("It reported nothing before it was closed.", cx))
                .into_any_element();
        }
        let state = info.reported.map(|state| {
            let color = if state.wants_attention() {
                t.warning
            } else {
                t.text_secondary
            };
            self.chip(state.label().to_string(), color, cx)
        });
        report = report.child(
            h_flex()
                .w_full()
                .min_w_0()
                .items_start()
                .gap(px(6.0))
                .children(state)
                .children(info.status.clone().map(|status| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_secondary))
                        .child(status)
                        .into_any_element()
                })),
        );
        if let Some(question) = &info.question {
            report = report.child(
                div()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .border_l_2()
                    .border_color(rgb(t.border))
                    .bg(rgb(t.bg_secondary))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_primary))
                    .child(question.clone()),
            );
        }
        if !info.suggestions.is_empty() {
            report = report.child(
                h_flex().gap(px(4.0)).flex_wrap().children(
                    info.suggestions
                        .iter()
                        .map(|s| self.chip(s.label.clone(), t.text_muted, cx)),
                ),
            );
        }
        report.into_any_element()
    }

    /// The agent's suggested next steps, each a button that sends it.
    fn render_suggestions(&self, info: &AgentSessionInfo, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let mut row = h_flex().mt(px(4.0)).gap(px(4.0)).flex_wrap();
        for (i, suggestion) in info.suggestions.iter().enumerate() {
            let instruction = suggestion.instruction.clone();
            row = row.child(
                div()
                    .id(SharedString::from(format!(
                        "agent-suggest-{}-{i}",
                        info.project_id
                    )))
                    .cursor_pointer()
                    .flex_shrink_0()
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .bg(rgb(t.button_primary_bg))
                    .hover(|s| s.bg(rgb(t.button_primary_hover)))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.button_primary_fg))
                    .child(suggestion.label.clone())
                    .tooltip({
                        let text: SharedString = suggestion.instruction.clone().into();
                        move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(text.clone()).build(window, cx)
                        }
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.send_instruction(instruction.clone(), cx);
                        }),
                    ),
            );
        }
        row.into_any_element()
    }

    /// One neighbouring agent: which ticket, how it is doing, and a way in.
    fn related_agent_row(&self, agent: &super::RelatedAgent, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let id = agent.project_id.clone();
        let color = agent.activity.color(&t);
        h_flex()
            .id(SharedString::from(format!(
                "related-agent-{}",
                agent.project_id
            )))
            .cursor_pointer()
            .w_full()
            .min_w_0()
            .items_center()
            .gap(px(6.0))
            .px(px(8.0))
            .py(px(5.0))
            .rounded(px(4.0))
            .bg(rgb(t.bg_secondary))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            // The rows that need you stand out without reading every label.
            .when(agent.activity.wants_attention(), |d| {
                d.border_l_2().border_color(rgb(color))
            })
            .child(
                div()
                    .flex_shrink_0()
                    .size(px(6.0))
                    .rounded_full()
                    .bg(rgb(color)),
            )
            .children(agent.key.clone().map(|key| {
                div()
                    .flex_shrink_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(key)
                    .into_any_element()
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_primary))
                    .child(agent.name.clone()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(color))
                    .child(agent.activity.label()),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.open_project(id.clone(), cx);
                }),
            )
            .into_any_element()
    }

    /// The armed delete, shown in place of the button.
    fn render_delete_confirm(
        &self,
        info: &AgentSessionInfo,
        choice: DeleteChoice,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let count = info.workspaces.len();
        let title = match count {
            0 => "Delete this session? It has no checkouts.".to_string(),
            1 => "Delete this session and its 1 checkout?".to_string(),
            n => format!("Delete this session and its {n} checkouts?"),
        };

        let mut card = v_flex()
            .mt(px(10.0))
            .gap(px(10.0))
            .p(px(12.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(with_alpha(t.error, 0.5))
            .bg(with_alpha(t.error, 0.06))
            .child(
                div()
                    .text_size(ui_text(13.0, cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_primary))
                    .child(title),
            );

        // Nothing on disk to choose about.
        if count > 0 {
            let mut switches = v_flex().gap(px(8.0)).child(self.delete_switch_row(
                "agent-panel-delete-worktrees",
                "Remove worktrees",
                choice.remove_worktrees,
                DeleteChoice {
                    remove_worktrees: !choice.remove_worktrees,
                    // Turning it back on starts the branch switch off again.
                    delete_branches: false,
                },
                cx,
            ));
            if choice.remove_worktrees {
                switches = switches.child(self.delete_switch_row(
                    "agent-panel-delete-branches",
                    "Delete local branches",
                    choice.delete_branches,
                    DeleteChoice {
                        delete_branches: !choice.delete_branches,
                        ..choice
                    },
                    cx,
                ));
            }
            card = card.child(switches);
        }

        card.child(
            h_flex()
                .justify_end()
                .gap(px(6.0))
                .child(
                    div()
                        .id("agent-panel-delete-cancel")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .bg(rgb(t.bg_secondary))
                        .hover(|s| s.bg(rgb(t.bg_hover)))
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_primary))
                        .child("Cancel")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                this.pending_delete = None;
                                cx.notify();
                            }),
                        ),
                )
                .child(
                    div()
                        .id("agent-panel-delete-go")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .bg(rgb(t.error))
                        .hover(|s| s.bg(with_alpha(t.error, 0.85)))
                        .text_size(ui_text_ms(cx))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(t.button_primary_fg))
                        .child("Delete")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                this.delete_workspace(choice, cx);
                            }),
                        ),
                ),
        )
        .into_any_element()
    }

    /// One labelled switch on the delete card, laid out like a settings row:
    /// label on the left, switch on the right. Clicking anywhere on the row
    /// sets the card to `next`.
    fn delete_switch_row(
        &self,
        id: &'static str,
        label: &'static str,
        enabled: bool,
        next: DeleteChoice,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        h_flex()
            .id(id)
            .cursor_pointer()
            .items_center()
            .justify_between()
            .gap(px(16.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(label),
            )
            .child(okena_ui::toggle::toggle_switch(
                format!("{id}-toggle"),
                enabled,
                &t,
            ))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.pending_delete = Some(next);
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// Header: who this session is, how it is doing, and the way into it.
    fn render_header(&self, info: &AgentSessionInfo, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let open_id = info.project_id.clone();

        let mut header = v_flex()
            .gap(px(6.0))
            .px(px(10.0))
            .py(px(8.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .child(
                h_flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_text(13.0, cx))
                            .text_color(rgb(t.text_primary))
                            .child(info.name.clone()),
                    )
                    .child(self.status_pill(info, cx)),
            );

        // The tab strip only exists where the terminal isn't already on screen,
        // which is exactly where the panel draws its own header.
        {
            let tab = |this: &Self,
                       label: &'static str,
                       value: PanelTab,
                       cx: &mut Context<Self>|
             -> AnyElement {
                let selected = this.tab == value;
                div()
                    .id(ElementId::Name(
                        format!("agent-tab-{}-{label}", this.project_id).into(),
                    ))
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .when(selected, |d| {
                        d.bg(with_alpha(t.button_primary_bg, 0.2))
                            .text_color(rgb(t.text_primary))
                    })
                    .when(!selected, |d| {
                        d.text_color(rgb(t.text_muted))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                    })
                    .text_size(ui_text_ms(cx))
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.tab = value;
                            cx.notify();
                        }),
                    )
                    .into_any_element()
            };
            header = header.child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.0))
                    .child(
                        h_flex()
                            .gap(px(2.0))
                            .child(tab(self, "Info", PanelTab::Info, cx))
                            .child(tab(self, "Terminal", PanelTab::Terminal, cx)),
                    )
                    .child(
                        div()
                            .id(ElementId::Name(
                                format!("agent-open-{}", info.project_id).into(),
                            ))
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .bg(rgb(t.bg_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_primary))
                            .child("Open")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _window, cx| {
                                    this.open_project(open_id.clone(), cx);
                                }),
                            ),
                    ),
            );
        }

        header.into_any_element()
    }
}

impl Render for AgentSessionPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let Some(info) = self.info(cx) else {
            // The session went away — deleted, or its window closed. Say so
            // rather than rendering an empty shell that looks like a loading
            // state that never resolves.
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .bg(rgb(t.bg_primary))
                .child(self.note("This session is gone.", cx));
        };
        self.fetch_task_states(&info.assets, cx);

        // Bind the terminal here rather than only on the tab switch: a session
        // that has just restarted has no terminal in the snapshot yet, and its
        // layout arrives a frame or two later.
        if self.density.has_header() && self.tab == PanelTab::Terminal {
            self.sync_terminal(cx);
        }

        let body = if self.density.has_header() && self.tab == PanelTab::Terminal {
            self.render_terminal(cx)
        } else {
            self.render_info(&info, cx)
        };

        let mut root = v_flex().size_full().bg(rgb(t.bg_primary));
        if self.density.has_header() {
            root = root.child(self.render_header(&info, cx));
        }
        root.child(body)
    }
}
