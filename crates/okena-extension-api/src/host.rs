//! Calls into okena: commands, files, storage, configuration, logging.
//!
//! Every call that touches the machine is checked against the permissions
//! the user approved at install. A refused call returns an error naming what
//! was refused, and okena shows the refusal on the extension.

use crate::wit::okena::extension::host as h;
use crate::wit::okena::extension::types as t;
use crate::Result;

pub use t::{DirEntry, LogLevel, Project};

/// A program to run, without a shell. The program must be one the manifest
/// declares under `permissions.commands`.
#[derive(Clone, Debug)]
pub struct Command {
    program: String,
    args: Vec<String>,
    cwd: Option<String>,
    env: Vec<(String, String)>,
    stdin: Option<String>,
    timeout_ms: Option<u32>,
}

/// What a finished program printed, and how it exited.
#[derive(Clone, Debug)]
pub struct CommandOutput {
    /// `None` when it was killed by a signal or the timeout.
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// `stdout` when the program exited 0, else an error with its stderr.
    pub fn into_stdout(self) -> Result<String> {
        if self.success() {
            Ok(self.stdout)
        } else {
            let status = match self.exit_code {
                Some(code) => format!("exited with {code}"),
                None => "was killed".to_string(),
            };
            let stderr = self.stderr.trim();
            Err(if stderr.is_empty() {
                format!("the command {status}")
            } else {
                format!("the command {status}: {stderr}")
            })
        }
    }
}

impl Command {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            stdin: None,
            timeout_ms: None,
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<S: Into<String>>(mut self, args: impl IntoIterator<Item = S>) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn current_dir(mut self, dir: impl Into<String>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn stdin(mut self, input: impl Into<String>) -> Self {
        self.stdin = Some(input.into());
        self
    }

    /// Killed after this long; okena caps it at five minutes.
    pub fn timeout_ms(mut self, timeout_ms: u32) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    /// Runs it and waits. `Err` means it could not run at all (refused, not
    /// found); a program that ran and failed is an `Ok` with its exit code.
    pub fn output(self) -> Result<CommandOutput> {
        let output = h::run_command(&t::Command {
            program: self.program,
            args: self.args,
            cwd: self.cwd,
            env: self.env,
            stdin: self.stdin,
            timeout_ms: self.timeout_ms,
        })?;
        Ok(CommandOutput {
            exit_code: output.exit_code,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    /// Runs it and returns its stdout, or an error if it did not exit 0.
    pub fn run(self) -> Result<String> {
        self.output()?.into_stdout()
    }
}

/// Reads a file under a path the manifest declares.
pub fn read_file(path: &str) -> Result<Vec<u8>> {
    h::read_file(path)
}

pub fn read_to_string(path: &str) -> Result<String> {
    let bytes = h::read_file(path)?;
    String::from_utf8(bytes).map_err(|_| format!("{path} is not UTF-8 text"))
}

/// Lists a directory under a path the manifest declares.
pub fn read_dir(path: &str) -> Result<Vec<DirEntry>> {
    h::read_dir(path)
}

/// The extension's configuration, as the user set it in Settings. Keys are
/// the manifest's `[[config]]` keys; unset fields take their defaults.
pub fn config<T: serde::de::DeserializeOwned>() -> Result<T> {
    serde_json::from_str(&h::config()).map_err(|e| format!("configuration: {e}"))
}

/// The configuration as raw JSON.
pub fn config_json() -> serde_json::Value {
    serde_json::from_str(&h::config()).unwrap_or(serde_json::Value::Null)
}

/// okena's projects, for choosing where an agent works.
pub fn projects() -> Vec<Project> {
    h::projects()
}

/// A string store private to the extension, kept across restarts and
/// deleted when it is removed.
pub mod storage {
    use super::h;
    use crate::Result;

    pub fn get(key: &str) -> Option<String> {
        h::kv_get(key)
    }

    pub fn set(key: &str, value: &str) -> Result<()> {
        h::kv_set(key, value)
    }

    pub fn delete(key: &str) {
        h::kv_delete(key)
    }

    pub fn keys() -> Vec<String> {
        h::kv_keys()
    }

    /// Reads a JSON value stored with [`set_json`].
    pub fn get_json<T: serde::de::DeserializeOwned>(key: &str) -> Option<T> {
        get(key).and_then(|raw| serde_json::from_str(&raw).ok())
    }

    pub fn set_json<T: serde::Serialize>(key: &str, value: &T) -> Result<()> {
        let raw = serde_json::to_string(value).map_err(|e| e.to_string())?;
        set(key, &raw)
    }
}

/// Writes to okena's log, tagged with the extension's id.
pub fn log(level: LogLevel, message: &str) {
    h::log(level, message)
}

pub fn info(message: &str) {
    log(LogLevel::Info, message)
}

pub fn warn(message: &str) {
    log(LogLevel::Warn, message)
}

pub fn error(message: &str) {
    log(LogLevel::Error, message)
}
