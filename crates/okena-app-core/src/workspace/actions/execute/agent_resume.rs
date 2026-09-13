//! Recovering an agent's conversation after its terminal restarts.
//!
//! Restarting an agent used to start a new conversation: the terminal came
//! back with the original brief, and everything the agent had read, decided
//! and been told was gone. Agent CLIs can resume a conversation, so restart
//! resumes it.
//!
//! The exact conversation, where the agent lets okena name it: okena picks a
//! Claude session id at launch (`--session-id`) and restart asks for that one
//! (`--resume <id>`). No extra state is kept — the id is read back out of the
//! launch command already stored as the session's shell. Where okena did not
//! name it (a session from before this, or an agent that only resumes "the
//! latest"), restart continues the most recent conversation in the session's
//! directory instead, which is the right one whenever that directory belongs
//! to one agent.

use okena_terminal::shell_config::ShellType;
use std::path::Path;

/// The program name of an agent command, lowercased: `/usr/bin/claude` →
/// `claude`.
fn program(command: &str) -> String {
    Path::new(command)
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// Arguments that name a new conversation, for agents that let okena name it.
///
/// Only Claude takes an id at launch. Anything else gets nothing, and restart
/// falls back to continuing its latest conversation.
pub fn session_args(command: &str) -> Vec<String> {
    match program(command).as_str() {
        "claude" => vec!["--session-id".to_string(), uuid::Uuid::new_v4().to_string()],
        _ => Vec::new(),
    }
}

/// The conversation id a launch named, if it named one.
pub fn session_id_of(args: &[String]) -> Option<&str> {
    args.windows(2)
        .find(|w| w[0] == "--session-id")
        .map(|w| w[1].as_str())
}

/// How to resume the agent `command` was launched as, given its launch
/// `args`. `None` for a program okena does not know how to resume.
///
/// The resume command carries no prompt: the conversation already has one,
/// and repeating the brief into a resumed session would restart the task
/// inside it.
pub fn resume_args(command: &str, args: &[String]) -> Option<Vec<String>> {
    match program(command).as_str() {
        "claude" => Some(match session_id_of(args) {
            Some(id) => vec!["--resume".to_string(), id.to_string()],
            None => vec!["--continue".to_string()],
        }),
        // Copilot names its own sessions, so okena cannot pick one at launch.
        "copilot" => Some(vec!["--continue".to_string()]),
        _ => None,
    }
}

/// Whether a session launched with `shell` can be resumed, when
/// `shares_dir` says another agent session runs in the same directory.
///
/// A named conversation is always safe to resume. An unnamed one is resumed by
/// "the latest conversation in this directory", which is only this session's
/// when the directory is only this session's — sessions rooted in a shared
/// projects folder would resume whichever of them spoke last.
pub fn resumable(shell: &ShellType, shares_dir: bool) -> bool {
    match shell {
        ShellType::Custom { path, args } => match resume_args(path, args) {
            Some(_) => session_id_of(args).is_some() || !shares_dir,
            None => false,
        },
        _ => false,
    }
}

/// The shell that resumes a session launched as `shell`, with okena's MCP
/// config handed over again. `None` when it cannot be resumed.
pub fn resume_shell(
    shell: &ShellType,
    settings: &crate::workspace::persistence::AppSettings,
) -> Option<ShellType> {
    let ShellType::Custom { path, args } = shell else {
        return None;
    };
    let mut resumed = resume_args(path, args)?;
    resumed.extend(super::agent_mcp::injection_args(path, settings));
    Some(ShellType::Custom {
        path: path.clone(),
        args: resumed,
    })
}

#[cfg(test)]
mod tests {
    use super::{resume_args, session_args, session_id_of};

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn claude_is_given_a_session_id_it_can_be_resumed_by() {
        let args = session_args("/usr/local/bin/claude");
        assert_eq!(args[0], "--session-id");
        assert!(uuid::Uuid::parse_str(&args[1]).is_ok(), "{args:?}");
        assert!(session_args("copilot").is_empty());
    }

    #[test]
    fn a_named_conversation_is_resumed_by_its_id() {
        let launched = strings(&["--session-id", "abc", "Work on QBL-1", "--mcp-config", "x"]);
        assert_eq!(
            resume_args("claude", &launched),
            Some(strings(&["--resume", "abc"]))
        );
    }

    #[test]
    fn resuming_drops_the_brief() {
        // Repeating the brief into a resumed conversation would restart the
        // task inside it.
        let launched = strings(&["--session-id", "abc", "Work on QBL-1"]);
        let resumed = resume_args("claude", &launched).expect("resumable");
        assert!(!resumed.iter().any(|a| a.contains("QBL-1")), "{resumed:?}");
    }

    #[test]
    fn a_session_from_before_ids_continues_its_latest_conversation() {
        assert_eq!(
            resume_args("claude", &strings(&["Work on QBL-1"])),
            Some(strings(&["--continue"]))
        );
    }

    #[test]
    fn copilot_continues_and_unknown_agents_are_not_resumable() {
        assert_eq!(resume_args("copilot", &[]), Some(strings(&["--continue"])));
        assert_eq!(resume_args("aider", &[]), None);
    }

    #[test]
    fn an_unnamed_conversation_in_a_shared_directory_is_not_resumable() {
        // "Continue the latest conversation here" would pick whichever agent
        // in that folder spoke last.
        use okena_terminal::shell_config::ShellType;
        let unnamed = ShellType::Custom {
            path: "claude".into(),
            args: strings(&["Break QBL-1 down"]),
        };
        let named = ShellType::Custom {
            path: "claude".into(),
            args: strings(&["--session-id", "abc", "Break QBL-1 down"]),
        };
        assert!(!super::resumable(&unnamed, true));
        assert!(super::resumable(&unnamed, false));
        assert!(
            super::resumable(&named, true),
            "a named conversation is always exact"
        );
        assert!(!super::resumable(&ShellType::Default, false));
    }

    #[test]
    fn the_id_is_read_from_wherever_it_sits_in_the_args() {
        assert_eq!(
            session_id_of(&strings(&["p", "--session-id", "x1"])),
            Some("x1")
        );
        assert_eq!(session_id_of(&strings(&["--session-id"])), None);
    }
}
