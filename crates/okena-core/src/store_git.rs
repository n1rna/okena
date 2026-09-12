//! A store checkout's git state, as the harness shows it.
//!
//! Shared by OpenSpec stores and knowledge stores (ADR-0004): both are git
//! checkouts okena fetches, fast-forwards, commits and pushes on a person's
//! behalf, so both report the same shape. The git itself lives in
//! `okena_git::store`.

use serde::{Deserialize, Serialize};

/// How many changed files a status lists. Past this the list is cut and
/// [`StoreGitStatus::changes_truncated`] is set: a store with a forgotten
/// build folder must not stall every listing with a hundred thousand paths.
pub const MAX_LISTED_CHANGES: usize = 1_000;

/// What happened to a changed file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreChangeKind {
    Added,
    Modified,
    Deleted,
    /// Not tracked by git yet.
    Untracked,
    /// Unmerged: a merge or rebase stopped on it. Committing it from okena
    /// would mark it resolved, so it is refused.
    Conflicted,
}

/// One file with uncommitted changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreChange {
    /// Relative to the checkout root, with `/` separators, as git prints it.
    pub path: String,
    pub kind: StoreChangeKind,
    /// Some of the change is in the index.
    #[serde(default)]
    pub staged: bool,
    /// Some of the change is only in the working tree.
    #[serde(default)]
    pub unstaged: bool,
}

/// A checkout's sync state, from local git only — nothing here touches the
/// network, so it is only as fresh as the last fetch ([`Self::fetched_at`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreGitStatus {
    /// Checked-out branch; `None` when HEAD is detached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The branch's upstream, e.g. `origin/main`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
    /// Uncommitted changes, staged or not, including untracked files. Also set
    /// when git could not say: the flag only prompts a look.
    #[serde(default)]
    pub dirty: bool,
    /// The changed files, sorted by path. Empty with `dirty` set when git's
    /// status could not be read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<StoreChange>,
    /// More than [`MAX_LISTED_CHANGES`] files changed; `changes` holds the
    /// first of them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub changes_truncated: bool,
    /// When the checkout last fetched, in Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<u64>,
}

impl StoreGitStatus {
    /// Whether a fast-forward pull has anything to do and can do it. Pull
    /// refuses anything else, so the view disables it rather than offering a
    /// button that can only fail; [`Self::pull_blocker`] says why.
    pub fn can_fast_forward(&self) -> bool {
        self.pull_blocker().is_none()
    }

    /// Why a pull is unavailable, in words for the button's neighbour. `None`
    /// when a fast-forward can run.
    ///
    /// Uncommitted changes block it even where git would let the merge through:
    /// a pull that lands on top of someone's unsaved edit, or half-fails on one,
    /// is the surprise this refuses up front.
    pub fn pull_blocker(&self) -> Option<String> {
        if self.branch.is_none() {
            return Some("HEAD is detached; check out a branch to pull.".into());
        }
        let Some(upstream) = &self.upstream else {
            return Some("The branch has no upstream to pull from.".into());
        };
        if self.dirty {
            return Some(format!(
                "Commit the uncommitted {} first; okena only fast-forwards a clean checkout.",
                plural(self.changes.len(), "change", "changes")
            ));
        }
        if self.ahead > 0 && self.behind > 0 {
            return Some(format!(
                "{} ahead of {upstream} and {} behind; rebase or merge in a terminal.",
                plural(self.ahead as usize, "commit", "commits"),
                self.behind
            ));
        }
        if self.behind == 0 {
            return Some(if self.ahead > 0 {
                format!(
                    "Nothing to pull; {} to push.",
                    plural(self.ahead as usize, "commit", "commits")
                )
            } else {
                "Nothing to pull.".into()
            });
        }
        None
    }

    /// Why a push is unavailable. `None` when there are commits to push and
    /// the upstream has none we lack.
    pub fn push_blocker(&self) -> Option<String> {
        if self.branch.is_none() {
            return Some("HEAD is detached; check out a branch to push.".into());
        }
        let Some(upstream) = &self.upstream else {
            return Some("The branch has no upstream to push to.".into());
        };
        if self.behind > 0 {
            return Some(format!(
                "{upstream} has {} this branch lacks; pull first.",
                plural(self.behind as usize, "commit", "commits")
            ));
        }
        if self.ahead == 0 {
            return Some("Nothing to push.".into());
        }
        None
    }

    pub fn can_push(&self) -> bool {
        self.push_blocker().is_none()
    }
}

/// The message a commit of `changes` is offered with: the file when there is
/// one, else a count.
pub fn default_commit_message(changes: &[StoreChange]) -> String {
    match changes {
        [] => "Update store".to_string(),
        [one] => match one.kind {
            StoreChangeKind::Added | StoreChangeKind::Untracked => format!("Add {}", one.path),
            StoreChangeKind::Deleted => format!("Remove {}", one.path),
            StoreChangeKind::Modified | StoreChangeKind::Conflicted => {
                format!("Update {}", one.path)
            }
        },
        many => format!("Update {} files", many.len()),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracked(ahead: u32, behind: u32) -> StoreGitStatus {
        StoreGitStatus {
            branch: Some("main".into()),
            upstream: Some("origin/main".into()),
            ahead,
            behind,
            ..Default::default()
        }
    }

    fn change(path: &str, kind: StoreChangeKind) -> StoreChange {
        StoreChange {
            path: path.into(),
            kind,
            staged: false,
            unstaged: true,
        }
    }

    #[test]
    fn fast_forward_needs_an_upstream_a_clean_tree_and_only_incoming_commits() {
        assert!(tracked(0, 3).can_fast_forward());
        assert_eq!(
            tracked(0, 0).pull_blocker().as_deref(),
            Some("Nothing to pull.")
        );
        assert_eq!(
            tracked(2, 0).pull_blocker().as_deref(),
            Some("Nothing to pull; 2 commits to push.")
        );
        assert!(
            tracked(1, 3)
                .pull_blocker()
                .is_some_and(|w| w.contains("rebase or merge"))
        );
        let untracked = StoreGitStatus {
            upstream: None,
            ..tracked(0, 3)
        };
        assert!(!untracked.can_fast_forward());
        let detached = StoreGitStatus {
            branch: None,
            ..tracked(0, 3)
        };
        assert!(
            detached
                .pull_blocker()
                .is_some_and(|w| w.contains("detached"))
        );
    }

    #[test]
    fn uncommitted_changes_block_a_pull_and_say_how_many() {
        let dirty = StoreGitStatus {
            dirty: true,
            changes: vec![change("docs/a.md", StoreChangeKind::Modified)],
            ..tracked(0, 3)
        };
        assert_eq!(
            dirty.pull_blocker().as_deref(),
            Some(
                "Commit the uncommitted 1 change first; okena only fast-forwards a clean checkout."
            )
        );
    }

    #[test]
    fn push_needs_outgoing_commits_and_nothing_incoming() {
        assert!(tracked(1, 0).can_push());
        assert_eq!(
            tracked(0, 0).push_blocker().as_deref(),
            Some("Nothing to push.")
        );
        assert!(
            tracked(1, 2)
                .push_blocker()
                .is_some_and(|w| w.contains("pull first"))
        );
        let untracked = StoreGitStatus {
            upstream: None,
            ..tracked(1, 0)
        };
        assert!(!untracked.can_push());
    }

    #[test]
    fn the_default_message_names_one_file_and_counts_many() {
        assert_eq!(
            default_commit_message(&[change("docs/a.md", StoreChangeKind::Modified)]),
            "Update docs/a.md"
        );
        assert_eq!(
            default_commit_message(&[change("docs/b.md", StoreChangeKind::Untracked)]),
            "Add docs/b.md"
        );
        assert_eq!(
            default_commit_message(&[change("docs/c.md", StoreChangeKind::Deleted)]),
            "Remove docs/c.md"
        );
        assert_eq!(
            default_commit_message(&[
                change("a", StoreChangeKind::Modified),
                change("b", StoreChangeKind::Added),
            ]),
            "Update 2 files"
        );
    }

    #[test]
    fn a_status_from_before_changes_were_listed_still_decodes() {
        let old = r#"{"branch":"main","upstream":"origin/main","ahead":0,"behind":1,"dirty":true}"#;
        let status: StoreGitStatus = serde_json::from_str(old).expect("decode");
        assert!(status.dirty);
        assert!(status.changes.is_empty());
        assert!(!status.changes_truncated);
    }
}
