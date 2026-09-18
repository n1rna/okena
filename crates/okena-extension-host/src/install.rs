//! Getting an extension onto the machine: fetch the ref from git (or read a
//! local folder), read its manifest, use its prebuilt `extension.wasm` or
//! build it from source, and record it with what the user approved.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use okena_core::extension::{ExtInstallPreview, ExtPermissions, ExtSource};

use crate::exec::{self, SearchPath};
use crate::manifest::{Manifest, PREBUILT_WASM};
use crate::store::{Dirs, InstalledRecord, now_ms, write_atomic};

const GIT_TIMEOUT: Duration = Duration::from_secs(300);
const BUILD_TIMEOUT: Duration = Duration::from_secs(20 * 60);
pub const WASM_TARGET: &str = "wasm32-wasip2";

/// Dropped from builds and the toolchain check alike: the daemon may itself
/// run under cargo, and the extension's own rust-toolchain.toml and settings
/// decide how it builds.
const BUILD_ENV_REMOVE: &[&str] = &[
    "RUSTUP_TOOLCHAIN",
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_TARGET",
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
];

/// An extension fetched and read, ready to show for approval or install.
#[derive(Clone, Debug)]
pub struct Prepared {
    /// The source with the commit it resolved to.
    pub source: ExtSource,
    /// The folder holding `extension.toml`.
    pub dir: PathBuf,
    pub manifest: Manifest,
    pub prebuilt: bool,
}

impl Prepared {
    pub fn preview(&self, search: &SearchPath) -> ExtInstallPreview {
        ExtInstallPreview {
            id: self.manifest.id.clone(),
            name: self.manifest.name.clone(),
            version: self.manifest.version.clone(),
            description: self.manifest.description.clone(),
            source: self.source.clone(),
            permissions: self.manifest.permissions.clone(),
            added_permissions: None,
            requires: self.manifest.requires.clone(),
            prebuilt: self.prebuilt,
            build_problem: if self.prebuilt {
                None
            } else {
                build_problem(&self.dir, search)
            },
            installed: false,
        }
    }
}

/// Fetches `source` and reads its manifest. For git, the ref is fetched
/// into a cache checkout and checked out at the commit it resolves to.
pub fn prepare(dirs: &Dirs, source: &ExtSource, search: &SearchPath) -> Result<Prepared, String> {
    let (source, dir) = match source {
        ExtSource::Git {
            url, git_ref, path, ..
        } => {
            let url = okena_git::validate_clone_url(url)
                .map_err(|_| format!("`{url}` is not a git URL"))?
                .to_string();
            let git_ref = git_ref.as_deref().map(str::trim).filter(|r| !r.is_empty());
            let path = path
                .as_deref()
                .map(|p| p.trim().trim_matches('/'))
                .filter(|p| !p.is_empty());
            let checkout = dirs.repo_cache(&url);
            let commit = fetch(&checkout, &url, git_ref, search)?;
            let dir = match path {
                Some(path) => {
                    let relative = safe_relative(path)?;
                    checkout.join(relative)
                }
                None => checkout.clone(),
            };
            (
                ExtSource::Git {
                    url,
                    git_ref: git_ref.map(Into::into),
                    path: path.map(Into::into),
                    commit,
                },
                dir,
            )
        }
        ExtSource::Local { path } => {
            let dir = PathBuf::from(path.trim());
            if !dir.is_absolute() {
                return Err(format!("`{path}` is not an absolute path"));
            }
            (ExtSource::Local { path: path.trim().into() }, dir)
        }
    };
    if !dir.is_dir() {
        return Err(format!("{} does not exist in the source", dir.display()));
    }
    if !dir.join(crate::manifest::MANIFEST_FILE).is_file() {
        return Err(no_manifest_here(&dir, &source));
    }
    let manifest = Manifest::load(&dir)?;
    let prebuilt = dir.join(PREBUILT_WASM).is_file();
    Ok(Prepared {
        source,
        dir,
        manifest,
        prebuilt,
    })
}

/// What to say when the folder has no manifest: most likely the root of a
/// library, so name the extension folders inside it.
fn no_manifest_here(dir: &Path, source: &ExtSource) -> String {
    let mut found = Vec::new();
    find_manifests(dir, dir, 0, &mut found);
    found.sort();
    let what = match source {
        ExtSource::Git { .. } => "Folder in the repository",
        ExtSource::Local { .. } => "Folder",
    };
    if found.is_empty() {
        format!(
            "There is no {} in {}. Point {what} at the folder holding the extension's manifest.",
            crate::manifest::MANIFEST_FILE,
            dir.display()
        )
    } else {
        let shown: Vec<String> = found.iter().take(8).map(|p| format!("  {p}")).collect();
        format!(
            "There is no {} at the top of {}; it looks like a library. Set {what} to one of:\n{}",
            crate::manifest::MANIFEST_FILE,
            dir.display(),
            shown.join("\n")
        )
    }
}

/// Folders under `root` holding an `extension.toml`, relative to `base`,
/// a few levels deep.
fn find_manifests(base: &Path, root: &Path, depth: usize, found: &mut Vec<String>) {
    if depth > 4 || found.len() >= 50 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !path.is_dir() || name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if path.join(crate::manifest::MANIFEST_FILE).is_file() {
            if let Ok(relative) = path.strip_prefix(base) {
                found.push(relative.to_string_lossy().into_owned());
            }
        } else {
            find_manifests(base, &path, depth + 1, found);
        }
    }
}

/// A path inside the repo, refusing anything that climbs out of it.
fn safe_relative(path: &str) -> Result<PathBuf, String> {
    let relative = Path::new(path);
    if relative
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
    {
        Ok(relative.to_path_buf())
    } else {
        Err(format!("the path `{path}` must stay inside the repository"))
    }
}

fn git(
    search: &SearchPath,
    cwd: Option<&Path>,
    args: &[&str],
) -> Result<exec::Finished, String> {
    let program = search
        .resolve("git")
        .ok_or("installing extensions needs git, which is not on PATH")?;
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let done = exec::run(exec::Run {
        program: &program,
        args: &args,
        cwd,
        // Never wait on a credential prompt nobody can see.
        env: &[("GIT_TERMINAL_PROMPT".into(), "0".into())],
        remove_env: &[],
        stdin: None,
        timeout: GIT_TIMEOUT,
        search_path: search,
    })?;
    if done.timed_out {
        return Err(format!("`git {}` took longer than {}s", args.join(" "), GIT_TIMEOUT.as_secs()));
    }
    Ok(done)
}

fn git_ok(search: &SearchPath, cwd: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let done = git(search, cwd, args)?;
    if done.exit_code == Some(0) {
        Ok(done.stdout)
    } else {
        Err(format!(
            "`git {}` failed: {}",
            args.join(" "),
            done.stderr.trim()
        ))
    }
}

fn looks_like_commit(git_ref: &str) -> bool {
    (7..=40).contains(&git_ref.len()) && git_ref.chars().all(|c| c.is_ascii_hexdigit())
}

/// Brings the cache checkout for `url` to `git_ref` (the remote's default
/// branch when `None`) and returns the commit it is at.
fn fetch(checkout: &Path, url: &str, git_ref: Option<&str>, search: &SearchPath) -> Result<String, String> {
    if !checkout.join(".git").is_dir() {
        if let Some(parent) = checkout.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let _ = std::fs::remove_dir_all(checkout);
        let target = checkout.to_string_lossy().to_string();
        git_ok(search, None, &["clone", "--no-checkout", "--quiet", "--", url, &target])
            .map_err(|e| format!("cloning {url}: {e}"))?;
    } else {
        git_ok(search, Some(checkout), &["remote", "set-url", "origin", url])?;
    }

    let wanted = git_ref.unwrap_or("HEAD");
    let fetched = git(search, Some(checkout), &["fetch", "--quiet", "--force", "origin", wanted])?;
    let commit = if fetched.exit_code == Some(0) {
        git_ok(search, Some(checkout), &["rev-parse", "FETCH_HEAD"])?
    } else if git_ref.is_some_and(looks_like_commit) {
        // Servers that refuse fetching a bare commit still serve it within
        // a full fetch.
        git_ok(search, Some(checkout), &["fetch", "--quiet", "--force", "origin"])?;
        git_ok(search, Some(checkout), &["rev-parse", "--verify", &format!("{wanted}^{{commit}}")])
            .map_err(|_| format!("{url} has no commit `{wanted}`"))?
    } else {
        return Err(format!(
            "{url} has no ref `{wanted}`: {}",
            fetched.stderr.trim()
        ));
    };
    let commit = commit.trim().to_string();
    git_ok(search, Some(checkout), &["checkout", "--quiet", "--force", "--detach", &commit])?;
    git_ok(search, Some(checkout), &["clean", "-ffdq"])?;
    Ok(commit)
}

/// The commit `git_ref` points at on the remote now, without fetching, or
/// `None` when the ref is itself a commit (pinned, never updates).
pub fn remote_commit(url: &str, git_ref: Option<&str>, search: &SearchPath) -> Result<Option<String>, String> {
    let wanted = git_ref.unwrap_or("HEAD");
    if looks_like_commit(wanted) {
        return Ok(None);
    }
    let out = git_ok(search, None, &["ls-remote", "--", url, wanted])?;
    // Prefer an exact branch or tag match; a peeled tag (`^{}`) names the commit.
    let lines: Vec<(&str, &str)> = out
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .collect();
    let peeled = lines
        .iter()
        .find(|(_, name)| name.ends_with("^{}"))
        .map(|(sha, _)| *sha);
    let first = lines.first().map(|(sha, _)| *sha);
    match peeled.or(first) {
        Some(sha) => Ok(Some(sha.to_string())),
        None => Err(format!("{url} has no ref `{wanted}`")),
    }
}

/// Why this machine cannot build the extension in `dir` from source, if it cannot.
pub fn build_problem(dir: &Path, search: &SearchPath) -> Option<String> {
    if !dir.join("Cargo.toml").is_file() {
        return Some(format!(
            "there is no prebuilt {PREBUILT_WASM} and no Cargo.toml to build one from"
        ));
    }
    if search.resolve("cargo").is_none() {
        return Some(
            "there is no prebuilt extension.wasm, and building it from source needs Rust, which is not installed. Install it from https://rustup.rs, then run `rustup target add wasm32-wasip2`.".into(),
        );
    }
    let Some(rustup) = search.resolve("rustup") else {
        // Without rustup the target cannot be checked ahead of the build;
        // the build itself says so if it is missing.
        return None;
    };
    let args = vec!["target".to_string(), "list".into(), "--installed".into()];
    let listed = exec::run(exec::Run {
        program: &rustup,
        args: &args,
        cwd: Some(dir),
        env: &[],
        remove_env: BUILD_ENV_REMOVE,
        stdin: None,
        timeout: Duration::from_secs(60),
        search_path: search,
    })
    .ok()?;
    if listed.exit_code == Some(0) && !listed.stdout.lines().any(|l| l.trim() == WASM_TARGET) {
        return Some(format!(
            "there is no prebuilt extension.wasm, and building it from source needs the {WASM_TARGET} Rust target, which is not installed. Run `rustup target add {WASM_TARGET}`."
        ));
    }
    None
}

/// The component: the prebuilt one, or built from source.
pub fn component_bytes(prepared: &Prepared, dirs: &Dirs, search: &SearchPath) -> Result<Vec<u8>, String> {
    let prebuilt = prepared.dir.join(PREBUILT_WASM);
    if prebuilt.is_file() {
        return std::fs::read(&prebuilt).map_err(|e| format!("reading {}: {e}", prebuilt.display()));
    }
    if let Some(problem) = build_problem(&prepared.dir, search) {
        return Err(problem);
    }
    build(&prepared.dir, dirs, search)
}

fn build(dir: &Path, dirs: &Dirs, search: &SearchPath) -> Result<Vec<u8>, String> {
    let cargo = search.resolve("cargo").ok_or("cargo is not on PATH")?;
    let target_dir = dirs.build_target();
    let args = vec![
        "build".to_string(),
        "--release".into(),
        "--target".into(),
        WASM_TARGET.into(),
        "--target-dir".into(),
        target_dir.to_string_lossy().into_owned(),
    ];
    log::info!("building extension in {}", dir.display());
    let done = exec::run(exec::Run {
        program: &cargo,
        args: &args,
        cwd: Some(dir),
        env: &[],
        remove_env: BUILD_ENV_REMOVE,
        stdin: None,
        timeout: BUILD_TIMEOUT,
        search_path: search,
    })?;
    if done.timed_out {
        return Err(format!("building took longer than {} minutes", BUILD_TIMEOUT.as_secs() / 60));
    }
    if done.exit_code != Some(0) {
        let tail: Vec<&str> = done.stderr.lines().rev().take(30).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        if done.stderr.contains("target may not be installed") {
            return Err(format!(
                "building needs the {WASM_TARGET} Rust target, which is not installed. Run `rustup target add {WASM_TARGET}`."
            ));
        }
        return Err(format!("building from source failed:\n{}", tail.join("\n")));
    }
    let name = package_name(dir)?;
    let artifact = target_dir
        .join(WASM_TARGET)
        .join("release")
        .join(format!("{}.wasm", name.replace('-', "_")));
    std::fs::read(&artifact).map_err(|e| {
        format!(
            "the build finished but {} is missing ({e}); is the crate a `cdylib`?",
            artifact.display()
        )
    })
}

fn package_name(dir: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(dir.join("Cargo.toml")).map_err(|e| e.to_string())?;
    let value: toml::Value = toml::from_str(&text).map_err(|e| format!("Cargo.toml: {e}"))?;
    value
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(str::to_string)
        .ok_or_else(|| "Cargo.toml has no [package] name".into())
}

/// Writes the manifest and component into the installed folder.
pub fn place(
    dirs: &Dirs,
    prepared: &Prepared,
    wasm: &[u8],
    approved: ExtPermissions,
) -> Result<InstalledRecord, String> {
    let id = &prepared.manifest.id;
    let manifest_text = std::fs::read(prepared.dir.join(crate::manifest::MANIFEST_FILE))
        .map_err(|e| e.to_string())?;
    write_atomic(&dirs.installed_wasm(id), wasm)?;
    write_atomic(&dirs.installed_manifest(id), &manifest_text)?;
    Ok(InstalledRecord {
        id: id.clone(),
        version: prepared.manifest.version.clone(),
        source: prepared.source.clone(),
        approved,
        installed_at_ms: now_ms(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_library_root_names_the_extension_folders_in_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        for id in ["cli-table", "git-tree"] {
            let ext = dir.path().join("extensions").join(id);
            std::fs::create_dir_all(&ext).expect("mkdir");
            std::fs::write(ext.join("extension.toml"), "").expect("write");
        }
        let source = ExtSource::Local {
            path: dir.path().to_string_lossy().into_owned(),
        };
        let err = prepare(&Dirs::new(dir.path().join("x")), &source, &SearchPath::from_env())
            .expect_err("no manifest at the root");
        assert!(err.contains("looks like a library"), "{err}");
        assert!(err.contains("extensions/cli-table") && err.contains("extensions/git-tree"), "{err}");
    }

    #[test]
    fn repo_paths_cannot_climb_out() {
        assert!(safe_relative("extensions/cli-table").is_ok());
        assert!(safe_relative("./extensions/x").is_ok());
        assert!(safe_relative("../x").is_err());
        assert!(safe_relative("extensions/../../x").is_err());
        assert!(safe_relative("/etc").is_err());
    }

    #[test]
    fn commit_like_refs_are_pinned() {
        assert!(looks_like_commit("0123abc"));
        assert!(looks_like_commit(&"a".repeat(40)));
        assert!(!looks_like_commit("main"));
        assert!(!looks_like_commit("v1.2.3"));
        assert!(!looks_like_commit("abc"));
    }

    #[test]
    fn a_folder_without_sources_or_prebuilt_says_so() {
        let dir = tempfile::tempdir().expect("tempdir");
        let problem = build_problem(dir.path(), &SearchPath::from_env()).expect("a problem");
        assert!(problem.contains("no Cargo.toml"), "{problem}");
    }

    #[test]
    fn missing_rust_is_reported_before_building() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").expect("write");
        let no_tools = SearchPath(Some(std::ffi::OsString::new()));
        let problem = build_problem(dir.path(), &no_tools).expect("a problem");
        assert!(problem.contains("needs Rust") && problem.contains("rustup.rs"), "{problem}");
    }
}
