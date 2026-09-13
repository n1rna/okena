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
    let stdin = (signal == "needs-input").then(read_stdin).flatten();
    let Some(event) = interpret(signal, stdin.as_deref(), payload) else {
        return 0;
    };
    let Ok(terminal_id) = std::env::var("OKENA_TERMINAL_ID") else {
        return 0;
    };
    if let Err(e) = post(&terminal_id, event) {
        eprintln!("okena agent-event: {e}");
    }
    0
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

fn post(terminal_id: &str, event: AgentHookEvent) -> Result<(), String> {
    let token = super::ensure_token()?;
    let body = serde_json::json!({
        "action": "agent_hook_event",
        "terminal_id": terminal_id,
        "event": event,
    });
    super::api_action(&token, &body.to_string()).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::interpret;
    use okena_core::agent_activity::AgentHookEvent;

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
