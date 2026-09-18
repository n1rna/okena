//! The permission options and extra arguments configured for an agent.
//!
//! One table for every launch route and for restarts, so an agent started
//! without permission prompts keeps running without them wherever it was
//! started from. These go alongside the brief, never in place of it.

use crate::workspace::persistence::AppSettings;
use okena_core::agent_model;

/// Which model a launch runs on: what its brief's template names, and what the
/// person picked for this launch alone.
#[derive(Clone, Debug, Default)]
pub(super) struct LaunchModel {
    pub(super) models: agent_model::AgentModels,
    /// `None` when nothing was picked, `Some("")` for the CLI's default.
    pub(super) picked: Option<String>,
}

impl LaunchModel {
    pub(super) fn new(models: agent_model::AgentModels, picked: Option<String>) -> Self {
        Self { models, picked }
    }

    /// The model `command` is launched with, if any.
    pub(super) fn for_agent(&self, command: &str) -> Option<String> {
        agent_model::resolve(&self.models, command, self.picked.as_deref())
    }
}

/// Arguments from `harness.agents` for the agent `command` runs, and the
/// `model` this launch runs on.
///
/// Matched on the program's file stem like `session_args` and `prompt_args`.
/// Empty for a command okena does not know, and for an agent with nothing
/// set. Flags only: callers place them before the positional prompt.
///
/// `model` is what the launch resolved (see [`okena_core::agent_model`]) and
/// goes in as the CLI's model flag. A model the person put in the extra
/// arguments by hand is theirs: it is left alone and not passed twice.
pub(super) fn option_args(
    command: &str,
    settings: &AppSettings,
    model: Option<&str>,
) -> Vec<String> {
    let agents = &settings.harness.agents;
    let mut args: Vec<String> = Vec::new();
    let extra = match super::agent_resume::program(command).as_str() {
        "claude" => {
            let o = &agents.claude;
            if let Some(mode) = o.permission_mode {
                args.push("--permission-mode".into());
                args.push(mode.cli_value().into());
            }
            if o.skip_permissions {
                args.push("--dangerously-skip-permissions".into());
            }
            &o.extra_args
        }
        "copilot" => {
            let o = &agents.copilot;
            if let Some(mode) = o.mode {
                args.push("--mode".into());
                args.push(mode.cli_value().into());
            }
            if let Some(tools) = o.tool_permissions {
                args.push(tools.flag().into());
            }
            &o.extra_args
        }
        "codex" => {
            let o = &agents.codex;
            if let Some(approvals) = o.approvals {
                args.extend(approvals.args().iter().map(|a| a.to_string()));
            }
            &o.extra_args
        }
        _ => return Vec::new(),
    };
    if let (Some(model), Some(flag)) = (
        model.map(str::trim).filter(|m| !m.is_empty()),
        agent_model::flag(command),
    ) && agent_model::model_in(command, extra).is_none()
    {
        args.push(flag.into());
        args.push(model.into());
    }
    args.extend(
        extra
            .iter()
            .map(|a| a.trim())
            .filter(|a| !a.is_empty())
            .map(str::to_string),
    );
    args
}

#[cfg(test)]
mod tests {
    use super::option_args;
    use crate::workspace::persistence::AppSettings;
    use okena_workspace::settings::{
        ClaudePermissionMode, CodexApprovals, CopilotMode, CopilotToolPermissions,
    };

    #[test]
    fn nothing_set_adds_nothing() {
        let s = AppSettings::default();
        for agent in ["claude", "copilot", "codex"] {
            assert!(option_args(agent, &s, None).is_empty(), "{agent}");
        }
    }

    #[test]
    fn every_claude_permission_mode_is_passed_by_its_cli_name() {
        let mut s = AppSettings::default();
        for (mode, value) in [
            (ClaudePermissionMode::Manual, "manual"),
            (ClaudePermissionMode::AcceptEdits, "acceptEdits"),
            (ClaudePermissionMode::Auto, "auto"),
            (ClaudePermissionMode::Plan, "plan"),
            (ClaudePermissionMode::DontAsk, "dontAsk"),
            (ClaudePermissionMode::BypassPermissions, "bypassPermissions"),
        ] {
            s.harness.agents.claude.permission_mode = Some(mode);
            assert_eq!(
                option_args("claude", &s, None),
                ["--permission-mode", value]
            );
        }
    }

    #[test]
    fn claude_options_combine_in_a_fixed_order_and_match_a_path() {
        let mut s = AppSettings::default();
        s.harness.agents.claude.skip_permissions = true;
        assert_eq!(
            option_args("/usr/local/bin/claude", &s, None),
            ["--dangerously-skip-permissions"]
        );
        s.harness.agents.claude.permission_mode = Some(ClaudePermissionMode::Plan);
        s.harness.agents.claude.extra_args = vec!["--verbose".into(), "  ".into()];
        assert_eq!(
            option_args("claude", &s, None),
            [
                "--permission-mode",
                "plan",
                "--dangerously-skip-permissions",
                "--verbose"
            ]
        );
    }

    #[test]
    fn copilot_mode_and_tool_permissions() {
        let mut s = AppSettings::default();
        for (mode, value) in [
            (CopilotMode::Interactive, "interactive"),
            (CopilotMode::Plan, "plan"),
            (CopilotMode::Autopilot, "autopilot"),
        ] {
            s.harness.agents.copilot.mode = Some(mode);
            assert_eq!(option_args("copilot", &s, None), ["--mode", value]);
        }
        s.harness.agents.copilot.mode = None;
        s.harness.agents.copilot.tool_permissions = Some(CopilotToolPermissions::AllowAllTools);
        assert_eq!(option_args("copilot", &s, None), ["--allow-all-tools"]);
        s.harness.agents.copilot.tool_permissions = Some(CopilotToolPermissions::AllowAll);
        s.harness.agents.copilot.extra_args = vec!["--no-ask-user".into()];
        assert_eq!(
            option_args("copilot", &s, None),
            ["--allow-all", "--no-ask-user"]
        );
    }

    #[test]
    fn codex_approvals() {
        let mut s = AppSettings::default();
        s.harness.agents.codex.approvals = Some(CodexApprovals::Auto);
        assert_eq!(
            option_args("codex", &s, None),
            [
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "on-request"
            ]
        );
        s.harness.agents.codex.approvals = Some(CodexApprovals::Bypass);
        assert_eq!(
            option_args("codex", &s, None),
            ["--dangerously-bypass-approvals-and-sandbox"]
        );
    }

    #[test]
    fn an_agent_gets_only_its_own_options() {
        let mut s = AppSettings::default();
        s.harness.agents.claude.skip_permissions = true;
        s.harness.agents.claude.extra_args = vec!["--verbose".into()];
        assert!(option_args("codex", &s, None).is_empty());
        assert!(option_args("copilot", &s, None).is_empty());
    }

    #[test]
    fn a_model_is_passed_by_each_clis_flag_after_its_options() {
        let mut s = AppSettings::default();
        for agent in ["claude", "codex", "copilot"] {
            assert_eq!(
                option_args(agent, &s, Some("m1")),
                ["--model", "m1"],
                "{agent}"
            );
        }
        s.harness.agents.claude.skip_permissions = true;
        s.harness.agents.claude.extra_args = vec!["--verbose".into()];
        assert_eq!(
            option_args("claude", &s, Some("opus")),
            [
                "--dangerously-skip-permissions",
                "--model",
                "opus",
                "--verbose"
            ]
        );
        // None resolved, or a blank one: no flag at all.
        assert_eq!(option_args("codex", &s, None), Vec::<String>::new());
        assert_eq!(option_args("codex", &s, Some(" ")), Vec::<String>::new());
        assert!(option_args("aider", &s, Some("opus")).is_empty());
    }

    #[test]
    fn a_model_already_in_extra_args_is_not_passed_twice() {
        let mut s = AppSettings::default();
        s.harness.agents.claude.extra_args = vec!["--model".into(), "haiku".into()];
        assert_eq!(
            option_args("claude", &s, Some("opus")),
            ["--model", "haiku"]
        );
        s.harness.agents.codex.extra_args = vec!["-m".into(), "o3".into()];
        assert_eq!(option_args("codex", &s, Some("gpt-5")), ["-m", "o3"]);
        s.harness.agents.copilot.extra_args = vec!["--model=gpt-5".into()];
        assert_eq!(option_args("copilot", &s, Some("x")), ["--model=gpt-5"]);
    }

    #[test]
    fn a_command_okena_does_not_know_gets_nothing() {
        let mut s = AppSettings::default();
        s.harness.agents.claude.skip_permissions = true;
        s.harness.agents.codex.extra_args = vec!["--x".into()];
        assert!(option_args("aider", &s, None).is_empty());
        assert!(option_args("my-claude-wrapper", &s, None).is_empty());
    }
}
