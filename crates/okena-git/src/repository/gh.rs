//! Finding the `gh` binary without trusting the inherited `PATH`.
//!
//! okena started from the Dock, a desktop entry or a service manager inherits
//! a minimal `PATH` (`/usr/bin:/bin:…`) that misses Homebrew and user installs,
//! so `Command::new("gh")` fails for most people who have gh. The binary is
//! looked for, in order, at the path set in Settings (`gh_path`), on `PATH`,
//! then in the directories gh is commonly installed to. Like
//! `get_extended_path` for terminals, this probes directories rather than
//! asking a login shell, which can hang or print noise.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use okena_core::process::command;
use parking_lot::{Mutex, RwLock};

#[cfg(windows)]
const GH_EXE: &str = "gh.exe";
#[cfg(not(windows))]
const GH_EXE: &str = "gh";

/// A gh that could not be found is looked for again this often, so installing
/// it is noticed without a restart.
const MISSING_GH_TTL: Duration = Duration::from_secs(60);

/// The path set in Settings, `~` expanded. The daemon installs it with
/// [`set_gh_path`].
static CONFIGURED: RwLock<Option<PathBuf>> = RwLock::new(None);

/// The last lookup: where gh was found (or not) and when.
static RESOLVED: Mutex<Option<(Option<PathBuf>, Instant)>> = Mutex::new(None);

/// Use the gh binary at `path` — a file, or a directory holding `gh` — ahead
/// of any other. `None` or blank goes back to looking it up.
pub fn set_gh_path(path: Option<&str>) {
    *CONFIGURED.write() = path
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(expand_home);
    *RESOLVED.lock() = None;
}

/// Where gh will be run from, or `None` when it is nowhere to be found.
pub fn resolved_gh_path() -> Option<PathBuf> {
    let mut cache = RESOLVED.lock();
    if let Some((found, at)) = cache.as_ref() {
        match found {
            Some(path) if is_executable(path) => return Some(path.clone()),
            None if at.elapsed() < MISSING_GH_TTL => return None,
            _ => {}
        }
    }
    let configured = CONFIGURED.read().clone();
    let found = resolve_gh(
        configured.as_deref(),
        std::env::var_os("PATH").as_deref(),
        &well_known_dirs(),
    );
    match &found {
        Some(path) => log::debug!("Using gh at {}", path.display()),
        None => log::warn!("gh not found on PATH or in the usual install directories"),
    }
    *cache = Some((found.clone(), Instant::now()));
    found
}

/// A command for gh, run from wherever it was found. When it was found
/// nowhere, plain `gh`, so the spawn fails the way it always did.
pub(crate) fn gh_command() -> std::process::Command {
    match resolved_gh_path() {
        Some(path) => command(&path.to_string_lossy()),
        None => command(GH_EXE),
    }
}

/// The configured path if it is (or holds) an executable gh, else the first
/// gh on `path_var`, else the first in `fallback_dirs`.
fn resolve_gh(
    configured: Option<&Path>,
    path_var: Option<&std::ffi::OsStr>,
    fallback_dirs: &[PathBuf],
) -> Option<PathBuf> {
    if let Some(configured) = configured {
        let candidate = if configured.is_dir() {
            configured.join(GH_EXE)
        } else {
            configured.to_path_buf()
        };
        if is_executable(&candidate) {
            return Some(candidate);
        }
        log::warn!(
            "gh_path {} is not an executable gh; looking it up instead",
            configured.display()
        );
    }
    path_var
        .into_iter()
        .flat_map(std::env::split_paths)
        .chain(fallback_dirs.iter().cloned())
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(GH_EXE))
        .find(|candidate| is_executable(candidate))
}

/// Where gh is installed when it is not on the inherited `PATH`.
fn well_known_dirs() -> Vec<PathBuf> {
    let home = dirs::home_dir();
    let mut dirs = Vec::new();
    #[cfg(not(windows))]
    {
        dirs.extend(
            [
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/home/linuxbrew/.linuxbrew/bin",
                "/opt/local/bin",
                "/snap/bin",
                "/usr/bin",
            ]
            .map(PathBuf::from),
        );
        if let Some(home) = &home {
            dirs.insert(2, home.join(".local/bin"));
            dirs.push(home.join(".linuxbrew/bin"));
            dirs.push(home.join("bin"));
        }
    }
    #[cfg(windows)]
    {
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(dir) = std::env::var_os(var) {
                dirs.push(PathBuf::from(dir).join("GitHub CLI"));
            }
        }
        if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(dir).join("Programs").join("GitHub CLI"));
        }
        if let Some(home) = &home {
            dirs.push(home.join("scoop").join("shims"));
        }
    }
    dirs
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/").or_else(|| (path == "~").then_some("")) {
        Some(rest) => match dirs::home_dir() {
            Some(home) => home.join(rest),
            None => PathBuf::from(path),
        },
        None => PathBuf::from(path),
    }
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

#[cfg(all(test, unix))]
pub(crate) mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;

    /// A `gh` in `dir` that runs `script`, executable unless told otherwise.
    pub(crate) fn fake_gh(dir: &Path, script: &str, executable: bool) -> PathBuf {
        std::fs::create_dir_all(dir).expect("dir");
        let path = dir.join(GH_EXE);
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).expect("write gh");
        let mode = if executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("chmod");
        path
    }

    fn path_of(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).expect("joinable")
    }

    #[test]
    fn a_homebrew_gh_is_found_when_path_is_minimal() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let usr_bin = tmp.path().join("usr/bin");
        std::fs::create_dir_all(&usr_bin).expect("dir");
        let homebrew = tmp.path().join("opt/homebrew/bin");
        let gh = fake_gh(&homebrew, "exit 0", true);

        // The Dock's PATH has no gh; the Homebrew directory does.
        let path = path_of(&[&usr_bin]);
        assert_eq!(
            resolve_gh(None, Some(&path), std::slice::from_ref(&homebrew)),
            Some(gh.clone())
        );
        assert_eq!(resolve_gh(None, None, &[homebrew]), Some(gh));
    }

    #[test]
    fn gh_on_path_comes_before_the_install_directories() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let on_path = fake_gh(&tmp.path().join("path"), "exit 0", true);
        let homebrew = tmp.path().join("homebrew");
        fake_gh(&homebrew, "exit 0", true);
        let path = path_of(&[on_path.parent().unwrap()]);
        assert_eq!(resolve_gh(None, Some(&path), &[homebrew]), Some(on_path));
    }

    #[test]
    fn a_configured_gh_wins_as_a_file_or_a_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let custom = fake_gh(&tmp.path().join("custom"), "exit 0", true);
        let on_path = fake_gh(&tmp.path().join("path"), "exit 0", true);
        let path = path_of(&[on_path.parent().unwrap()]);

        assert_eq!(
            resolve_gh(Some(&custom), Some(&path), &[]),
            Some(custom.clone())
        );
        assert_eq!(
            resolve_gh(Some(custom.parent().unwrap()), Some(&path), &[]),
            Some(custom)
        );
    }

    #[test]
    fn a_configured_path_that_is_no_gh_falls_back_to_the_lookup() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let on_path = fake_gh(&tmp.path().join("path"), "exit 0", true);
        let path = path_of(&[on_path.parent().unwrap()]);
        let not_executable = fake_gh(&tmp.path().join("plain"), "exit 0", false);
        let missing = tmp.path().join("nowhere/gh");

        for configured in [&not_executable, &missing] {
            assert_eq!(
                resolve_gh(Some(configured), Some(&path), &[]),
                Some(on_path.clone()),
                "{}",
                configured.display()
            );
        }
    }

    #[test]
    fn no_gh_anywhere_is_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(&empty).expect("dir");
        // A non-executable gh does not count.
        let plain = tmp.path().join("plain");
        fake_gh(&plain, "exit 0", false);
        let path = path_of(&[&empty]);
        assert_eq!(resolve_gh(None, Some(&path), &[plain, empty]), None);
    }

    #[test]
    fn install_directories_cover_homebrew_and_user_installs() {
        let dirs = well_known_dirs();
        for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/home/linuxbrew/.linuxbrew/bin"] {
            assert!(dirs.contains(&PathBuf::from(dir)), "{dir}");
        }
        if let Some(home) = dirs::home_dir() {
            assert!(dirs.contains(&home.join(".local/bin")));
        }
    }

    #[test]
    fn a_configured_path_expands_home() {
        if let Some(home) = dirs::home_dir() {
            assert_eq!(expand_home("~/bin/gh"), home.join("bin/gh"));
            assert_eq!(expand_home("~"), home);
        }
        assert_eq!(expand_home("/opt/gh"), PathBuf::from("/opt/gh"));
    }
}
