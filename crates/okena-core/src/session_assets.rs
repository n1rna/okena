//! What a session produced, as okena shows it.
//!
//! Two sources feed the list. Agents register assets over MCP, and okena
//! detects the branches and pull requests of the worktrees linked to the
//! session's task from the git poll. Detected rows are derived here, each time
//! the list is read, rather than stored: a stored copy of a branch's push state
//! is stale the moment the agent pushes again, and a second registration of
//! the same PR would show twice.
//!
//! The one stored input is [`TrackedPullRequest`]: once a worktree is removed
//! there is no checkout left to poll, so an open PR it produced is remembered
//! on the session until it is merged or closed.

use crate::api::{ApiGitStatus, PrState};
use crate::harness::{AgentAsset, AgentAssetKind, TrackedPullRequest};

/// One row of a session's PRODUCED list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionAsset {
    pub kind: AgentAssetKind,
    /// The agent's title when it registered this asset, otherwise the branch.
    pub title: String,
    pub url: Option<String>,
    /// Repo it lives in. Always set on detected rows, so a task spanning
    /// several repos gives one labelled row per repo.
    pub project: Option<String>,
    pub branch: Option<String>,
    /// What okena sees of it. `None` for an asset only the agent knows about,
    /// or a checkout the poll has not reached yet.
    pub state: Option<DetectedState>,
    /// Uncommitted changes in the worktree behind this row.
    pub uncommitted: Option<LineChanges>,
    /// Whether the agent registered it, alone or merged into a detected row.
    pub registered: bool,
}

/// How a detected branch stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetectedState {
    /// No upstream: the work exists only on this machine.
    LocalOnly,
    /// On the remote. `unpushed` counts commits since the last push; `ahead`
    /// and `behind` are measured against the branch's review base.
    Pushed {
        unpushed: usize,
        ahead: Option<usize>,
        behind: Option<usize>,
    },
    PullRequest {
        number: u32,
        state: PrState,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineChanges {
    pub added: usize,
    pub removed: usize,
}

/// A worktree linked to the session's task, as the poll last saw it.
#[derive(Clone, Copy, Debug)]
pub struct LinkedCheckout<'a> {
    /// Repo label: the worktree's parent project.
    pub project: &'a str,
    /// Branch the worktree was created on, used until the poll reports one.
    pub branch: Option<&'a str>,
    pub git: Option<&'a ApiGitStatus>,
}

/// Build the PRODUCED list: detected checkouts first, then the open PRs of
/// removed worktrees, then whatever the agent registered that matched neither.
pub fn derive_session_assets(
    registered: &[AgentAsset],
    checkouts: &[LinkedCheckout<'_>],
    tracked: &[TrackedPullRequest],
) -> Vec<SessionAsset> {
    let mut rows: Vec<SessionAsset> = checkouts.iter().filter_map(checkout_row).collect();

    for pr in tracked {
        // The branch was checked out again: the live row already covers it.
        if rows
            .iter()
            .any(|r| same_url(r.url.as_deref(), Some(&pr.url)))
        {
            continue;
        }
        rows.push(SessionAsset {
            kind: AgentAssetKind::PullRequest,
            title: pr
                .branch
                .clone()
                .unwrap_or_else(|| format!("#{}", pr.number)),
            url: Some(pr.url.clone()),
            project: Some(pr.project.clone()),
            branch: pr.branch.clone(),
            state: Some(DetectedState::PullRequest {
                number: pr.number,
                state: pr.state.clone(),
            }),
            uncommitted: None,
            registered: false,
        });
    }

    let detected = rows.len();
    for asset in registered {
        let matched = matching_row(&rows[..detected], asset);
        match matched {
            Some(i) => {
                let row = &mut rows[i];
                // The agent's title is kept; okena's state is shown. A second
                // registration of the same thing does not retitle it again.
                if !row.registered {
                    row.title = asset.title.clone();
                    row.registered = true;
                }
            }
            None => rows.push(SessionAsset {
                kind: asset.kind.clone(),
                title: asset.title.clone(),
                url: asset.url.clone(),
                project: asset.project.clone(),
                branch: asset.branch.clone(),
                state: None,
                uncommitted: None,
                registered: true,
            }),
        }
    }
    rows
}

fn checkout_row(c: &LinkedCheckout<'_>) -> Option<SessionAsset> {
    let branch = c
        .git
        .and_then(|g| g.branch.as_deref())
        .or(c.branch)
        .filter(|b| !b.is_empty())?
        .to_string();
    let pr = c.git.and_then(|g| g.pr_info.as_ref());
    let state = c.git.map(|g| match (pr, g.unpushed) {
        (Some(pr), _) => DetectedState::PullRequest {
            number: pr.number,
            state: pr.state.clone(),
        },
        (None, Some(unpushed)) => DetectedState::Pushed {
            unpushed,
            ahead: g.ahead,
            behind: g.behind,
        },
        (None, None) => DetectedState::LocalOnly,
    });
    let uncommitted = c
        .git
        .filter(|g| g.lines_added > 0 || g.lines_removed > 0)
        .map(|g| LineChanges {
            added: g.lines_added,
            removed: g.lines_removed,
        });
    Some(SessionAsset {
        kind: if pr.is_some() {
            AgentAssetKind::PullRequest
        } else {
            AgentAssetKind::Branch
        },
        title: branch.clone(),
        url: pr.map(|pr| pr.url.clone()),
        project: Some(c.project.to_string()),
        branch: Some(branch),
        state,
        uncommitted,
        registered: false,
    })
}

/// The detected row a registered asset describes, if any.
///
/// By URL first. Otherwise by branch and project: the agent names the branch
/// in `branch`, or — for a branch asset — as its title. With no project, the
/// branch has to be unambiguous, since a task spanning several repos uses the
/// same branch name in each of them.
fn matching_row(rows: &[SessionAsset], asset: &AgentAsset) -> Option<usize> {
    if asset.url.is_some()
        && let Some(i) = rows
            .iter()
            .position(|r| same_url(r.url.as_deref(), asset.url.as_deref()))
    {
        return Some(i);
    }

    let branch = asset.branch.as_deref().or(match asset.kind {
        AgentAssetKind::Branch => Some(asset.title.as_str()),
        _ => None,
    })?;
    let branch = branch.trim();
    let on_branch = |r: &&SessionAsset| r.branch.as_deref() == Some(branch);
    match asset.project.as_deref().map(str::trim) {
        Some(project) => rows.iter().position(|r| {
            on_branch(&r)
                && r.project
                    .as_deref()
                    .is_some_and(|p| p.eq_ignore_ascii_case(project))
        }),
        None => {
            let mut candidates = rows.iter().enumerate().filter(|(_, r)| on_branch(r));
            match (candidates.next(), candidates.next()) {
                (Some((i, _)), None) => Some(i),
                _ => None,
            }
        }
    }
}

fn same_url(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            let norm = |u: &str| u.trim().trim_end_matches('/').to_ascii_lowercase();
            norm(a) == norm(b)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::PrInfo;

    fn git(unpushed: Option<usize>, pr: Option<(u32, PrState)>) -> ApiGitStatus {
        ApiGitStatus {
            branch: Some("feat/x".into()),
            unpushed,
            ahead: Some(2),
            behind: Some(1),
            pr_info: pr.map(|(number, state)| PrInfo {
                url: format!("https://github.com/o/r/pull/{number}"),
                state,
                number,
                base: None,
            }),
            ..ApiGitStatus::default()
        }
    }

    fn checkout<'a>(project: &'a str, git: Option<&'a ApiGitStatus>) -> LinkedCheckout<'a> {
        LinkedCheckout {
            project,
            branch: Some("feat/x"),
            git,
        }
    }

    fn registered(kind: AgentAssetKind, title: &str) -> AgentAsset {
        AgentAsset {
            kind,
            title: title.into(),
            url: None,
            project: None,
            branch: None,
            created_at: 0,
        }
    }

    #[test]
    fn a_branch_never_pushed_is_local_only() {
        let g = git(None, None);
        let rows = derive_session_assets(&[], &[checkout("okena", Some(&g))], &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, AgentAssetKind::Branch);
        assert_eq!(rows[0].state, Some(DetectedState::LocalOnly));
        assert_eq!(rows[0].project.as_deref(), Some("okena"));
        assert!(!rows[0].registered);
    }

    #[test]
    fn a_pushed_branch_carries_its_counts() {
        let g = git(Some(0), None);
        let rows = derive_session_assets(&[], &[checkout("okena", Some(&g))], &[]);
        assert_eq!(
            rows[0].state,
            Some(DetectedState::Pushed {
                unpushed: 0,
                ahead: Some(2),
                behind: Some(1)
            })
        );
    }

    #[test]
    fn a_branch_with_a_pr_becomes_a_pr_row() {
        let g = git(Some(0), Some((7, PrState::Draft)));
        let rows = derive_session_assets(&[], &[checkout("okena", Some(&g))], &[]);
        assert_eq!(rows[0].kind, AgentAssetKind::PullRequest);
        assert_eq!(
            rows[0].url.as_deref(),
            Some("https://github.com/o/r/pull/7")
        );
        assert_eq!(
            rows[0].state,
            Some(DetectedState::PullRequest {
                number: 7,
                state: PrState::Draft
            })
        );
    }

    #[test]
    fn uncommitted_changes_show_on_the_row() {
        let mut g = git(None, None);
        g.lines_added = 4;
        let rows = derive_session_assets(&[], &[checkout("okena", Some(&g))], &[]);
        assert_eq!(
            rows[0].uncommitted,
            Some(LineChanges {
                added: 4,
                removed: 0
            })
        );
    }

    #[test]
    fn several_repos_give_one_labelled_row_each() {
        let (a, b) = (git(None, None), git(Some(1), None));
        let rows = derive_session_assets(
            &[],
            &[checkout("okena", Some(&a)), checkout("web", Some(&b))],
            &[],
        );
        let projects: Vec<_> = rows.iter().map(|r| r.project.as_deref()).collect();
        assert_eq!(projects, [Some("okena"), Some("web")]);
    }

    #[test]
    fn a_checkout_the_poll_has_not_reached_shows_its_branch_without_state() {
        let rows = derive_session_assets(&[], &[checkout("okena", None)], &[]);
        assert_eq!(rows[0].branch.as_deref(), Some("feat/x"));
        assert_eq!(rows[0].state, None);
    }

    #[test]
    fn a_registered_pr_merges_by_url_and_keeps_the_agents_title() {
        let g = git(Some(0), Some((7, PrState::Open)));
        let mut pr = registered(AgentAssetKind::PullRequest, "Detect session assets");
        pr.url = Some("https://github.com/o/r/pull/7/".into());
        let rows = derive_session_assets(&[pr.clone(), pr], &[checkout("okena", Some(&g))], &[]);
        assert_eq!(rows.len(), 1, "one row, however often it was registered");
        assert_eq!(rows[0].title, "Detect session assets");
        assert!(rows[0].registered);
        assert!(matches!(
            rows[0].state,
            Some(DetectedState::PullRequest { number: 7, .. })
        ));
    }

    #[test]
    fn a_registered_branch_merges_by_branch_and_project() {
        let (a, b) = (git(None, None), git(None, None));
        let mut asset = registered(AgentAssetKind::Other, "web side");
        asset.branch = Some("feat/x".into());
        asset.project = Some("WEB".into());
        let rows = derive_session_assets(
            &[asset],
            &[checkout("okena", Some(&a)), checkout("web", Some(&b))],
            &[],
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].title, "web side");
        assert_eq!(rows[0].title, "feat/x");
    }

    #[test]
    fn a_branch_named_only_by_title_merges_when_unambiguous() {
        let g = git(None, None);
        let rows = derive_session_assets(
            &[registered(AgentAssetKind::Branch, "feat/x")],
            &[checkout("okena", Some(&g))],
            &[],
        );
        assert_eq!(rows.len(), 1);
        assert!(rows[0].registered);
    }

    #[test]
    fn an_ambiguous_branch_without_a_project_stays_separate() {
        let (a, b) = (git(None, None), git(None, None));
        let rows = derive_session_assets(
            &[registered(AgentAssetKind::Branch, "feat/x")],
            &[checkout("okena", Some(&a)), checkout("web", Some(&b))],
            &[],
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].state, None);
    }

    #[test]
    fn unmatched_registrations_are_listed_after_detected_rows() {
        let g = git(None, None);
        let mut doc = registered(AgentAssetKind::Document, "Design notes");
        doc.url = Some("https://example.com/doc".into());
        let rows = derive_session_assets(&[doc], &[checkout("okena", Some(&g))], &[]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].kind, AgentAssetKind::Document);
        assert_eq!(rows[1].state, None);
    }

    fn tracked(number: u32) -> TrackedPullRequest {
        TrackedPullRequest {
            project: "okena".into(),
            repo_path: "/p/okena".into(),
            branch: Some("feat/x".into()),
            number,
            url: format!("https://github.com/o/r/pull/{number}"),
            state: PrState::Open,
        }
    }

    #[test]
    fn the_pr_of_a_removed_worktree_stays_listed() {
        let mut pr = registered(AgentAssetKind::PullRequest, "Agent title");
        pr.url = Some("https://github.com/o/r/pull/9".into());
        let rows = derive_session_assets(&[pr], &[], &[tracked(9)]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Agent title");
        assert_eq!(rows[0].project.as_deref(), Some("okena"));
    }

    #[test]
    fn a_tracked_pr_checked_out_again_is_not_listed_twice() {
        let g = git(Some(0), Some((9, PrState::Open)));
        let rows = derive_session_assets(&[], &[checkout("okena", Some(&g))], &[tracked(9)]);
        assert_eq!(rows.len(), 1);
    }
}
