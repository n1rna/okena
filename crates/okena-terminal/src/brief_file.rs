//! An agent's opening brief, handed over from a file.
//!
//! A brief used to be one argv element of the agent's command. Session
//! backends carry that command as a string — tmux's whole `new-session` is one
//! message of at most ~16 KB — so a long enough brief kept the agent from
//! starting at all (`command too long`). A launch now writes its brief to a
//! file and puts a [`reference`] to it where the brief went. The launch shell
//! stored on the project keeps its shape — `claude --session-id … <brief> …` —
//! so everything that reads it (agent detection, resume) is unchanged, and the
//! reference is resolved here, as the terminal is spawned, by a short POSIX
//! wrapper that reads the file and execs the agent with its contents in the
//! same argv position.

use crate::backend::{TerminalLaunchCommand, TerminalLaunchPlan};
use crate::shell_config::ShellType;
use std::path::Path;

/// What marks an argument as a reference to a brief file.
const PREFIX: &str = "@okena-brief-file:";

/// The argument that stands for the brief in `path`.
pub fn reference(path: &Path) -> String {
    format!("{PREFIX}{}", path.to_string_lossy())
}

/// The brief file `arg` refers to, if it is a reference.
pub fn path_of(arg: &str) -> Option<&str> {
    arg.strip_prefix(PREFIX).filter(|p| !p.is_empty())
}

/// Reads the file (`$2`) and execs the command that follows it, with the file's
/// contents inserted at argument index `$1` — byte for byte: the `.` sentinel
/// keeps the trailing newlines command substitution would strip.
#[cfg(not(windows))]
const WRAPPER: &str = r#"i=$1; f=$2; shift 2; b=$(cat -- "$f" && printf .) || exit 1; b=${b%.}; n=0; for a do [ "$n" = "$i" ] && set -- "$@" "$b"; set -- "$@" "$a"; shift; n=$((n+1)); done; [ "$n" = "$i" ] && set -- "$@" "$b"; exec "$@""#;

/// `program args…` with its brief reference resolved by the wrapper, or `None`
/// when no argument is a reference. The result does not grow with the brief.
#[cfg(not(windows))]
pub fn wrap(program: &str, args: &[String]) -> Option<(String, Vec<String>)> {
    let (at, file) = args
        .iter()
        .enumerate()
        .find_map(|(i, a)| path_of(a).map(|p| (i, p)))?;
    let mut wrapped = vec![
        "-c".to_string(),
        WRAPPER.to_string(),
        "okena-brief".to_string(),
        // Index into `program args…` without the reference.
        (at + 1).to_string(),
        file.to_string(),
        program.to_string(),
    ];
    wrapped.extend(
        args.iter()
            .enumerate()
            .filter(|(i, _)| *i != at)
            .map(|(_, a)| a.clone()),
    );
    Some(("/bin/sh".to_string(), wrapped))
}

/// Windows and WSL read the brief back into argv. The host has no POSIX `sh`
/// to wrap with; psmux takes the command as argv tokens rather than one shell
/// string; and a WSL terminal's brief path is a Windows path its `sh` would
/// have to translate. So those routes keep today's argv, and today's limits.
#[cfg(windows)]
fn inline(program: &str, args: &[String]) -> Option<(String, Vec<String>)> {
    let at = args.iter().position(|a| path_of(a).is_some())?;
    let mut args = args.to_vec();
    if let Some(file) = path_of(&args[at]) {
        match std::fs::read_to_string(file) {
            Ok(brief) => args[at] = brief,
            Err(e) => log::warn!("[terminal] could not read the brief {file}: {e}"),
        }
    }
    Some((program.to_string(), args))
}

fn resolve(program: &str, args: &[String]) -> Option<(String, Vec<String>)> {
    #[cfg(not(windows))]
    return wrap(program, args);
    #[cfg(windows)]
    return inline(program, args);
}

/// `plan` with any brief reference resolved, or `None` when it has none — the
/// ordinary case, which spawns exactly as before.
pub fn resolve_plan(plan: &TerminalLaunchPlan) -> Option<TerminalLaunchPlan> {
    let route = match &plan.route {
        ShellType::Custom { path, args } => {
            resolve(path, args).map(|(path, args)| ShellType::Custom { path, args })
        }
        _ => None,
    };
    let command = plan.initial_command.as_ref().and_then(|c| {
        resolve(&c.program, &c.args).map(|(program, args)| TerminalLaunchCommand { program, args })
    });
    if route.is_none() && command.is_none() {
        return None;
    }
    Some(TerminalLaunchPlan {
        route: route.unwrap_or_else(|| plan.route.clone()),
        initial_command: command.or_else(|| plan.initial_command.clone()),
        environment: plan.environment.clone(),
    })
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    use crate::session_backend::{ResolvedBackend, SessionCommand};

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_launch_without_a_reference_is_left_alone() {
        assert_eq!(wrap("claude", &strings(&["--session-id", "x", "hi"])), None);
        let plan = TerminalLaunchPlan::for_shell(ShellType::Custom {
            path: "claude".into(),
            args: strings(&["hi"]),
        });
        assert_eq!(resolve_plan(&plan), None);
        assert_eq!(
            resolve_plan(&TerminalLaunchPlan::for_shell(ShellType::Default)),
            None
        );
        // A prefix alone names no file.
        assert_eq!(path_of(PREFIX), None);
    }

    #[test]
    fn the_wrapper_keeps_every_other_argument_in_order() {
        let brief = reference(Path::new("/p/agent-briefs/1.md"));
        let (program, args) = wrap(
            "claude",
            &strings(&["--session-id", "abc", &brief, "--mcp-config", "m"]),
        )
        .expect("wrapped");
        assert_eq!(program, "/bin/sh");
        assert_eq!(args[0], "-c");
        assert_eq!(
            args[2..],
            strings(&[
                "okena-brief",
                "3",
                "/p/agent-briefs/1.md",
                "claude",
                "--session-id",
                "abc",
                "--mcp-config",
                "m"
            ])
        );
    }

    #[test]
    fn a_resolved_plan_routes_the_wrapper_as_a_program_not_a_script() {
        // Two args starting with `-c` would read as a shell script; the
        // wrapper always carries more.
        let plan = TerminalLaunchPlan::for_shell(ShellType::Custom {
            path: "claude".into(),
            args: vec![reference(Path::new("/b.md"))],
        });
        match resolve_plan(&plan).expect("resolved").route {
            ShellType::Custom { path, args } => {
                assert_eq!(path, "/bin/sh");
                assert!(args.len() > 2, "{args:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_session_command_does_not_grow_with_the_brief() {
        let brief = reference(Path::new(
            "/Users/someone/.config/okena/profiles/default/agent-briefs/0f8b7e1c-6a55-4f0e-9d7e-1c2b3a4d5e6f.md",
        ));
        let (program, args) = wrap(
            "claude",
            &strings(&[
                "--session-id",
                "0f8b7e1c-6a55-4f0e-9d7e-1c2b3a4d5e6f",
                &brief,
                "--mcp-config",
                "/x/agent-mcp.json",
            ]),
        )
        .expect("wrapped");
        for backend in [
            ResolvedBackend::Tmux,
            ResolvedBackend::Screen,
            ResolvedBackend::Dtach,
        ] {
            let (_, built) = backend
                .build_command_with_custom(
                    "tm-12345678",
                    "/Users/someone/p/okena",
                    Some(SessionCommand::Program {
                        program: &program,
                        args: &args,
                    }),
                    &[],
                )
                .expect("command");
            let size: usize = built.iter().map(String::len).sum();
            assert!(size < 2_000, "{backend:?}: {size} bytes");
        }
    }
}

/// The wrapper run for real: through each session backend on this machine, in
/// a PTY, handing a fake agent a brief it writes back out argument by argument.
#[cfg(all(test, unix))]
mod round_trip {
    use super::*;
    use crate::session_backend::{ResolvedBackend, SessionCommand, get_dtach_socket_path};
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    /// Quotes of both kinds, shell syntax, newlines, a trailing blank line and
    /// non-ASCII, repeated past the tmux limit.
    fn awkward_brief() -> String {
        let chunk =
            "Don't \"quote\" me: $HOME `id` $(id) \\ %s *?[x] ; & | > \n\tünïcødé — 日本語 🚀\n";
        let mut brief = chunk.repeat(20_000 / chunk.len() + 1);
        brief.push_str("\n\n");
        assert!(brief.len() > 20_000);
        brief
    }

    /// Whether this test runs inside tmux or screen — an okena terminal, most
    /// likely. The tmux and screen round trips skip there: a mistake in their
    /// isolation would reach the server every one of the user's terminals
    /// lives on.
    fn inside_a_multiplexer() -> bool {
        ["TMUX", "STY"]
            .iter()
            .any(|v| std::env::var_os(v).is_some_and(|s| !s.is_empty()))
    }

    fn installed(program: &str) -> bool {
        std::process::Command::new("sh")
            .args(["-c", &format!("command -v {program}")])
            .stdout(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Launch `agent --session-id abc <brief> --mcp-config "m b"` through
    /// `backend` and return the arguments the agent received.
    fn received(backend: Option<ResolvedBackend>, brief: &str) -> Vec<String> {
        // Short: tmux's socket path lives under it, and sun_path is ~104 bytes.
        let dir = Scratch(PathBuf::from(format!(
            "/tmp/okb-{}",
            &uuid::Uuid::new_v4().to_string()[..8]
        )));
        std::fs::create_dir_all(dir.0.join("screen")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.0.join("screen"), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let out = dir.0.join("argv");
        let agent = dir.0.join("agent");
        std::fs::write(
            &agent,
            format!(
                "#!/bin/sh\nfor a do printf '%s\\0' \"$a\"; done > {0}.tmp && mv {0}.tmp {0}\n",
                out.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).unwrap();
        let brief_path = dir.0.join("brief.md");
        std::fs::write(&brief_path, brief).unwrap();

        let agent_path = agent.to_string_lossy().into_owned();
        let (program, args) = wrap(
            &agent_path,
            &[
                "--session-id".into(),
                "abc".into(),
                reference(&brief_path),
                "--mcp-config".into(),
                "m b".into(),
            ],
        )
        .expect("wrapped");
        let session = format!("tm-okb{}", &uuid::Uuid::new_v4().to_string()[..6]);
        let cwd = dir.0.to_string_lossy().into_owned();
        let (program, args) = match &backend {
            Some(b) => b
                .build_command_with_custom(
                    &session,
                    &cwd,
                    Some(SessionCommand::Program {
                        program: &program,
                        args: &args,
                    }),
                    &[],
                )
                .expect("session command"),
            None => (program, args),
        };

        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut cmd = CommandBuilder::new(program);
        cmd.args(&args);
        cmd.cwd(&cwd);
        // Never the tmux or screen this test runs inside: tmux follows an
        // inherited `$TMUX` over `TMUX_TMPDIR`, so a test run from an okena
        // terminal would start — and then kill — the user's own server.
        cmd.env_remove("TMUX");
        cmd.env_remove("TMUX_PANE");
        cmd.env_remove("STY");
        cmd.env("TERM", "xterm-256color");
        cmd.env("TMUX_TMPDIR", &cwd);
        cmd.env("SCREENDIR", dir.0.join("screen"));
        let mut child = pty.slave.spawn_command(cmd).unwrap();
        drop(pty.slave);
        let mut reader = pty.master.try_clone_reader().unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while matches!(std::io::Read::read(&mut reader, &mut buf), Ok(n) if n > 0) {}
        });

        let started = Instant::now();
        while !out.exists() && started.elapsed() < Duration::from_secs(15) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        if matches!(backend, Some(ResolvedBackend::Tmux)) {
            let _ = std::process::Command::new("tmux")
                .env_remove("TMUX")
                .arg("-S")
                .arg(
                    dir.0
                        .join(format!("tmux-{}", unsafe { libc::getuid() }))
                        .join("default"),
                )
                .arg("kill-server")
                .output();
        }
        if matches!(backend, Some(ResolvedBackend::Dtach)) {
            let _ = std::fs::remove_file(get_dtach_socket_path(&session));
        }
        let bytes = std::fs::read(&out)
            .unwrap_or_else(|e| panic!("{backend:?}: the agent never ran ({e})"));
        bytes
            .split(|b| *b == 0)
            .map(|a| String::from_utf8(a.to_vec()).unwrap())
            .collect::<Vec<_>>()
            .split_last()
            .map(|(_, rest)| rest.to_vec())
            .unwrap_or_default()
    }

    fn expected(brief: &str) -> Vec<String> {
        ["--session-id", "abc", brief, "--mcp-config", "m b"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn the_agent_gets_the_brief_byte_for_byte_without_a_session_backend() {
        let brief = awkward_brief();
        assert_eq!(received(None, &brief), expected(&brief));
        // A short brief and an empty one arrive as they would have in argv.
        assert_eq!(received(None, "Fix it"), expected("Fix it"));
        assert_eq!(received(None, ""), expected(""));
    }

    #[test]
    fn the_agent_gets_the_brief_byte_for_byte_through_tmux() {
        if inside_a_multiplexer() {
            eprintln!("skipped: running inside tmux or screen ($TMUX/$STY is set)");
            return;
        }
        if !installed("tmux") {
            eprintln!("skipped: tmux is not installed");
            return;
        }
        let brief = awkward_brief();
        assert_eq!(
            received(Some(ResolvedBackend::Tmux), &brief),
            expected(&brief)
        );
    }

    #[test]
    fn the_agent_gets_the_brief_byte_for_byte_through_screen() {
        if inside_a_multiplexer() {
            eprintln!("skipped: running inside tmux or screen ($TMUX/$STY is set)");
            return;
        }
        if !installed("screen") {
            eprintln!("skipped: screen is not installed");
            return;
        }
        let brief = awkward_brief();
        assert_eq!(
            received(Some(ResolvedBackend::Screen), &brief),
            expected(&brief)
        );
    }

    #[test]
    fn the_agent_gets_the_brief_byte_for_byte_through_dtach() {
        if !installed("dtach") {
            eprintln!("skipped: dtach is not installed");
            return;
        }
        let brief = awkward_brief();
        assert_eq!(
            received(Some(ResolvedBackend::Dtach), &brief),
            expected(&brief)
        );
    }
}
