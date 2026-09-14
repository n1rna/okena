//! Harness settings: where agent sessions run, which agent to launch, and how
//! okena's MCP server reaches it.
//!
//! These were previously reachable only by hand-editing `settings.json`.

use crate::settings::{SettingsState, settings_entity};
use crate::theme::{ThemeColors, theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_ms};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_workspace::settings::{
    ClaudePermissionMode, CodexApprovals, CopilotMode, CopilotToolPermissions,
};

use super::SettingsPanel;
use super::components::*;

/// Agents okena can launch and recognize.
///
/// Deliberately only the two whose MCP config flag has been verified against
/// their actual CLI — offering an agent okena cannot wire up would produce
/// sessions that silently lack its tools.
const AGENTS: &[&str] = &["claude", "copilot"];

const EXTRA_ARGS_DESC: &str = "One argument per line. Added to every launch and restart of \
                               this agent, before its brief.";

fn codex_approvals_label(value: CodexApprovals) -> &'static str {
    match value {
        CodexApprovals::Auto => "Auto",
        CodexApprovals::Bypass => "Bypass approvals and sandbox",
    }
}

/// A stacked row offering "Not set" and every value of one agent option.
#[allow(clippy::too_many_arguments)]
fn choice_row<T: Copy + PartialEq + 'static>(
    id: &'static str,
    label: &str,
    desc: &str,
    current: Option<T>,
    all: &'static [T],
    name: fn(T) -> &'static str,
    set: fn(&mut SettingsState, Option<T>, &mut Context<SettingsState>),
    has_border: bool,
    t: &ThemeColors,
    cx: &App,
) -> Stateful<Div> {
    let chips: Vec<AnyElement> = std::iter::once(None)
        .chain(all.iter().copied().map(Some))
        .map(|value| {
            let is_selected = current == value;
            let text = value.map(name).unwrap_or("Not set");
            div()
                .id(SharedString::from(format!("{id}-{text}")))
                .cursor_pointer()
                .px(px(10.0))
                .py(px(3.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(rgb(if is_selected {
                    t.border_active
                } else {
                    t.border
                }))
                .when(is_selected, |d| d.bg(with_alpha(t.button_primary_bg, 0.15)))
                .text_size(ui_text_ms(cx))
                .text_color(rgb(if is_selected {
                    t.text_primary
                } else {
                    t.text_secondary
                }))
                .child(text)
                .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                    settings_entity(cx).update(cx, |state, cx| set(state, value, cx));
                })
                .into_any_element()
        })
        .collect();
    settings_input_row(id, label, desc, t, cx, has_border)
        .child(h_flex().gap(px(6.0)).flex_wrap().children(chips))
}

impl SettingsPanel {
    pub(super) fn render_harness(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let s = settings_entity(cx).read(cx).settings.clone();
        let h = &s.harness;
        let agents = h.agents.clone();
        let selected_agent = h.agent_command.clone();
        let injection = h.agent_mcp_injection;

        // "No agent" first: creating worktrees without launching anything is a
        // legitimate default, and starting an AI agent should be deliberate.
        let mut options: Vec<Option<String>> = vec![None];
        options.extend(AGENTS.iter().map(|a| Some((*a).to_string())));
        // A command configured by hand that isn't in the list must stay
        // visible, or opening this page would silently offer to change it.
        if let Some(current) = selected_agent.as_ref()
            && !AGENTS.contains(&current.as_str())
        {
            options.push(Some(current.clone()));
        }

        let chips: Vec<AnyElement> = options
            .into_iter()
            .map(|option| {
                let is_selected = selected_agent == option;
                let label = option.clone().unwrap_or_else(|| "No agent".to_string());
                let for_click = option.clone();
                div()
                    .id(SharedString::from(format!("harness-agent-{label}")))
                    .cursor_pointer()
                    .px(px(10.0))
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(rgb(if is_selected {
                        t.border_active
                    } else {
                        t.border
                    }))
                    .when(is_selected, |d| d.bg(with_alpha(t.button_primary_bg, 0.15)))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(if is_selected {
                        t.text_primary
                    } else {
                        t.text_secondary
                    }))
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |_, _, _window, cx| {
                            let value = for_click.clone();
                            settings_entity(cx).update(cx, |state: &mut SettingsState, cx| {
                                state.set_harness_agent_command(value.clone(), cx);
                            });
                        }),
                    )
                    .into_any_element()
            })
            .collect();

        v_flex()
            .child(section_header("Projects", &t, cx))
            .child(section_container(&t).child(hook_input_row(
                "harness-agent-root",
                "Projects root directory",
                "Where a multi-project agent session runs, so one agent can see \
                 every worktree. Empty uses the parent of the first project.",
                &self.harness_agent_root_input,
                &t,
                false,
                cx,
            )))
            .child(section_header("Agent", &t, cx))
            .child(
                section_container(&t)
                    .child(
                        v_flex()
                            .px(px(12.0))
                            .py(px(8.0))
                            .gap(px(6.0))
                            .child(
                                v_flex()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(ui_text(13.0, cx))
                                            .text_color(rgb(t.text_primary))
                                            .child("Agent to launch"),
                                    )
                                    .child(
                                        div()
                                            .text_size(ui_text_ms(cx))
                                            .text_color(rgb(t.text_muted))
                                            .child(
                                                "Started in the worktree when you begin work \
                                                 on a task. Also the default in the Start work \
                                                 dialog.",
                                            ),
                                    ),
                            )
                            .child(h_flex().gap(px(6.0)).flex_wrap().children(chips)),
                    ),
            )
            .child(section_header("Claude", &t, cx))
            .child(
                section_container(&t)
                    .child(choice_row(
                        "harness-claude-permission-mode",
                        "Permission mode",
                        "--permission-mode. Not set leaves it to Claude's own settings.",
                        agents.claude.permission_mode,
                        ClaudePermissionMode::ALL,
                        ClaudePermissionMode::cli_value,
                        SettingsState::set_claude_permission_mode,
                        false,
                        &t,
                        cx,
                    ))
                    .child(self.render_toggle(
                        "harness-claude-skip-permissions",
                        "Skip all permission prompts (--dangerously-skip-permissions)",
                        agents.claude.skip_permissions,
                        true,
                        |state, val, cx| state.set_claude_skip_permissions(val, cx),
                        cx,
                    ))
                    .child(hook_input_row(
                        "harness-claude-extra-args",
                        "Extra arguments",
                        EXTRA_ARGS_DESC,
                        &self.harness_claude_extra_args_input,
                        &t,
                        true,
                        cx,
                    )),
            )
            .child(section_header("Copilot", &t, cx))
            .child(
                section_container(&t)
                    .child(choice_row(
                        "harness-copilot-mode",
                        "Mode",
                        "--mode. Not set starts Copilot in its default mode.",
                        agents.copilot.mode,
                        CopilotMode::ALL,
                        CopilotMode::cli_value,
                        SettingsState::set_copilot_mode,
                        false,
                        &t,
                        cx,
                    ))
                    .child(choice_row(
                        "harness-copilot-tools",
                        "Tool permissions",
                        "--allow-all-tools runs tools without asking; --allow-all also \
                         allows every path and URL.",
                        agents.copilot.tool_permissions,
                        CopilotToolPermissions::ALL,
                        CopilotToolPermissions::flag,
                        SettingsState::set_copilot_tool_permissions,
                        true,
                        &t,
                        cx,
                    ))
                    .child(hook_input_row(
                        "harness-copilot-extra-args",
                        "Extra arguments",
                        EXTRA_ARGS_DESC,
                        &self.harness_copilot_extra_args_input,
                        &t,
                        true,
                        cx,
                    )),
            )
            .child(section_header("Codex", &t, cx))
            .child(
                section_container(&t)
                    .child(choice_row(
                        "harness-codex-approvals",
                        "Approvals and sandbox",
                        "Auto is --sandbox workspace-write --ask-for-approval on-request. \
                         Bypass is --dangerously-bypass-approvals-and-sandbox.",
                        agents.codex.approvals,
                        CodexApprovals::ALL,
                        codex_approvals_label,
                        SettingsState::set_codex_approvals,
                        false,
                        &t,
                        cx,
                    ))
                    .child(hook_input_row(
                        "harness-codex-extra-args",
                        "Extra arguments",
                        EXTRA_ARGS_DESC,
                        &self.harness_codex_extra_args_input,
                        &t,
                        true,
                        cx,
                    )),
            )
            .child(section_header("okena MCP", &t, cx))
            .child(
                section_container(&t)
                    .child(self.render_toggle(
                        "harness-mcp-injection",
                        "Give launched agents okena's MCP server",
                        injection,
                        true,
                        |state, val, cx| state.set_harness_agent_mcp_injection(val, cx),
                        cx,
                    ))
                    .child(hook_input_row(
                        "harness-mcp-args",
                        "MCP flag override",
                        "One argument per line; {config} becomes the generated config path. \
                         Empty uses the built-in flags for the selected agent.",
                        &self.harness_agent_mcp_args_input,
                        &t,
                        false,
                        cx,
                    )),
            )
            .child(section_header("Git", &t, cx))
            .child(section_container(&t).child(hook_input_row(
                "harness-worktree-template",
                "Worktree path template",
                "Where a task's worktrees are created. Same setting as Worktree → Path.",
                &self.worktree_dir_suffix_input,
                &t,
                false,
                cx,
            )))
    }
}
