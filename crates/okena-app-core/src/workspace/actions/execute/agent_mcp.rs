//! Wiring okena into the agents okena launches.
//!
//! The point is that the user never configures this. When okena starts an
//! agent, it also hands that agent:
//!
//! * okena's MCP server (`okena mcp`), so the agent can call `okena_whoami`,
//!   report status and register assets;
//! * hooks that tell okena when a turn starts, when it ends and when the agent
//!   is blocked on you (`okena agent-event`), so its state shows whether or not
//!   it ever reports.
//!
//! Everything is handed over per launch — on the command line, or in files
//! written **into okena's profile directory**. Never into the worktree: writing
//! `.mcp.json` into a checkout would either clobber a repository's own
//! checked-in config or leave an untracked file dirtying every `git status`.
//! And never into the agent's own settings: editing `~/.claude/settings.json`
//! or Codex's config would change every agent the user runs, okena's or not.

use okena_workspace::settings::AppSettings;
use serde_json::json;
use std::path::{Path, PathBuf};

/// Placeholder replaced with the generated config's path.
const CONFIG_PLACEHOLDER: &str = "{config}";

/// Placeholder replaced with this okena binary's path, as a quoted TOML string.
const EXE_PLACEHOLDER: &str = "{exe}";

/// How long an agent waits on one of okena's hooks before giving up on it.
const HOOK_TIMEOUT_SECS: u64 = 10;

/// Default flags per agent for pointing it at okena's MCP server.
///
/// Only agents whose flag is known are listed; anything else gets no injection
/// rather than a guessed flag that would make the agent fail to start.
/// `settings.harness.agent_mcp_args` overrides this per deployment, so a
/// changed CLI can be fixed in settings instead of waiting for a release.
fn default_mcp_args(agent: &str) -> Option<Vec<String>> {
    match agent {
        // Verified against `claude --help`: takes JSON files or strings.
        "claude" => Some(vec!["--mcp-config".into(), CONFIG_PLACEHOLDER.into()]),
        // Verified against `copilot --help`: takes a JSON string, or a file
        // path when prefixed with `@`. It augments ~/.copilot/mcp-config.json
        // rather than replacing it, so the user's own servers survive.
        "copilot" => Some(vec![
            "--additional-mcp-config".into(),
            format!("@{CONFIG_PLACEHOLDER}"),
        ]),
        // Codex has no flag for an MCP config file. `-c key=value` overrides
        // one key of ~/.codex/config.toml for this run only, as TOML, so the
        // user's own servers survive here too.
        "codex" => Some(vec![
            "-c".into(),
            format!("mcp_servers.okena.command={EXE_PLACEHOLDER}"),
            "-c".into(),
            r#"mcp_servers.okena.args=["mcp"]"#.into(),
        ]),
        _ => None,
    }
}

/// Write `body` into the active profile's directory as `name`.
///
/// Rewritten on every launch rather than cached: the binary path inside
/// changes across upgrades and profile switches, and a stale path yields an
/// agent whose tools or hooks silently fail.
fn write_profile_file(name: &str, body: &serde_json::Value) -> Option<PathBuf> {
    let path = okena_core::profiles::try_current().map(|p| p.root.join(name))?;
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return None;
    }
    match std::fs::write(&path, serde_json::to_string_pretty(body).ok()?) {
        Ok(()) => Some(path),
        Err(e) => {
            log::warn!("[tasks] could not write {name}: {e}");
            None
        }
    }
}

/// `s` as a TOML basic string. JSON's string escapes are ones TOML accepts.
fn toml_string(s: &str) -> String {
    serde_json::Value::String(s.to_string()).to_string()
}

/// `s` quoted as one word for the shell an agent runs its hooks through.
fn shell_quote(s: &str) -> String {
    if cfg!(windows) {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Arguments that point `agent` at okena's MCP server.
fn mcp_args(agent: &str, exe: &Path, settings: &AppSettings) -> Vec<String> {
    let template = settings
        .harness
        .agent_mcp_args
        .clone()
        .filter(|t| !t.is_empty())
        .or_else(|| default_mcp_args(agent));
    let Some(template) = template else {
        return Vec::new();
    };
    let config = if template.iter().any(|a| a.contains(CONFIG_PLACEHOLDER)) {
        let body = json!({
            "mcpServers": {
                "okena": { "command": exe.to_string_lossy(), "args": ["mcp"] }
            }
        });
        match write_profile_file("agent-mcp.json", &body) {
            Some(path) => path.to_string_lossy().into_owned(),
            None => return Vec::new(),
        }
    } else {
        String::new()
    };
    let exe = toml_string(&exe.to_string_lossy());
    template
        .into_iter()
        .map(|arg| {
            arg.replace(CONFIG_PLACEHOLDER, &config)
                .replace(EXE_PLACEHOLDER, &exe)
        })
        .collect()
}

/// The Claude Code settings that report its lifecycle to okena.
///
/// `UserPromptSubmit` starts a turn and `Stop` ends one. Tool hooks keep a long
/// turn marked working, and `PostToolUse` in particular brings it back from a
/// permission prompt once you answer. `Notification` is how Claude Code says
/// it is blocked on you.
fn claude_hook_settings(exe: &str) -> serde_json::Value {
    let command = |signal: &str| {
        json!({
            "type": "command",
            "command": format!("{} agent-event {signal}", shell_quote(exe)),
            "timeout": HOOK_TIMEOUT_SECS,
        })
    };
    let on = |signal: &str| json!([{ "hooks": [command(signal)] }]);
    let on_every_tool = |signal: &str| json!([{ "matcher": "*", "hooks": [command(signal)] }]);
    json!({
        "hooks": {
            "UserPromptSubmit": on("turn-started"),
            "PreToolUse": on_every_tool("tool"),
            "PostToolUse": on_every_tool("tool"),
            "Notification": on("needs-input"),
            "Stop": on("turn-ended"),
        }
    })
}

/// Arguments that make `agent` tell okena what it is doing.
///
/// Copilot CLI has no hook or notify mechanism (checked against `copilot
/// --help`, 1.0.34), so like any other agent it is read from its terminal.
fn status_args(agent: &str, exe: &Path) -> Vec<String> {
    let exe = exe.to_string_lossy();
    match agent {
        // Verified against `claude --help`: `--settings <file-or-json>` loads
        // additional settings on top of the user's own, whose hooks still run.
        "claude" => {
            match write_profile_file("agent-hooks-claude.json", &claude_hook_settings(&exe)) {
                Some(path) => vec!["--settings".into(), path.to_string_lossy().into_owned()],
                None => Vec::new(),
            }
        }
        // Codex runs `notify` with its event as JSON in the last argument.
        "codex" => vec!["-c".into(), codex_notify(&exe)],
        _ => Vec::new(),
    }
}

/// The `-c` value that points Codex's `notify` at okena.
fn codex_notify(exe: &str) -> String {
    format!(
        "notify=[{},{},{}]",
        toml_string(exe),
        toml_string("agent-event"),
        toml_string("codex-notify")
    )
}

/// Extra arguments that wire okena into `agent`: its MCP server and its status
/// hooks.
///
/// Empty when injection is disabled, the agent is unknown, or a file could not
/// be written — in every one of those cases the agent still launches, just
/// without okena's tools, and its state is read from its terminal instead.
pub(super) fn injection_args(agent: &str, settings: &AppSettings) -> Vec<String> {
    if !settings.harness.agent_mcp_injection {
        return Vec::new();
    }
    let Ok(exe) = std::env::current_exe() else {
        return Vec::new();
    };
    // A launch command can be a path; the flags belong to the program.
    let agent = okena_core::agents::command_name(agent);
    let mut args = mcp_args(&agent, &exe, settings);
    args.extend(status_args(&agent, &exe));
    args
}

/// Whether `args` show okena's MCP server was injected.
///
/// Used by the Agents view to report whether a running session actually has
/// okena's tools, rather than assuming every agent it launched does.
pub fn args_have_mcp(args: &[String]) -> bool {
    args.iter()
        .any(|a| a.ends_with("agent-mcp.json") || a.starts_with("mcp_servers.okena."))
}

#[cfg(test)]
mod tests {
    use super::{
        args_have_mcp, claude_hook_settings, codex_notify, default_mcp_args, shell_quote,
        toml_string,
    };
    use okena_workspace::settings::AppSettings;

    #[test]
    fn claude_has_a_known_flag() {
        let args = default_mcp_args("claude").expect("claude is supported");
        assert_eq!(args[0], "--mcp-config");
        assert_eq!(args[1], "{config}");
    }

    #[test]
    fn copilot_takes_a_file_path_prefixed_with_at() {
        let args = default_mcp_args("copilot").expect("copilot is supported");
        assert_eq!(args[0], "--additional-mcp-config");
        assert_eq!(args[1], "@{config}");
    }

    #[test]
    fn codex_gets_its_server_through_config_overrides() {
        let args = default_mcp_args("codex").expect("codex is supported");
        assert_eq!(
            args,
            [
                "-c",
                "mcp_servers.okena.command={exe}",
                "-c",
                r#"mcp_servers.okena.args=["mcp"]"#
            ]
        );
    }

    #[test]
    fn an_unknown_agent_gets_no_injection() {
        // Guessing a flag would make the agent fail to start, which is worse
        // than launching it without okena's tools.
        assert!(default_mcp_args("some-new-agent").is_none());
        assert!(default_mcp_args("").is_none());
    }

    #[test]
    fn injection_is_on_by_default() {
        assert!(AppSettings::default().harness.agent_mcp_injection);
    }

    #[test]
    fn disabling_injection_yields_no_args() {
        let mut s = AppSettings::default();
        s.harness.agent_mcp_injection = false;
        assert!(super::injection_args("claude", &s).is_empty());
        assert!(super::injection_args("codex", &s).is_empty());
    }

    #[test]
    fn detects_an_injected_config_in_args() {
        assert!(args_have_mcp(&[
            "--mcp-config".into(),
            "/x/profiles/dev/agent-mcp.json".into()
        ]));
        // copilot's `@`-prefixed form must be recognized too, or its sessions
        // would wrongly report "no okena mcp".
        assert!(args_have_mcp(&[
            "--additional-mcp-config".into(),
            "@/x/profiles/dev/agent-mcp.json".into()
        ]));
        assert!(args_have_mcp(&[
            "-c".into(),
            r#"mcp_servers.okena.command="/x/okena""#.into()
        ]));
    }

    #[test]
    fn plain_args_are_not_mistaken_for_injection() {
        assert!(!args_have_mcp(&["Work on LIN-1".into()]));
        assert!(!args_have_mcp(&[]));
    }

    #[test]
    fn claude_hooks_cover_the_whole_turn() {
        let settings = claude_hook_settings("/opt/okena");
        let hooks = settings["hooks"].as_object().expect("hooks object");
        for (event, signal) in [
            ("UserPromptSubmit", "turn-started"),
            ("PreToolUse", "tool"),
            ("PostToolUse", "tool"),
            ("Notification", "needs-input"),
            ("Stop", "turn-ended"),
        ] {
            let command = hooks[event][0]["hooks"][0]["command"]
                .as_str()
                .expect("command");
            assert!(
                command.ends_with(&format!(" agent-event {signal}")),
                "{event}: {command}"
            );
            assert_eq!(hooks[event][0]["hooks"][0]["type"], "command");
        }
        assert_eq!(hooks["PreToolUse"][0]["matcher"], "*");
    }

    #[cfg(not(windows))]
    #[test]
    fn a_hook_command_survives_a_path_with_spaces_and_quotes() {
        assert_eq!(
            shell_quote("/Applications/My Okena.app/okena"),
            "'/Applications/My Okena.app/okena'"
        );
        assert_eq!(shell_quote("/x/it's/okena"), r"'/x/it'\''s/okena'");
    }

    #[test]
    fn codex_notify_is_a_toml_array() {
        assert_eq!(
            codex_notify("/opt/okena"),
            r#"notify=["/opt/okena","agent-event","codex-notify"]"#
        );
        // A Windows path's backslashes have to be escaped in TOML.
        assert_eq!(toml_string(r"C:\okena.exe"), r#""C:\\okena.exe""#);
    }
}
