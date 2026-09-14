//! Branches an agent session pushed, as its own hooks report them.
//!
//! Detection by task alone (`git_poll::session_links`) misses every branch an
//! agent makes outside a Start work worktree: a `git worktree add` of its own,
//! or a branch in the session's own checkout. The agent's hook tells the
//! daemon where a `git push` or `gh pr create` ran, and a turn's end is a
//! chance to look at the session's own checkout; either way the branch is
//! recorded on the session, so the git poller looks its PR up — now, and on
//! its cadence after — and the card outlives the checkout.

use std::path::Path;

use okena_core::harness::PushedBranch;
use okena_git as git;

/// The branch checked out at `dir`, as the session's to record: the main
/// checkout of its repository, its name and the branch.
///
/// `None` for no repository, a detached HEAD, or the default branch — a push
/// to `main` is not work of the session's own. A branch seen only at a turn's
/// end (`explicit` false) must also have an upstream: nothing says it was
/// pushed otherwise, and a local-only branch has no PR to find.
pub fn pushed_branch_at(dir: &Path, explicit: bool) -> Option<PushedBranch> {
    let branch = git::get_current_branch(dir)?;
    let default = git::get_default_branch(dir);
    let is_default = match default.as_deref() {
        Some(default) => branch == default,
        None => matches!(branch.as_str(), "main" | "master"),
    };
    if is_default || branch.trim().is_empty() {
        return None;
    }
    if !explicit && git::repository::status::get_pushed_sha(dir).is_none() {
        return None;
    }
    let repo_path = main_checkout(dir)?;
    let project = Path::new(&repo_path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| repo_path.clone());
    Some(PushedBranch {
        project,
        repo_path,
        branch,
    })
}

/// The main checkout of the repository at `dir`: the directory holding its
/// common `.git`, so a linked worktree resolves to the checkout it came from,
/// which is still there once the worktree is removed.
fn main_checkout(dir: &Path) -> Option<String> {
    let common = git::get_repo_common_dir(dir)?;
    let root = if common.file_name().is_some_and(|name| name == ".git") {
        common.parent()?.to_path_buf()
    } else {
        git::get_repo_root(dir)?
    };
    Some(root.to_string_lossy().into_owned())
}

/// Record `pushed` on session `session_id`, once. Returns whether the session
/// changed.
pub fn record_pushed_branch(
    projects: &mut [okena_state::ProjectData],
    session_id: &str,
    pushed: PushedBranch,
) -> bool {
    let Some(session) = projects.iter_mut().find(|p| p.id == session_id) else {
        return false;
    };
    let agent = session.agent.get_or_insert_with(Default::default);
    let known = agent
        .pushed_branches
        .iter()
        .any(|b| b.repo_path == pushed.repo_path && b.branch == pushed.branch);
    if known {
        return false;
    }
    agent.pushed_branches.push(pushed);
    true
}

#[cfg(test)]
mod tests {
    use super::{pushed_branch_at, record_pushed_branch};
    use okena_core::harness::PushedBranch;
    use std::path::Path;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repo `app` on `main` with one commit, inside a temp dir.
    fn repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().canonicalize().expect("canonical").join("app");
        std::fs::create_dir_all(&root).expect("mkdir");
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("f"), "x").expect("write");
        git(&root, &["add", "."]);
        git(&root, &["-c", "commit.gpgsign=false", "commit", "-qm", "init"]);
        (tmp, root)
    }

    #[test]
    fn a_push_from_an_agents_own_worktree_is_recorded_against_the_main_checkout() {
        let (tmp, root) = repo();
        let wt = tmp.path().canonicalize().unwrap().join("app-wt");
        git(
            &root,
            &["worktree", "add", "-q", "-b", "feat/x", wt.to_str().unwrap()],
        );

        let pushed = pushed_branch_at(&wt, true).expect("a feature branch");
        assert_eq!(pushed.branch, "feat/x");
        assert_eq!(Path::new(&pushed.repo_path), root, "the main checkout");
        assert_eq!(pushed.project, "app");
    }

    #[test]
    fn a_push_on_the_default_branch_is_not_the_sessions_work() {
        let (_tmp, root) = repo();
        assert_eq!(pushed_branch_at(&root, true), None);
        assert_eq!(pushed_branch_at(Path::new("/"), true), None, "no repository");
    }

    #[test]
    fn a_turn_end_records_only_a_branch_with_an_upstream() {
        let (_tmp, root) = repo();
        // Configured before the repository is first read: okena-git caches a
        // repository's handle, config included, per path.
        git(&root, &["branch", "feat/local"]);
        git(&root, &["branch", "feat/y"]);
        // `feat/y` as `git push -u origin feat/y` leaves it.
        git(&root, &["update-ref", "refs/remotes/origin/feat/y", "HEAD"]);
        git(
            &root,
            &["config", "remote.origin.url", "git@github.com:o/app.git"],
        );
        git(
            &root,
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/remotes/origin/*",
            ],
        );
        git(&root, &["config", "branch.feat/y.remote", "origin"]);
        git(&root, &["config", "branch.feat/y.merge", "refs/heads/feat/y"]);

        git(&root, &["checkout", "-q", "feat/local"]);
        assert_eq!(
            pushed_branch_at(&root, false),
            None,
            "local only: nothing to look up"
        );
        assert_eq!(
            pushed_branch_at(&root, true).map(|b| b.branch).as_deref(),
            Some("feat/local"),
            "a push the hook saw needs no upstream to be recorded"
        );

        git(&root, &["checkout", "-q", "feat/y"]);
        let pushed = pushed_branch_at(&root, false).expect("pushed, with an upstream");
        assert_eq!(pushed.branch, "feat/y");
    }

    #[test]
    fn a_branch_is_recorded_once_on_its_session() {
        let mut projects: Vec<okena_state::ProjectData> = vec![
            serde_json::from_value(serde_json::json!({ "id": "s1", "name": "s", "path": "/w" }))
                .unwrap(),
        ];
        let branch = PushedBranch {
            project: "app".into(),
            repo_path: "/w/app".into(),
            branch: "feat/x".into(),
        };
        assert!(record_pushed_branch(&mut projects, "s1", branch.clone()));
        assert!(!record_pushed_branch(&mut projects, "s1", branch.clone()));
        assert!(!record_pushed_branch(&mut projects, "gone", branch));
        let agent = projects[0].agent.as_ref().unwrap();
        assert_eq!(agent.pushed_branches.len(), 1);
    }
}
