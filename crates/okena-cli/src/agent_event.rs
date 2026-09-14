//! `okena agent-event` — how an agent's own hooks tell okena what it is doing.
//!
//! okena injects these into the agents it launches (Claude Code hooks through
//! `--settings`, Codex's `notify`), so they run inside the agent's terminal and
//! read its identity from `$OKENA_TERMINAL_ID`, like `okena mcp` does.
//!
//! A hook runs in the agent's path, so this never fails it: it always exits 0
//! and never writes to stdout, which Claude Code would add to the conversation
//! for a prompt-submit hook.

use okena_core::agent_activity::AgentHookEvent;
use serde_json::Value;
use std::io::{IsTerminal, Read};

/// Run the subcommand. Always returns 0; failures go to stderr.
pub fn run(signal: &str, payload: Option<&str>) -> i32 {
    let stdin = matches!(signal, "needs-input" | "tool")
        .then(read_stdin)
        .flatten();
    let Some(event) = interpret(signal, stdin.as_deref(), payload) else {
        return 0;
    };
    let Ok(terminal_id) = std::env::var("OKENA_TERMINAL_ID") else {
        return 0;
    };
    let pushed_from = (signal == "tool")
        .then(|| stdin.as_deref().and_then(pushed_from))
        .flatten();
    if let Err(e) = post(&terminal_id, event, pushed_from) {
        eprintln!("okena agent-event: {e}");
    }
    0
}

/// The directory a push just ran in, from Claude Code's `PostToolUse` hook
/// input: a Bash command that runs `git push` or `gh pr create`. `None` for
/// anything else, including the same command before it ran (`PreToolUse`).
fn pushed_from(stdin: &str) -> Option<String> {
    let hook: Value = serde_json::from_str(stdin).ok()?;
    if hook.get("hook_event_name").and_then(Value::as_str) != Some("PostToolUse")
        || hook.get("tool_name").and_then(Value::as_str) != Some("Bash")
    {
        return None;
    }
    let command = hook.pointer("/tool_input/command").and_then(Value::as_str)?;
    let cwd = hook.get("cwd").and_then(Value::as_str)?;
    push_dir(command, cwd)
}

/// Where a shell command pushes from: the working directory of its first
/// `git push` or `gh pr create`, following `cd <dir>` steps before it and a
/// `git -C <dir>`. `None` when it does neither.
///
/// A reading of plain command lines, not a shell: quoting is honoured for
/// paths, but subshells, variables and aliases are not expanded. A push it
/// cannot place is still found by the PR poll's own cadence.
fn push_dir(command: &str, cwd: &str) -> Option<String> {
    let mut dir = std::path::PathBuf::from(cwd);
    for segment in split_segments(command) {
        let words = shell_words(&segment);
        let mut words = words.iter().map(String::as_str).peekable();
        // Leading environment assignments: `GIT_TRACE=1 git push`.
        while words.peek().is_some_and(|w| is_assignment(w)) {
            words.next();
        }
        match words.next() {
            Some("cd") => {
                let target = words.next().unwrap_or("~");
                dir = resolve(&dir, target);
            }
            Some("git") => {
                let mut run_in = dir.clone();
                let mut rest = words.collect::<Vec<_>>().into_iter();
                let mut subcommand = None;
                while let Some(word) = rest.next() {
                    match word {
                        "-C" => {
                            if let Some(target) = rest.next() {
                                run_in = resolve(&run_in, target);
                            }
                        }
                        // Global options that take a value.
                        "-c" | "--git-dir" | "--work-tree" | "--namespace" => {
                            rest.next();
                        }
                        w if w.starts_with('-') => {}
                        w => {
                            subcommand = Some(w);
                            break;
                        }
                    }
                }
                if subcommand == Some("push") {
                    return Some(run_in.to_string_lossy().into_owned());
                }
            }
            Some("gh") => {
                let rest: Vec<&str> = words.filter(|w| !w.starts_with('-')).collect();
                if rest.starts_with(&["pr", "create"]) {
                    return Some(dir.to_string_lossy().into_owned());
                }
            }
            _ => {}
        }
    }
    None
}

/// Split a command line into the commands it chains: `&&`, `||`, `;`, `|`
/// and newlines, outside quotes.
fn split_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => {
                quote = None;
                current.push(c);
            }
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                current.push(c);
            }
            (None, '&' | '|') => {
                if chars.peek() == Some(&c) {
                    chars.next();
                }
                segments.push(std::mem::take(&mut current));
            }
            (None, ';' | '\n') => segments.push(std::mem::take(&mut current)),
            (None, c) => current.push(c),
        }
    }
    segments.push(current);
    segments
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// A command's words, with single and double quotes removed.
fn shell_words(segment: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut in_word = false;
    for c in segment.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            (None, c) => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(current);
    }
    words
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// `target` taken from `base`, the way `cd` takes it: absolute stays, `~`
/// is home, anything else is relative to `base`.
fn resolve(base: &std::path::Path, target: &str) -> std::path::PathBuf {
    if let Some(rest) = target.strip_prefix('~')
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest.trim_start_matches('/'));
    }
    let path = std::path::Path::new(target);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

/// The JSON a Claude Code hook receives on stdin. Not read from a terminal,
/// where it would wait for a person to type.
fn read_stdin() -> Option<String> {
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        return None;
    }
    let mut buf = String::new();
    stdin.read_to_string(&mut buf).ok()?;
    Some(buf)
}

/// What a hook invocation means, or `None` for one that says nothing about
/// whether the agent needs you.
///
/// * `turn-started`, `tool`, `turn-ended` — Claude Code's `UserPromptSubmit`,
///   `PreToolUse`/`PostToolUse` and `Stop`.
/// * `needs-input` — Claude Code's `Notification`, whose stdin says why. Its
///   idle reminder ("waiting for your input") comes after a turn has already
///   ended and is not a new reason to stop.
/// * `codex-notify` — Codex's `notify`, which passes its event as JSON in the
///   last argument.
fn interpret(signal: &str, stdin: Option<&str>, payload: Option<&str>) -> Option<AgentHookEvent> {
    match signal {
        "turn-started" => Some(AgentHookEvent::TurnStarted),
        "tool" => Some(AgentHookEvent::ToolActivity),
        "turn-ended" => Some(AgentHookEvent::TurnEnded),
        "needs-input" => {
            let notification = stdin.and_then(|s| serde_json::from_str::<Value>(s).ok());
            let is_idle_reminder = notification.as_ref().is_some_and(|n| {
                n.get("notification_type").and_then(Value::as_str) == Some("idle_prompt")
                    || n.get("message")
                        .and_then(Value::as_str)
                        .is_some_and(|m| m.to_ascii_lowercase().contains("waiting for your input"))
            });
            (!is_idle_reminder).then_some(AgentHookEvent::NeedsInput)
        }
        "codex-notify" => {
            let kind = payload
                .and_then(|p| serde_json::from_str::<Value>(p).ok())
                .and_then(|v| v.get("type").and_then(Value::as_str).map(str::to_string))?;
            if kind == "agent-turn-complete" {
                Some(AgentHookEvent::TurnEnded)
            } else if kind.contains("approval") {
                Some(AgentHookEvent::NeedsInput)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn post(
    terminal_id: &str,
    event: AgentHookEvent,
    pushed_from: Option<String>,
) -> Result<(), String> {
    let token = super::ensure_token()?;
    let mut body = serde_json::json!({
        "action": "agent_hook_event",
        "terminal_id": terminal_id,
        "event": event,
    });
    if let Some(dir) = pushed_from {
        body["pushed_from"] = Value::String(dir);
    }
    super::api_action(&token, &body.to_string()).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{interpret, push_dir, pushed_from};
    use okena_core::agent_activity::AgentHookEvent;

    #[test]
    fn a_push_or_pr_create_is_placed_where_it_ran() {
        let cwd = "/work/session";
        for (command, expected) in [
            ("git push -u origin feat/x", Some("/work/session")),
            ("gh pr create --fill", Some("/work/session")),
            ("gh pr create -R n1rna/okena --title 'x'", Some("/work/session")),
            (
                "cd /work/okena-wt/feat-x && git push",
                Some("/work/okena-wt/feat-x"),
            ),
            ("cd repo && git add -A && git commit -m 'x; y' && git push", Some("/work/session/repo")),
            ("git -C ../other push origin HEAD", Some("/work/session/../other")),
            ("GIT_TRACE=1 git -c core.x=1 push", Some("/work/session")),
            ("cd \"/work/with space\"; gh pr create", Some("/work/with space")),
        ] {
            assert_eq!(push_dir(command, cwd).as_deref(), expected, "{command}");
        }
    }

    #[test]
    fn anything_but_a_push_is_not_a_push() {
        for command in [
            "git status",
            "git log --grep push",
            "echo 'git push'",
            "gh pr view 12",
            "gh pr list",
            "cargo test",
        ] {
            assert_eq!(push_dir(command, "/w"), None, "{command}");
        }
    }

    #[test]
    fn only_a_bash_push_that_has_run_counts() {
        let hook = |event: &str, tool: &str, command: &str| {
            serde_json::json!({
                "hook_event_name": event, "tool_name": tool, "cwd": "/w",
                "tool_input": { "command": command },
            })
            .to_string()
        };
        assert_eq!(
            pushed_from(&hook("PostToolUse", "Bash", "git push")).as_deref(),
            Some("/w")
        );
        assert_eq!(
            pushed_from(&hook("PreToolUse", "Bash", "git push")),
            None,
            "not yet run"
        );
        assert_eq!(pushed_from(&hook("PostToolUse", "Edit", "git push")), None);
        assert_eq!(pushed_from("not json"), None);
    }

    #[test]
    fn claude_lifecycle_hooks_map_to_events() {
        assert_eq!(
            interpret("turn-started", None, None),
            Some(AgentHookEvent::TurnStarted)
        );
        assert_eq!(
            interpret("tool", None, None),
            Some(AgentHookEvent::ToolActivity)
        );
        assert_eq!(
            interpret("turn-ended", None, None),
            Some(AgentHookEvent::TurnEnded)
        );
    }

    #[test]
    fn a_permission_notification_needs_input() {
        let stdin = r#"{"hook_event_name":"Notification","message":"Claude needs your permission to use Bash","notification_type":"permission_prompt"}"#;
        assert_eq!(
            interpret("needs-input", Some(stdin), None),
            Some(AgentHookEvent::NeedsInput)
        );
        // No stdin to say why: a notification is still a reason to look.
        assert_eq!(
            interpret("needs-input", None, None),
            Some(AgentHookEvent::NeedsInput)
        );
    }

    #[test]
    fn the_idle_reminder_is_not_a_new_reason_to_stop() {
        let typed =
            r#"{"message":"Claude is waiting for your input","notification_type":"idle_prompt"}"#;
        assert_eq!(interpret("needs-input", Some(typed), None), None);
        let untyped = r#"{"message":"Claude is waiting for your input"}"#;
        assert_eq!(interpret("needs-input", Some(untyped), None), None);
    }

    #[test]
    fn codex_turn_complete_ends_the_turn() {
        let payload =
            r#"{"type":"agent-turn-complete","turn-id":"1","last-assistant-message":"Done."}"#;
        assert_eq!(
            interpret("codex-notify", None, Some(payload)),
            Some(AgentHookEvent::TurnEnded)
        );
        assert_eq!(
            interpret("codex-notify", None, Some(r#"{"type":"something-else"}"#)),
            None
        );
        assert_eq!(interpret("codex-notify", None, None), None);
    }

    #[test]
    fn an_unknown_signal_says_nothing() {
        assert_eq!(interpret("compacting", None, None), None);
    }
}
