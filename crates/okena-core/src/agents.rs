//! Recognizing which terminal sessions are running a coding agent.
//!
//! A heuristic over the session's launch command and OSC title, so it is
//! labelled as such wherever it surfaces rather than presented as
//! authoritative. An agent that renames its own title, or one launched through
//! a wrapper script, will be missed; nothing here should be read as "these are
//! all the agents".
//!
//! Lives in `okena-core` because the daemon decides agent activity per terminal
//! and has to agree with every client about which terminals are agents.

use crate::shell::ShellType;

/// Commands that identify an AI coding agent okena knows.
///
/// Matched against the session's custom-shell command and its terminal title.
/// Deliberately a short, explicit list: a broad pattern would sweep in ordinary
/// shells and make the view untrustworthy.
pub const AGENT_COMMANDS: &[&str] = &["claude", "copilot", "codex"];

/// Shells a session can be left on. Launched as a session's command they are
/// a plain shell, not an agent okena does not know.
const PLAIN_SHELLS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "ksh",
    "tcsh",
    "csh",
    "nu",
    "pwsh",
    "powershell",
    "cmd",
    "wsl",
];

/// The lowercase command name of `path`, without directory or `.exe`.
pub fn command_name(path: &str) -> String {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let base = base.to_ascii_lowercase();
    match base.strip_suffix(".exe") {
        Some(stem) => stem.to_string(),
        None => base,
    }
}

/// Identify the agent a session is running, if any.
///
/// Returns the matched command name so the UI can label the row ("claude")
/// rather than asserting a vendor.
pub fn detect_agent(shell: &ShellType, title: Option<&str>) -> Option<String> {
    // A custom shell records the command okena launched, which is the strongest
    // signal available — it is what okena itself ran, not what the process
    // later claimed via an escape sequence.
    if let ShellType::Custom { path, .. } = shell {
        let base = command_name(path);
        if let Some(cmd) = AGENT_COMMANDS.iter().find(|c| base == **c) {
            return Some((*cmd).to_string());
        }
    }
    // Fall back to the OSC title, which most agents set. Weaker: a shell
    // sitting in a directory named "claude" would match, so require the title
    // to start with the command.
    let title = title?.trim().to_ascii_lowercase();
    AGENT_COMMANDS
        .iter()
        .find(|c| title == **c || title.starts_with(&format!("{c} ")))
        .map(|c| (*c).to_string())
}

/// Identify the agent a session's terminal runs, including one okena does not
/// know.
///
/// `session` says the terminal belongs to an agent session, where the command
/// okena launched is the agent whatever it is called — the `agent_command` a
/// user configured. A plain shell there is still a shell.
pub fn detect_session_agent(
    shell: &ShellType,
    title: Option<&str>,
    session: bool,
) -> Option<String> {
    if let Some(agent) = detect_agent(shell, title) {
        return Some(agent);
    }
    match shell {
        ShellType::Custom { path, .. } if session => {
            let base = command_name(path);
            (!base.is_empty() && !PLAIN_SHELLS.contains(&base.as_str())).then_some(base)
        }
        _ => None,
    }
}

/// How to name an agent to a reader: "Claude" for `claude`, and an agent
/// okena does not know by its command, capitalised.
pub fn display_name(command: &str) -> String {
    match command {
        "claude" => "Claude".to_string(),
        "codex" => "Codex".to_string(),
        "copilot" => "Copilot".to_string(),
        other => {
            let mut chars = other.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AGENT_COMMANDS, command_name, detect_agent, detect_session_agent, display_name};
    use crate::shell::ShellType;

    fn custom(path: &str) -> ShellType {
        ShellType::Custom {
            path: path.to_string(),
            args: Vec::new(),
        }
    }

    #[test]
    fn detects_agent_from_launch_command() {
        assert_eq!(
            detect_agent(&custom("claude"), None).as_deref(),
            Some("claude")
        );
        // An absolute path still resolves to its basename.
        assert_eq!(
            detect_agent(&custom("/opt/homebrew/bin/copilot"), None).as_deref(),
            Some("copilot")
        );
        assert_eq!(
            detect_agent(&custom(r"C:\tools\codex.exe"), None).as_deref(),
            Some("codex")
        );
    }

    #[test]
    fn plain_shell_is_not_an_agent() {
        assert_eq!(detect_agent(&ShellType::Default, None), None);
        assert_eq!(detect_agent(&custom("/bin/zsh"), None), None);
    }

    #[test]
    fn detects_agent_from_title() {
        assert_eq!(
            detect_agent(&ShellType::Default, Some("claude")).as_deref(),
            Some("claude")
        );
        assert_eq!(
            detect_agent(&ShellType::Default, Some("Copilot working…")).as_deref(),
            Some("copilot")
        );
    }

    #[test]
    fn title_must_start_with_the_command() {
        // A shell sitting in a directory named after an agent must not match —
        // that would fill the view with things that aren't agents.
        assert_eq!(
            detect_agent(&ShellType::Default, Some("~/src/claude")),
            None
        );
        assert_eq!(
            detect_agent(&ShellType::Default, Some("vim claude.rs")),
            None
        );
    }

    #[test]
    fn command_match_is_exact_not_substring() {
        // `claudius` is not `claude`.
        assert_eq!(detect_agent(&custom("claudius"), None), None);
    }

    #[test]
    fn agent_command_list_is_lowercase() {
        // Detection lowercases its input, so an uppercase entry could never match.
        for c in AGENT_COMMANDS {
            assert_eq!(*c, &c.to_ascii_lowercase(), "{c} must be lowercase");
        }
    }

    #[test]
    fn a_sessions_unknown_command_is_its_agent() {
        assert_eq!(
            detect_session_agent(&custom("/usr/local/bin/aider"), None, true).as_deref(),
            Some("aider")
        );
        // Outside a session an unknown command is just a program.
        assert_eq!(detect_session_agent(&custom("aider"), None, false), None);
    }

    #[test]
    fn a_session_left_on_a_shell_has_no_agent() {
        for shell in ["/bin/zsh", "bash", r"C:\Windows\System32\cmd.exe", "pwsh"] {
            assert_eq!(
                detect_session_agent(&custom(shell), None, true),
                None,
                "{shell}"
            );
        }
        assert_eq!(detect_session_agent(&ShellType::Default, None, true), None);
    }

    #[test]
    fn command_names_drop_directory_and_extension() {
        assert_eq!(command_name("/opt/bin/Claude"), "claude");
        assert_eq!(command_name(r"C:\x\codex.EXE"), "codex");
    }

    #[test]
    fn agents_are_named_for_a_reader() {
        assert_eq!(display_name("claude"), "Claude");
        assert_eq!(display_name("codex"), "Codex");
        assert_eq!(display_name("aider"), "Aider");
        assert_eq!(display_name(""), "");
    }
}
