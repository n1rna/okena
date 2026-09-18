//! Enforcing what the user approved: which programs an extension may run and
//! which paths it may read.

use std::path::{Path, PathBuf};

use okena_core::extension::ExtPermissions;

use crate::exec::SearchPath;

/// Why a host call did not go through.
#[derive(Debug, PartialEq, Eq)]
pub enum Denied {
    /// The approved permissions do not cover it. Shown on the extension.
    Refused(String),
    /// Allowed, but it could not be done (not installed, no such file).
    Failed(String),
}

impl Denied {
    pub fn message(&self) -> &str {
        match self {
            Denied::Refused(m) | Denied::Failed(m) => m,
        }
    }
}

/// Checks host calls against the approved permissions.
#[derive(Clone, Debug)]
pub struct Guard {
    approved: ExtPermissions,
    search_path: SearchPath,
}

impl Guard {
    pub fn new(approved: ExtPermissions, search_path: SearchPath) -> Self {
        Self {
            approved,
            search_path,
        }
    }

    pub fn search_path(&self) -> &SearchPath {
        &self.search_path
    }

    /// The executable to run for `program`, if the user approved it.
    pub fn command(&self, program: &str) -> Result<PathBuf, Denied> {
        if !self.approved.commands.iter().any(|c| c == program) {
            return Err(Denied::Refused(format!(
                "running `{program}` was refused: it is not one of the commands this extension was approved to run ({})",
                list_or_none(&self.approved.commands)
            )));
        }
        self.search_path
            .resolve(program)
            .ok_or_else(|| Denied::Failed(format!("`{program}` is not installed (not found on PATH)")))
    }

    /// `path`, resolved, if it lies under a path the user approved. `~` is
    /// the home directory; `config` fills in `{config.<key>}` patterns.
    pub fn path(&self, path: &str, config: &serde_json::Value) -> Result<PathBuf, Denied> {
        let requested = expand_home(path);
        if !requested.is_absolute() {
            return Err(Denied::Failed(format!("`{path}` is not an absolute path")));
        }
        let roots = self
            .approved
            .paths
            .iter()
            .filter_map(|pattern| expand_pattern(pattern, config))
            .collect::<Vec<_>>();
        let canonical_roots = roots
            .iter()
            .filter_map(|root| root.canonicalize().ok())
            .collect::<Vec<_>>();

        // Refuse before touching the disk, so a refused path's existence
        // does not leak through the error message.
        let lexical = normalize(&requested);
        let lexically_allowed = roots.iter().any(|root| lexical.starts_with(normalize(root)))
            || canonical_roots.iter().any(|root| lexical.starts_with(root));
        if !lexically_allowed {
            return Err(refused_path(path, &self.approved.paths));
        }
        let resolved = requested
            .canonicalize()
            .map_err(|e| Denied::Failed(format!("cannot read `{path}`: {e}")))?;
        // A symlink can point out of an approved directory; the resolved
        // path is what is checked.
        if canonical_roots.iter().any(|root| resolved.starts_with(root)) {
            Ok(resolved)
        } else {
            Err(refused_path(path, &self.approved.paths))
        }
    }

    /// A working directory for a command: allowed under an approved path.
    pub fn cwd(&self, dir: &str, config: &serde_json::Value) -> Result<PathBuf, Denied> {
        self.path(dir, config)
    }
}

fn refused_path(path: &str, approved: &[String]) -> Denied {
    Denied::Refused(format!(
        "reading `{path}` was refused: it is outside the paths this extension was approved to read ({})",
        list_or_none(approved)
    ))
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

/// A permission pattern with `~` and `{config.<key>}` filled in, or `None`
/// when it names a configuration field that is empty.
pub fn expand_pattern(pattern: &str, config: &serde_json::Value) -> Option<PathBuf> {
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(start) = rest.find("{config.") {
        out.push_str(&rest[..start]);
        let after = &rest[start + "{config.".len()..];
        let (key, tail) = after.split_once('}')?;
        let value = config.get(key)?.as_str()?.trim();
        if value.is_empty() {
            return None;
        }
        out.push_str(value);
        rest = tail;
    }
    out.push_str(rest);
    let path = expand_home(&out);
    path.is_absolute().then_some(path)
}

fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(path));
    }
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}

/// `..` and `.` removed without touching the disk.
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn guard(commands: &[&str], paths: &[&str]) -> Guard {
        Guard::new(
            ExtPermissions {
                commands: commands.iter().map(|c| c.to_string()).collect(),
                paths: paths.iter().map(|p| p.to_string()).collect(),
                start_agents: false,
            },
            SearchPath::from_env(),
        )
    }

    #[cfg(unix)]
    #[test]
    fn only_approved_commands_run() {
        let g = guard(&["sh"], &[]);
        assert!(g.command("sh").is_ok());
        let err = g.command("rm").expect_err("refused");
        assert!(matches!(err, Denied::Refused(ref m) if m.contains("`rm`") && m.contains("(sh)")));
        // A path to an approved program's name is still a different program.
        assert!(matches!(g.command("/bin/sh"), Err(Denied::Refused(_))));
    }

    #[test]
    fn an_approved_but_missing_command_fails_without_a_refusal() {
        let g = guard(&["no-such-program-okena"], &[]);
        assert!(matches!(g.command("no-such-program-okena"), Err(Denied::Failed(_))));
    }

    #[test]
    fn paths_are_allowed_under_approved_roots_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inside = dir.path().join("data");
        std::fs::create_dir_all(&inside).expect("mkdir");
        std::fs::write(inside.join("a.txt"), "a").expect("write");
        std::fs::write(dir.path().join("secret.txt"), "s").expect("write");
        let root = inside.to_string_lossy().to_string();
        let g = guard(&[], &[&root]);
        let cfg = json!({});

        assert!(g.path(&format!("{root}/a.txt"), &cfg).is_ok());
        assert!(g.path(&root, &cfg).is_ok());
        let secret = format!("{}/secret.txt", dir.path().display());
        assert!(matches!(g.path(&secret, &cfg), Err(Denied::Refused(_))));
        // `..` does not climb out.
        assert!(matches!(g.path(&format!("{root}/../secret.txt"), &cfg), Err(Denied::Refused(_))));
        assert!(matches!(g.path("relative.txt", &cfg), Err(Denied::Failed(_))));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_an_approved_root_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inside = dir.path().join("data");
        std::fs::create_dir_all(&inside).expect("mkdir");
        std::fs::write(dir.path().join("secret.txt"), "s").expect("write");
        std::os::unix::fs::symlink(dir.path().join("secret.txt"), inside.join("link")).expect("symlink");
        let root = inside.to_string_lossy().to_string();
        let g = guard(&[], &[&root]);
        let err = g.path(&format!("{root}/link"), &json!({})).expect_err("refused");
        assert!(matches!(err, Denied::Refused(_)));
    }

    #[test]
    fn config_placeholders_resolve_from_the_configuration() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().to_string_lossy().to_string();
        std::fs::write(dir.path().join("x"), "x").expect("write");
        let g = guard(&[], &["{config.repo}"]);

        assert!(matches!(
            g.path(&format!("{repo}/x"), &json!({ "repo": "" })),
            Err(Denied::Refused(_))
        ), "an empty config field grants nothing");
        assert!(g.path(&format!("{repo}/x"), &json!({ "repo": repo })).is_ok());
        assert_eq!(expand_pattern("{config.repo}/sub", &json!({ "repo": "/r" })), Some(PathBuf::from("/r/sub")));
        assert_eq!(expand_pattern("relative", &json!({})), None);
    }
}
