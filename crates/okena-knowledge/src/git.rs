//! The git a knowledge checkout needs: clone, plus the store git every store
//! shares (`okena_git::store`, ADR-0004) — sync state with the changed files,
//! fetch, fast-forward pull, and a commit of picked files with a separate push
//! — its errors carried over as [`KnowledgeError`]s with their codes.
//!
//! Every probe first checks that the root is itself the top of a checkout. Git
//! discovers repositories by walking up, so a store folder nested inside
//! another repo would otherwise report that repo's branch.

use crate::registry::{self, RegisterOutcome};
use crate::{KnowledgeError, display};
use okena_core::knowledge::{KnowledgeGitStatus, KnowledgeRootKind, KnowledgeStores};
use okena_git::GitError;
use okena_git::repository as git;
use okena_git::store;
use std::path::Path;

/// Local sync state of the checkout at `root`; `None` when `root` is not the
/// top of one. Reads no network, so it is as fresh as the last fetch.
pub fn status(root: &Path) -> Option<KnowledgeGitStatus> {
    store::status(root)
}

/// Fill in sync state on every healthy store.
pub fn attach_status(stores: &mut KnowledgeStores) {
    for root in stores
        .roots
        .iter_mut()
        .filter(|r| r.kind == KnowledgeRootKind::Store && r.healthy)
    {
        root.git = status(Path::new(&root.path));
    }
}

/// Clone `url` and register the checkout.
///
/// `dest` is where to clone, as a person typed it; unset clones into
/// `clone_dir/<repo name>`, the folder `git clone` would create. A repository
/// that turns out not to be a knowledge store is left on disk — the clone is
/// the user's to keep or remove — and the error says where it is.
pub fn clone_store(
    registry: &Path,
    url: &str,
    dest: Option<&str>,
    clone_dir: &Path,
) -> Result<RegisterOutcome, KnowledgeError> {
    let url = git::validate_clone_url(url).map_err(|_| {
        KnowledgeError::new("invalid_clone_url", "Enter a repository URL to clone.")
            .with_fix("For example git@github.com:acme/eng-knowledge.git.")
    })?;
    let target = match dest.map(str::trim).filter(|d| !d.is_empty()) {
        Some(dest) => registry::absolute_input(dest)?,
        None => clone_dir.join(git::clone_dir_name(url).ok_or_else(|| {
            KnowledgeError::new(
                "clone_name_unknown",
                format!("No folder name can be derived from {url}."),
            )
            .with_fix("Choose a destination folder.")
        })?),
    };
    let existed = target.exists();
    if let Err(e) = git::clone_repository(url, &target) {
        if !existed {
            let _ = std::fs::remove_dir_all(&target);
        }
        return Err(match e {
            GitError::CloneTargetExists { .. } => KnowledgeError::new(
                "clone_target_exists",
                format!("{} already exists and is not empty.", display(&target)),
            )
            .with_fix("Choose another destination, or add that folder as an existing store."),
            e => KnowledgeError::new(
                "clone_failed",
                format!("Could not clone {url}: {}", e.user_detail()),
            )
            .with_fix("Check the URL, and that git can reach it from a terminal."),
        });
    }
    let remote = git::origin_url(&target).or_else(|| Some(url.to_string()));
    registry::register(registry, &display(&target), remote).map_err(|mut e| {
        e.message = format!(
            "Cloned into {}, but it can't be added: {}",
            display(&target),
            e.message
        );
        e
    })
}

/// `git fetch --all` in the checkout at `root`.
pub fn fetch(root: &Path) -> Result<(), KnowledgeError> {
    Ok(store::fetch(root)?)
}

/// Fetch, then fast-forward a clean checkout's branch to its upstream.
/// Anything else is refused with the reason and where to resolve it. Returns
/// the sync state after.
pub fn pull(root: &Path) -> Result<KnowledgeGitStatus, KnowledgeError> {
    Ok(store::pull(root)?)
}

/// Commit exactly `paths`, each one the sync state lists as changed, with
/// `message`. Nothing is pushed. Returns the sync state after.
pub fn commit(
    root: &Path,
    paths: &[String],
    message: &str,
) -> Result<KnowledgeGitStatus, KnowledgeError> {
    Ok(store::commit(root, paths, message)?)
}

/// Push the checked-out branch to its upstream. A failed push keeps every
/// local commit. Returns the sync state after.
pub fn push(root: &Path) -> Result<KnowledgeGitStatus, KnowledgeError> {
    Ok(store::push(root)?)
}

#[cfg(test)]
pub(crate) mod testgit {
    use std::path::{Path, PathBuf};

    /// Run git in `dir` with a fixed identity and no signing, asserting success.
    pub fn git(dir: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// A bare `remote.git` in `dir` holding store `acme-eng`, and the `seed`
    /// checkout that pushes to it.
    pub fn remote_store(dir: &Path) -> (PathBuf, PathBuf) {
        let seed = dir.join("seed");
        std::fs::create_dir_all(&seed).expect("mkdir");
        git(&seed, &["init", "-b", "main"]);
        crate::testutil::store(&seed, "acme-eng");
        git(&seed, &["add", "."]);
        git(&seed, &["commit", "-m", "seed"]);
        let remote = dir.join("remote.git");
        git(dir, &["clone", "--bare", "seed", "remote.git"]);
        (remote, seed)
    }

    /// Commit `rel` in `seed` and push it to `remote`.
    pub fn publish(seed: &Path, remote: &Path, rel: &str) {
        crate::testutil::write(&seed.join(rel), "new\n");
        git(seed, &["add", "."]);
        git(seed, &["commit", "-m", rel]);
        git(seed, &["push", remote.to_str().expect("utf8"), "main"]);
    }
}

#[cfg(test)]
mod tests {
    use super::testgit::{git, publish, remote_store};
    use super::*;
    use crate::registry::{list, registry_path};
    use crate::testutil::write;

    fn file_url(path: &Path) -> String {
        format!("file://{}", display(path))
    }

    #[test]
    fn a_clone_registers_and_tracks_its_upstream_through_fetch_and_pull() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (remote, seed) = remote_store(dir.path());
        let registry = registry_path(&dir.path().join("config"));

        let out = clone_store(
            &registry,
            &file_url(&remote),
            None,
            &dir.path().join("knowledge"),
        )
        .expect("clone");
        assert_eq!(out.id, "acme-eng");
        let root = dir.path().join("knowledge/remote");
        assert_eq!(out.root, crate::canonical(&root));
        assert_eq!(list(&registry).expect("list")[0].id, "acme-eng");

        let synced = status(&root).expect("a checkout");
        assert_eq!(synced.branch.as_deref(), Some("main"));
        assert_eq!(synced.upstream.as_deref(), Some("origin/main"));
        assert_eq!((synced.ahead, synced.behind, synced.dirty), (0, 0, false));

        publish(&seed, &remote, "docs/new.md");
        fetch(&root).expect("fetch");
        let behind = status(&root).expect("status");
        assert_eq!(behind.behind, 1);
        assert!(behind.can_fast_forward());
        assert!(behind.fetched_at.is_some());

        let after = pull(&root).expect("pull");
        assert_eq!(after.behind, 0);
        assert!(root.join("docs/new.md").is_file());

        write(&root.join("docs/local.md"), "x");
        let dirty = status(&root).expect("status");
        assert!(dirty.dirty);
        assert_eq!(dirty.changes[0].path, "docs/local.md");
    }

    #[test]
    fn pull_refuses_anything_but_a_fast_forward() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (remote, seed) = remote_store(dir.path());
        let clone = dir.path().join("clone");
        git(dir.path(), &["clone", "remote.git", "clone"]);
        git(&clone, &["commit", "--allow-empty", "-m", "local"]);
        publish(&seed, &remote, "docs/upstream.md");
        assert_eq!(pull(&clone).expect_err("diverged").code, "diverged");

        // The seed has no upstream at all.
        assert_eq!(pull(&seed).expect_err("untracked").code, "no_upstream");

        let plain = dir.path().join("plain");
        std::fs::create_dir_all(&plain).expect("mkdir");
        assert_eq!(
            pull(&plain).expect_err("no repo").code,
            "not_a_git_checkout"
        );
        assert_eq!(
            commit(&plain, &["docs/a.md".to_string()], "m")
                .expect_err("no repo")
                .code,
            "not_a_git_checkout"
        );
    }

    #[test]
    fn up_to_date_pulls_are_a_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        remote_store(dir.path());
        git(dir.path(), &["clone", "remote.git", "clone"]);
        let st = pull(&dir.path().join("clone")).expect("pull");
        assert_eq!((st.ahead, st.behind), (0, 0));
    }

    #[test]
    fn failed_clones_leave_nothing_and_non_stores_stay_on_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry_path(&dir.path().join("config"));
        let knowledge = dir.path().join("knowledge");

        let missing = dir.path().join("missing.git");
        assert_eq!(
            clone_store(&registry, &file_url(&missing), None, &knowledge)
                .expect_err("no such repo")
                .code,
            "clone_failed"
        );
        assert!(!knowledge.join("missing").exists());

        let (remote, _seed) = remote_store(dir.path());
        write(&knowledge.join("remote/occupied.txt"), "x");
        assert_eq!(
            clone_store(&registry, &file_url(&remote), None, &knowledge)
                .expect_err("occupied")
                .code,
            "clone_target_exists"
        );
        assert!(
            knowledge.join("remote/occupied.txt").is_file(),
            "left alone"
        );

        let plain_seed = dir.path().join("plain-seed");
        std::fs::create_dir_all(&plain_seed).expect("mkdir");
        git(&plain_seed, &["init", "-b", "main"]);
        write(&plain_seed.join("README.md"), "x");
        git(&plain_seed, &["add", "."]);
        git(&plain_seed, &["commit", "-m", "plain"]);
        let err = clone_store(&registry, &file_url(&plain_seed), None, &knowledge)
            .expect_err("not a store");
        assert_eq!(err.code, "not_a_knowledge_root");
        assert!(err.message.starts_with("Cloned into"));
        assert!(knowledge.join("plain-seed/README.md").is_file());
        assert!(list(&registry).expect("list").is_empty());

        assert_eq!(
            clone_store(&registry, " -x ", None, &knowledge)
                .expect_err("flag")
                .code,
            "invalid_clone_url"
        );
    }

    #[test]
    fn a_store_folder_nested_in_another_repo_has_no_git_status() {
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init", "-b", "main"]);
        let nested = dir.path().join("knowledge");
        crate::testutil::store(&nested, "nested");
        assert_eq!(status(&nested), None);
        assert!(status(dir.path()).is_some());
    }
}
