//! Running programs for extensions: found on the daemon's search path, fed
//! stdin, cut off at a timeout, their output capped.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Output beyond this is dropped, so a runaway program cannot fill memory.
const MAX_OUTPUT: usize = 16 * 1024 * 1024;

/// Once the program itself is gone, how long its reader threads get to collect
/// what is already sitting in the pipes before anything it left behind holding
/// them is killed. Same bound, for the same reason, as `POST_EXIT_DRAIN` in
/// `okena_core::process::bus`.
const POST_EXIT_DRAIN: Duration = Duration::from_millis(100);

/// Where programs are looked up. The daemon passes an extended PATH, since
/// one started from the Dock inherits a minimal one.
#[derive(Clone, Debug, Default)]
pub struct SearchPath(pub Option<OsString>);

impl SearchPath {
    pub fn from_env() -> Self {
        Self(std::env::var_os("PATH"))
    }

    /// The executable `program` names: itself when it is a path, else the
    /// first match on the search path.
    pub fn resolve(&self, program: &str) -> Option<PathBuf> {
        let as_path = Path::new(program);
        if as_path.components().count() > 1 || as_path.is_absolute() {
            return is_executable(as_path).then(|| as_path.to_path_buf());
        }
        let dirs = self.0.as_ref()?;
        std::env::split_paths(dirs)
            .filter(|dir| !dir.as_os_str().is_empty())
            .flat_map(|dir| candidates(&dir, program))
            .find(|candidate| is_executable(candidate))
    }
}

#[cfg(windows)]
fn candidates(dir: &Path, program: &str) -> Vec<PathBuf> {
    let mut out = vec![dir.join(program)];
    for ext in ["exe", "cmd", "bat"] {
        out.push(dir.join(format!("{program}.{ext}")));
    }
    out
}

#[cfg(not(windows))]
fn candidates(dir: &Path, program: &str) -> Vec<PathBuf> {
    vec![dir.join(program)]
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

pub struct Run<'a> {
    pub program: &'a Path,
    pub args: &'a [String],
    pub cwd: Option<&'a Path>,
    pub env: &'a [(String, String)],
    /// Inherited variables to drop.
    pub remove_env: &'a [&'a str],
    pub stdin: Option<&'a str>,
    pub timeout: Duration,
    pub search_path: &'a SearchPath,
}

pub struct Finished {
    /// `None` when killed by a signal or the timeout.
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// Run a program to completion or to its timeout, whichever comes first.
///
/// The timeout bounds this call, not just the program: whatever the program
/// starts is killed with it. That has to be deliberate, because a descendant
/// holds the stdout/stderr pipes it inherited, and collecting the output means
/// waiting for those pipes to close — so killing only the program we spawned
/// would leave this returning whenever *its children* felt like exiting.
pub fn run(run: Run<'_>) -> Result<Finished, String> {
    let mut command = okena_core::process::command(&run.program.to_string_lossy());
    command
        .args(run.args)
        .stdin(if run.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = &run.search_path.0 {
        command.env("PATH", path);
    }
    for key in run.remove_env {
        command.env_remove(key);
    }
    for (key, value) in run.env {
        command.env(key, value);
    }
    if let Some(cwd) = run.cwd {
        command.current_dir(cwd);
    }

    let (mut child, tree) = okena_core::process::ProcessTree::spawn(&mut command)
        .map_err(|e| format!("cannot run {}: {e}", run.program.display()))?;

    let stdin_writer = match (run.stdin, child.stdin.take()) {
        (Some(input), Some(mut pipe)) => {
            let input = input.to_owned();
            Some(std::thread::spawn(move || {
                // A program that exits without reading its input closes the pipe.
                let _ = pipe.write_all(input.as_bytes());
            }))
        }
        _ => None,
    };
    let stdout = child.stdout.take().map(reader);
    let stderr = child.stderr.take().map(reader);

    let deadline = Instant::now() + run.timeout;
    let mut timed_out = false;
    // `Child::try_wait` would reap the group leader as soon as it exits, and
    // its pid — which is the group id `terminate` signals — could then be
    // handed to an unrelated process while descendants still run under it.
    // `ProcessTree::exited` leaves the leader waitable until the group is gone.
    let status = loop {
        match okena_core::process::ProcessTree::exited(&mut child) {
            Ok(true) => {
                // The program has finished, but anything it left running still
                // holds the pipes. Take what is already buffered, then kill the
                // group so the joins below cannot wait on a stranger.
                drain_briefly(&stdout, &stderr, POST_EXIT_DRAIN);
                tree.terminate();
                break child.wait().ok();
            }
            Ok(false) if Instant::now() >= deadline => {
                timed_out = true;
                // The whole group, not just the direct child: a killed shell's
                // children keep its pipes open and this call would go on
                // waiting for them long past the timeout it was given.
                tree.terminate();
                let _ = child.kill();
                break child.wait().ok();
            }
            Ok(false) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(format!("waiting for {}: {e}", run.program.display())),
        }
    };

    if let Some(writer) = stdin_writer {
        let _ = writer.join();
    }
    let collect = |handle: Option<std::thread::JoinHandle<Vec<u8>>>| {
        handle
            .and_then(|h| h.join().ok())
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default()
    };
    Ok(Finished {
        exit_code: if timed_out {
            None
        } else {
            status.and_then(|s| s.code())
        },
        stdout: collect(stdout),
        stderr: collect(stderr),
        timed_out,
    })
}

/// Wait for both readers to reach EOF, giving up after `grace`.
///
/// Only ever a courtesy: the caller kills the process group straight after, and
/// that is what guarantees the pipes close and the joins return.
fn drain_briefly(
    stdout: &Option<std::thread::JoinHandle<Vec<u8>>>,
    stderr: &Option<std::thread::JoinHandle<Vec<u8>>>,
    grace: Duration,
) {
    let done = |handle: &Option<std::thread::JoinHandle<Vec<u8>>>| {
        handle.as_ref().is_none_or(|handle| handle.is_finished())
    };
    let deadline = Instant::now() + grace;
    while !(done(stdout) && done(stderr)) {
        if Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn reader(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut buf = [0u8; 64 * 1024];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if out.len() < MAX_OUTPUT {
                        let room = MAX_OUTPUT - out.len();
                        out.extend_from_slice(&buf[..n.min(room)]);
                    }
                }
            }
        }
        out
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str, stdin: Option<&str>, timeout: Duration) -> Finished {
        let search = SearchPath::from_env();
        let program = search.resolve("sh").expect("sh on PATH");
        let args = vec!["-c".to_string(), script.to_string()];
        run(Run {
            program: &program,
            args: &args,
            cwd: None,
            env: &[],
            remove_env: &[],
            stdin,
            timeout,
            search_path: &search,
        })
        .expect("runs")
    }

    #[test]
    fn output_exit_code_and_stdin_come_back() {
        let done = sh("cat; echo err >&2; exit 3", Some("piped"), Duration::from_secs(10));
        assert_eq!(done.stdout, "piped");
        assert_eq!(done.stderr.trim(), "err");
        assert_eq!(done.exit_code, Some(3));
        assert!(!done.timed_out);
    }

    #[test]
    fn a_program_past_its_timeout_is_killed() {
        let started = Instant::now();
        let done = sh("sleep 30", None, Duration::from_millis(200));
        assert!(done.timed_out);
        assert_eq!(done.exit_code, None);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    /// The timeout has to reach the program's children too. A shell that
    /// backgrounds `sleep` and waits leaves that `sleep` holding the stdout and
    /// stderr pipes it inherited, so killing the shell alone stops nothing that
    /// matters: this call still blocks collecting output until `sleep` exits on
    /// its own, 30 seconds after a 200ms deadline.
    #[test]
    fn a_timeout_kills_what_the_program_started_too() {
        let started = Instant::now();
        let done = sh("sleep 30 & echo $!; wait", None, Duration::from_millis(200));
        assert!(done.timed_out);
        assert!(started.elapsed() < Duration::from_secs(10));

        // Returning on time is not the same as having killed it. The shell
        // printed its child's pid before backgrounding it; `kill -0` says
        // whether that child is still around. SIGKILL is delivered
        // asynchronously and the orphan is reaped by init, so give it a moment
        // rather than asserting on the first probe.
        let pid: u32 = done.stdout.trim().parse().expect("the backgrounded pid");
        let mut gone = false;
        for _ in 0..100 {
            let probe = sh(&format!("kill -0 {pid}"), None, Duration::from_secs(5));
            if probe.exit_code != Some(0) {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            gone,
            "pid {pid} outlived the timeout that killed its parent"
        );
    }

    #[test]
    fn programs_resolve_on_the_search_path_only() {
        let empty = SearchPath(Some(OsString::new()));
        assert_eq!(empty.resolve("sh"), None);
        assert!(SearchPath::from_env().resolve("sh").is_some());
        assert!(SearchPath::from_env().resolve("no-such-program-okena").is_none());
    }
}
