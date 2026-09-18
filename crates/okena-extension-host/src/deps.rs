//! The dependency check: is each tool an extension requires installed, and
//! new enough? Until every check passes the extension runs nothing.

use std::time::Duration;

use okena_core::extension::{ExtRequiredTool, ExtToolStatus};

use crate::exec::{self, SearchPath};

const CHECK_TIMEOUT: Duration = Duration::from_secs(15);

/// Checks every tool, in manifest order.
pub fn check_tools(requires: &[ExtRequiredTool], search_path: &SearchPath) -> Vec<ExtToolStatus> {
    requires.iter().map(|tool| check_tool(tool, search_path)).collect()
}

pub fn all_ok(statuses: &[ExtToolStatus]) -> bool {
    statuses.iter().all(|s| s.ok)
}

fn check_tool(tool: &ExtRequiredTool, search_path: &SearchPath) -> ExtToolStatus {
    let failed = |problem: String| ExtToolStatus {
        name: tool.name.clone(),
        ok: false,
        version: None,
        problem: Some(problem),
        install_hint: tool.install_hint.clone(),
    };
    let Some((program, args)) = tool.check.split_first() else {
        return failed("the manifest gives no check command".into());
    };
    let Some(path) = search_path.resolve(program) else {
        return failed(format!("`{program}` is not installed (not found on PATH)"));
    };
    let done = match exec::run(exec::Run {
        program: &path,
        args,
        cwd: None,
        env: &[],
        stdin: None,
        timeout: CHECK_TIMEOUT,
        search_path,
    }) {
        Ok(done) => done,
        Err(e) => return failed(e),
    };
    if done.timed_out {
        return failed(format!("`{}` did not answer within {}s", tool.check.join(" "), CHECK_TIMEOUT.as_secs()));
    }
    if done.exit_code != Some(0) {
        return failed(format!(
            "`{}` failed: {}",
            tool.check.join(" "),
            first_line(&done.stderr).unwrap_or("no output")
        ));
    }
    let output = format!("{}\n{}", done.stdout, done.stderr);
    let found = find_version(&output);
    if let Some(min) = &tool.min_version {
        let (Some(found_version), Some(min_version)) = (found.as_deref().and_then(parse_version), parse_version(min)) else {
            return failed(format!(
                "cannot tell its version from `{}`: {}",
                tool.check.join(" "),
                first_line(&output).unwrap_or("no output")
            ));
        };
        if found_version < min_version {
            return ExtToolStatus {
                version: found,
                ..failed(format!("version {found_version} is older than the required {min}"))
            };
        }
    }
    ExtToolStatus {
        name: tool.name.clone(),
        ok: true,
        version: found,
        problem: None,
        install_hint: tool.install_hint.clone(),
    }
}

fn first_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|l| !l.is_empty())
}

/// The first `N.N` or `N.N.N` in a tool's `--version` output, e.g. `1.7.1`
/// from `jq-1.7.1` or `2.15.3` from `aws-cli/2.15.3 Python/3.11`.
pub fn find_version(output: &str) -> Option<String> {
    let bytes = output.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            let mut end = i;
            while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b'.') {
                end += 1;
            }
            let candidate = output[start..end].trim_end_matches('.');
            if candidate.contains('.') {
                return Some(candidate.to_string());
            }
            i = end;
        } else {
            i += 1;
        }
    }
    None
}

/// A lenient version: `1.6` is `1.6.0`, extra parts are ignored.
pub fn parse_version(text: &str) -> Option<semver::Version> {
    let mut parts = text.trim().trim_start_matches('v').split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |p| p.parse().ok())?;
    let patch = parts.next().map_or(Some(0), |p| p.parse().ok())?;
    Some(semver::Version::new(major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_found_in_common_outputs() {
        assert_eq!(find_version("jq-1.7.1").as_deref(), Some("1.7.1"));
        assert_eq!(find_version("aws-cli/2.15.3 Python/3.11.6 Darwin/23").as_deref(), Some("2.15.3"));
        assert_eq!(find_version("gh version 2.89.0 (2026-03-26)").as_deref(), Some("2.89.0"));
        assert_eq!(find_version("git version 2.39.5 (Apple Git-154)").as_deref(), Some("2.39.5"));
        assert_eq!(find_version("no digits here"), None);
        assert_eq!(parse_version("1.6"), Some(semver::Version::new(1, 6, 0)));
        assert_eq!(parse_version("v2"), Some(semver::Version::new(2, 0, 0)));
        assert_eq!(parse_version("x"), None);
    }

    #[cfg(unix)]
    mod with_programs {
        use super::super::*;
        use std::ffi::OsString;
        use std::os::unix::fs::PermissionsExt;

        /// A search path holding one fake tool that prints `output`.
        fn fake_tool(dir: &std::path::Path, name: &str, output: &str, code: i32) -> SearchPath {
            let script = dir.join(name);
            std::fs::write(&script, format!("#!/bin/sh\necho '{output}'\nexit {code}\n")).expect("write");
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            let path = std::env::join_paths([dir.to_path_buf(), "/bin".into(), "/usr/bin".into()]).expect("join");
            SearchPath(Some(path))
        }

        fn tool(min: Option<&str>) -> ExtRequiredTool {
            ExtRequiredTool {
                name: "jq".into(),
                check: vec!["jq".into(), "--version".into()],
                min_version: min.map(Into::into),
                install_hint: "brew install jq".into(),
            }
        }

        #[test]
        fn a_missing_tool_says_so_with_its_install_hint() {
            let dir = tempfile::tempdir().expect("tempdir");
            let search = SearchPath(Some(OsString::from(dir.path())));
            let [status] = check_tools(&[tool(None)], &search).try_into().expect("one");
            assert!(!status.ok);
            assert!(status.problem.as_deref().unwrap_or_default().contains("not installed"));
            assert_eq!(status.install_hint, "brew install jq");
        }

        #[test]
        fn a_tool_older_than_required_fails_and_a_new_enough_one_passes() {
            let dir = tempfile::tempdir().expect("tempdir");
            let search = fake_tool(dir.path(), "jq", "jq-1.5", 0);
            let [old] = check_tools(&[tool(Some("1.6"))], &search).try_into().expect("one");
            assert!(!old.ok);
            assert_eq!(old.version.as_deref(), Some("1.5"));
            assert!(old.problem.as_deref().unwrap_or_default().contains("older than the required 1.6"));

            let search = fake_tool(dir.path(), "jq", "jq-1.7.1", 0);
            let [new] = check_tools(&[tool(Some("1.6"))], &search).try_into().expect("one");
            assert!(new.ok, "{new:?}");
            assert_eq!(new.version.as_deref(), Some("1.7.1"));
        }

        #[test]
        fn a_failing_check_command_fails_the_tool() {
            let dir = tempfile::tempdir().expect("tempdir");
            let search = fake_tool(dir.path(), "jq", "broken", 2);
            let [status] = check_tools(&[tool(None)], &search).try_into().expect("one");
            assert!(!status.ok);
            assert!(status.problem.as_deref().unwrap_or_default().contains("failed"));
        }
    }
}
