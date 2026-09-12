//! Git for a store checkout okena syncs on a person's behalf — OpenSpec stores
//! and knowledge stores alike (ADR-0004): status with the changed files, fetch,
//! fast-forward pull, a commit of exactly the files a person picked, and a push
//! that is a separate step so a failed one never costs the commit.
//!
//! Every operation first checks that the root is itself the top of a checkout.
//! Git discovers repositories by walking up, so a store folder nested inside
//! another repo would otherwise report, and commit to, that repo.
//!
//! Errors carry a stable code, a message naming the checkout and a fix, so each
//! store crate can wrap them in its own error type unchanged.

use crate::repository::{self as git, UpstreamState};
use okena_core::process::{command, safe_output};
use okena_core::store_git::{MAX_LISTED_CHANGES, StoreChange, StoreChangeKind, StoreGitStatus};
use std::path::Path;
use std::time::UNIX_EPOCH;

/// A failed store git operation, with a stable code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreGitError {
    pub code: &'static str,
    pub message: String,
    /// A concrete next step.
    pub fix: Option<String>,
}

impl StoreGitError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            fix: None,
        }
    }

    fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }
}

impl std::fmt::Display for StoreGitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.fix {
            Some(fix) => write!(f, "{} — {}", self.message, fix),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for StoreGitError {}

/// Local sync state of the checkout at `root`; `None` when `root` is not the
/// top of one. Reads no network, so it is as fresh as the last fetch.
pub fn status(root: &Path) -> Option<StoreGitStatus> {
    if !git::is_repository_at_root(root) {
        return None;
    }
    let (branch, upstream) = match git::current_upstream(root) {
        Some((branch, upstream)) => (Some(branch), upstream),
        None => (None, UpstreamState::Untracked),
    };
    let (upstream, ahead, behind) = match upstream {
        UpstreamState::Tracked {
            name,
            ahead,
            behind,
        } => (Some(name), ahead, behind),
        UpstreamState::Gone | UpstreamState::Untracked => (None, 0, 0),
    };
    let (changes, changes_truncated, dirty) = match changed_files(root) {
        Some((changes, truncated)) => {
            let dirty = !changes.is_empty();
            (changes, truncated, dirty)
        }
        // "Could not tell" reads as dirty: the flag only prompts a look.
        None => (Vec::new(), false, true),
    };
    Some(StoreGitStatus {
        branch,
        upstream,
        ahead: u32::try_from(ahead).unwrap_or(u32::MAX),
        behind: u32::try_from(behind).unwrap_or(u32::MAX),
        dirty,
        changes,
        changes_truncated,
        fetched_at: fetched_at(root),
    })
}

/// Every file with uncommitted changes, sorted by path, and whether the list
/// was cut at [`MAX_LISTED_CHANGES`]. `None` when git refuses to report.
///
/// Untracked folders are listed file by file, so a commit can name exactly the
/// files it takes; renames are listed as the delete and the add they are.
fn changed_files(root: &Path) -> Option<(Vec<StoreChange>, bool)> {
    let output = safe_output(command("git").arg("-C").arg(root).args([
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--no-renames",
    ]))
    .ok()?;
    output
        .status
        .success()
        .then(|| parse_porcelain(&output.stdout))
}

/// `XY <path>` records, NUL-terminated: `X` is the index, `Y` the working
/// tree. `-z` leaves paths unquoted, and without renames no record carries a
/// second path.
fn parse_porcelain(stdout: &[u8]) -> (Vec<StoreChange>, bool) {
    let mut changes = Vec::new();
    let mut truncated = false;
    for record in stdout.split(|b| *b == 0) {
        let (Some(&x), Some(&y), Some(b' ')) = (record.first(), record.get(1), record.get(2))
        else {
            continue;
        };
        // A path that is not UTF-8 could not be sent back to commit, so it is
        // not offered.
        let Ok(path) = std::str::from_utf8(&record[3..]) else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        if changes.len() == MAX_LISTED_CHANGES {
            truncated = true;
            break;
        }
        changes.push(StoreChange {
            path: path.to_string(),
            kind: change_kind(x, y),
            staged: !matches!(x, b' ' | b'?'),
            unstaged: y != b' ',
        });
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    (changes, truncated)
}

fn change_kind(x: u8, y: u8) -> StoreChangeKind {
    match (x, y) {
        (b'?', b'?') => StoreChangeKind::Untracked,
        (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => StoreChangeKind::Conflicted,
        (b'D', _) | (_, b'D') => StoreChangeKind::Deleted,
        (b'A', _) => StoreChangeKind::Added,
        _ => StoreChangeKind::Modified,
    }
}

/// When the checkout last fetched: `FETCH_HEAD` is rewritten by every fetch.
fn fetched_at(root: &Path) -> Option<u64> {
    let fetch_head = git::get_repo_common_dir(root)?.join("FETCH_HEAD");
    let modified = std::fs::metadata(fetch_head).ok()?.modified().ok()?;
    modified
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// `git fetch --all` in the checkout at `root`.
pub fn fetch(root: &Path) -> Result<(), StoreGitError> {
    require_checkout(root)?;
    git::fetch_all(root).map_err(|e| {
        StoreGitError::new(
            "fetch_failed",
            format!("Could not fetch {}: {}", display(root), e.user_detail()),
        )
        .with_fix("Check that git can reach the remote from a terminal.")
    })
}

/// Fetch, then fast-forward the checked-out branch to its upstream.
///
/// Anything but a fast-forward of a clean checkout is refused with the reason
/// and where to resolve it: rebasing or merging someone's shared store is a
/// decision, not a button, and a pull must not land on top of unsaved edits.
/// Returns the sync state after.
pub fn pull(root: &Path) -> Result<StoreGitStatus, StoreGitError> {
    fetch(root)?;
    let before = status(root).ok_or_else(|| not_a_checkout(root))?;
    let at = display(root);
    let branch = on_branch(&before, &at)?;
    let Some(upstream) = before.upstream.clone() else {
        return Err(StoreGitError::new(
            "no_upstream",
            format!("`{branch}` in {at} has no upstream to pull from."),
        )
        .with_fix(format!(
            "Run `git branch --set-upstream-to origin/{branch}` in {at}."
        )));
    };
    if before.ahead > 0 && before.behind > 0 {
        return Err(StoreGitError::new(
            "diverged",
            format!(
                "`{branch}` is {} commit(s) ahead of {upstream} and {} behind; okena only fast-forwards.",
                before.ahead, before.behind
            ),
        )
        .with_fix(format!("Rebase or merge in a terminal at {at}.")));
    }
    if before.behind == 0 {
        return Ok(before);
    }
    if before.dirty {
        return Err(StoreGitError::new(
            "uncommitted_changes",
            format!("{at} has uncommitted changes; okena only fast-forwards a clean checkout."),
        )
        .with_fix("Commit them first, or stash them in a terminal."));
    }
    git::fast_forward_to_upstream(root).map_err(|e| {
        StoreGitError::new(
            "pull_failed",
            format!(
                "Could not fast-forward `{branch}` to {upstream}: {}",
                e.user_detail()
            ),
        )
        .with_fix(format!(
            "Run `git merge --ff-only @{{upstream}}` in a terminal at {at} to see why."
        ))
    })?;
    status(root).ok_or_else(|| not_a_checkout(root))
}

/// Commit exactly `paths` with `message`, and nothing else the checkout holds.
///
/// Every path must be one the status lists as changed. The client names files
/// from the list it showed, so anything else — a path outside the store, a
/// pathspec pattern, a file changed since — is refused rather than handed to
/// git. Anything already staged that was not picked stays out of the commit and
/// stays staged. Returns the sync state after; nothing is pushed.
pub fn commit(
    root: &Path,
    paths: &[String],
    message: &str,
) -> Result<StoreGitStatus, StoreGitError> {
    require_checkout(root)?;
    let at = display(root);
    let message = message.trim();
    if message.is_empty() {
        return Err(StoreGitError::new(
            "commit_message_empty",
            "Write a commit message first.",
        ));
    }
    if paths.is_empty() {
        return Err(StoreGitError::new(
            "nothing_to_commit",
            "Pick at least one changed file to commit.",
        ));
    }
    let before = status(root).ok_or_else(|| not_a_checkout(root))?;
    on_branch(&before, &at)?;
    for path in paths {
        match before.changes.iter().find(|c| &c.path == path) {
            None => {
                return Err(StoreGitError::new(
                    "not_a_changed_file",
                    format!("`{path}` has no uncommitted changes in {at}."),
                )
                .with_fix("Refresh, then commit from the current list."));
            }
            Some(c) if c.kind == StoreChangeKind::Conflicted => {
                return Err(StoreGitError::new(
                    "conflicted_file",
                    format!("`{path}` has unresolved conflicts."),
                )
                .with_fix(format!("Resolve them in a terminal at {at}.")));
            }
            Some(_) => {}
        }
    }
    if !git::has_commit_identity(root) {
        return Err(StoreGitError::new(
            "commit_identity_missing",
            "No git commit identity is configured, so okena cannot commit.",
        )
        .with_fix(
            "Run git config --global user.name \"Your Name\" and git config --global user.email \"you@example.com\".",
        ));
    }
    let pathspecs: Vec<&str> = paths.iter().map(String::as_str).collect();
    git::commit_paths(root, message, &pathspecs).map_err(|e| {
        StoreGitError::new(
            "commit_failed",
            format!("Could not commit in {at}: {}", e.user_detail()),
        )
        .with_fix(format!(
            "A commit hook or signing may have refused it; commit in a terminal at {at} to see git's full output."
        ))
    })?;
    status(root).ok_or_else(|| not_a_checkout(root))
}

/// Push the checked-out branch to its upstream.
///
/// Refused while the upstream has commits the branch lacks, as far as the
/// last fetch knows; otherwise git's own rejection is the error. A failed push
/// leaves every local commit in place. Returns the sync state after.
pub fn push(root: &Path) -> Result<StoreGitStatus, StoreGitError> {
    require_checkout(root)?;
    let before = status(root).ok_or_else(|| not_a_checkout(root))?;
    let at = display(root);
    let branch = on_branch(&before, &at)?;
    let Some(upstream) = before.upstream.clone() else {
        return Err(StoreGitError::new(
            "no_upstream",
            format!("`{branch}` in {at} has no upstream to push to."),
        )
        .with_fix(format!("Run `git push -u origin {branch}` in {at}.")));
    };
    if before.behind > 0 {
        return Err(StoreGitError::new(
            "behind_upstream",
            format!(
                "{upstream} has {} commit(s) `{branch}` lacks; pull before pushing.",
                before.behind
            ),
        )
        .with_fix(if before.ahead > 0 {
            format!("Rebase or merge in a terminal at {at}.")
        } else {
            "Pull, then push.".to_string()
        }));
    }
    if before.ahead == 0 {
        return Ok(before);
    }
    git::push_to_upstream(root).map_err(|e| {
        StoreGitError::new(
            "push_failed",
            format!(
                "Could not push `{branch}` to {upstream}: {}",
                e.user_detail()
            ),
        )
        .with_fix(format!(
            "The commit is kept locally. Check that git can push from a terminal at {at}."
        ))
    })?;
    status(root).ok_or_else(|| not_a_checkout(root))
}

fn on_branch(status: &StoreGitStatus, at: &str) -> Result<String, StoreGitError> {
    status.branch.clone().ok_or_else(|| {
        StoreGitError::new("detached_head", format!("{at} is not on a branch."))
            .with_fix(format!("Check out a branch in a terminal at {at}."))
    })
}

fn require_checkout(root: &Path) -> Result<(), StoreGitError> {
    if git::is_repository_at_root(root) {
        Ok(())
    } else {
        Err(not_a_checkout(root))
    }
}

fn not_a_checkout(root: &Path) -> StoreGitError {
    StoreGitError::new(
        "not_a_git_checkout",
        format!("{} is not the top of a git checkout.", display(root)),
    )
}

fn display(root: &Path) -> String {
    root.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::test_support::git_in;
    use std::path::PathBuf;

    fn git_out(repo: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .expect("git");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, content).expect("write");
    }

    /// A repo-local identity, so commits made by the functions under test
    /// don't depend on the machine's global config.
    fn identity(repo: &Path) {
        for (key, value) in [
            ("user.name", "test"),
            ("user.email", "test@test"),
            ("commit.gpgsign", "false"),
        ] {
            git_in(repo, &["config", key, value]);
        }
    }

    /// A bare `remote.git` holding `docs/a.md` and `docs/b.md`, and checkouts
    /// cloned from it on demand.
    struct Remote {
        dir: tempfile::TempDir,
    }

    impl Remote {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let seed = dir.path().join("seed");
            std::fs::create_dir_all(&seed).expect("mkdir");
            git_in(&seed, &["init", "-b", "main"]);
            identity(&seed);
            write(&seed.join("docs/a.md"), "a\n");
            write(&seed.join("docs/b.md"), "b\n");
            git_in(&seed, &["add", "."]);
            git_in(&seed, &["commit", "-m", "seed"]);
            git_in(dir.path(), &["clone", "--bare", "seed", "remote.git"]);
            Self { dir }
        }

        fn path(&self) -> PathBuf {
            self.dir.path().join("remote.git")
        }

        fn checkout(&self, name: &str) -> PathBuf {
            git_in(self.dir.path(), &["clone", "remote.git", name]);
            let path = self.dir.path().join(name);
            identity(&path);
            path
        }

        fn log(&self) -> String {
            git_out(&self.path(), &["log", "--format=%s", "main"])
        }
    }

    fn paths(status: &StoreGitStatus) -> Vec<(&str, StoreChangeKind, bool)> {
        status
            .changes
            .iter()
            .map(|c| (c.path.as_str(), c.kind, c.staged))
            .collect()
    }

    fn strings(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn status_lists_every_changed_file_and_what_happened_to_it() {
        let remote = Remote::new();
        let clone = remote.checkout("clone");
        let clean = status(&clone).expect("a checkout");
        assert!(!clean.dirty);
        assert!(clean.changes.is_empty());

        write(&clone.join("docs/a.md"), "edited\n");
        std::fs::remove_file(clone.join("docs/b.md")).expect("rm");
        write(&clone.join("docs/new/c.md"), "c\n");
        write(&clone.join("staged.md"), "s\n");
        git_in(&clone, &["add", "staged.md"]);

        let dirty = status(&clone).expect("status");
        assert!(dirty.dirty);
        assert_eq!(
            paths(&dirty),
            [
                ("docs/a.md", StoreChangeKind::Modified, false),
                ("docs/b.md", StoreChangeKind::Deleted, false),
                ("docs/new/c.md", StoreChangeKind::Untracked, false),
                ("staged.md", StoreChangeKind::Added, true),
            ]
        );
    }

    #[test]
    fn a_commit_takes_only_the_picked_files() {
        let remote = Remote::new();
        let clone = remote.checkout("clone");
        write(&clone.join("docs/a.md"), "edited\n");
        std::fs::remove_file(clone.join("docs/b.md")).expect("rm");
        write(&clone.join("docs/new/c.md"), "c\n");
        write(&clone.join("staged.md"), "s\n");
        git_in(&clone, &["add", "staged.md"]);

        let after = commit(
            &clone,
            &strings(&["docs/a.md", "docs/b.md", "docs/new/c.md"]),
            "  Update docs  ",
        )
        .expect("commit");
        assert_eq!((after.ahead, after.behind), (1, 0));
        assert_eq!(
            paths(&after),
            [("staged.md", StoreChangeKind::Added, true)],
            "what was not picked stays, and stays staged"
        );
        assert_eq!(
            git_out(&clone, &["show", "--name-only", "--format=%s", "HEAD"])
                .lines()
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>(),
            ["Update docs", "docs/a.md", "docs/b.md", "docs/new/c.md"]
        );
    }

    #[test]
    fn a_commit_refuses_anything_the_status_did_not_list() {
        let remote = Remote::new();
        let clone = remote.checkout("clone");
        write(&clone.join("docs/a.md"), "edited\n");
        write(&clone.join("docs/b.md"), "edited\n");

        let code = |paths: &[&str], message: &str| {
            commit(&clone, &strings(paths), message)
                .expect_err("refused")
                .code
        };
        assert_eq!(code(&["docs/a.md"], "  "), "commit_message_empty");
        assert_eq!(code(&[], "m"), "nothing_to_commit");
        // A pattern would match both edits; only listed paths are accepted.
        assert_eq!(code(&["docs/*.md"], "m"), "not_a_changed_file");
        assert_eq!(code(&["../remote.git/HEAD"], "m"), "not_a_changed_file");
        assert_eq!(code(&["docs/unchanged.md"], "m"), "not_a_changed_file");
        assert_eq!(
            status(&clone).expect("status").ahead,
            0,
            "nothing committed"
        );

        let plain = remote.dir.path().join("plain");
        std::fs::create_dir_all(&plain).expect("mkdir");
        assert_eq!(
            commit(&plain, &strings(&["x"]), "m")
                .expect_err("no repo")
                .code,
            "not_a_git_checkout"
        );
        assert_eq!(status(&plain), None);
    }

    #[test]
    fn push_sends_the_commit_and_a_rejected_push_keeps_it() {
        let remote = Remote::new();
        let clone = remote.checkout("clone");
        write(&clone.join("docs/a.md"), "edited\n");
        commit(&clone, &strings(&["docs/a.md"]), "Update docs/a.md").expect("commit");

        let pushed = push(&clone).expect("push");
        assert_eq!((pushed.ahead, pushed.behind), (0, 0));
        assert!(remote.log().starts_with("Update docs/a.md\n"));
        assert_eq!(push(&clone).expect("no-op").ahead, 0, "nothing to push");

        // Someone else pushes first; this checkout has not fetched, so only
        // git's rejection can say so.
        let other = remote.checkout("other");
        write(&other.join("docs/b.md"), "theirs\n");
        commit(&other, &strings(&["docs/b.md"]), "Theirs").expect("commit");
        push(&other).expect("push");

        write(&clone.join("docs/a.md"), "again\n");
        commit(&clone, &strings(&["docs/a.md"]), "Mine").expect("commit");
        let err = push(&clone).expect_err("rejected");
        assert_eq!(err.code, "push_failed");
        assert!(
            err.fix
                .as_deref()
                .is_some_and(|f| f.contains("kept locally"))
        );
        assert_eq!(status(&clone).expect("status").ahead, 1, "commit kept");
        assert!(remote.log().starts_with("Theirs\n"));

        // Once fetched, the view's reason and the refusal agree.
        fetch(&clone).expect("fetch");
        let diverged = status(&clone).expect("status");
        assert!(!diverged.can_push());
        assert_eq!(push(&clone).expect_err("behind").code, "behind_upstream");
    }

    #[test]
    fn pull_refuses_to_land_on_uncommitted_changes() {
        let remote = Remote::new();
        let clone = remote.checkout("clone");
        let other = remote.checkout("other");
        write(&other.join("docs/b.md"), "theirs\n");
        commit(&other, &strings(&["docs/b.md"]), "Theirs").expect("commit");
        push(&other).expect("push");

        write(&clone.join("docs/local.md"), "unsaved\n");
        assert_eq!(pull(&clone).expect_err("dirty").code, "uncommitted_changes");
        let st = status(&clone).expect("status");
        assert_eq!(st.behind, 1, "nothing pulled");
        assert!(!st.can_fast_forward());

        std::fs::remove_file(clone.join("docs/local.md")).expect("rm");
        assert_eq!(pull(&clone).expect("pull").behind, 0);
        assert_eq!(
            std::fs::read_to_string(clone.join("docs/b.md")).expect("read"),
            "theirs\n"
        );
    }

    #[test]
    fn porcelain_records_parse_into_kinds_and_the_list_is_capped() {
        let (changes, truncated) =
            parse_porcelain(b"UU both.md\0AM new.md\0 D gone.md\0?? odd name.md\0");
        assert!(!truncated);
        assert_eq!(
            changes
                .iter()
                .map(|c| (c.path.as_str(), c.kind, c.staged, c.unstaged))
                .collect::<Vec<_>>(),
            [
                ("both.md", StoreChangeKind::Conflicted, true, true),
                ("gone.md", StoreChangeKind::Deleted, false, true),
                ("new.md", StoreChangeKind::Added, true, true),
                ("odd name.md", StoreChangeKind::Untracked, false, true),
            ]
        );

        let many: Vec<u8> = (0..MAX_LISTED_CHANGES + 5)
            .flat_map(|i| format!("?? f{i}\0").into_bytes())
            .collect();
        let (changes, truncated) = parse_porcelain(&many);
        assert!(truncated);
        assert_eq!(changes.len(), MAX_LISTED_CHANGES);
    }
}
