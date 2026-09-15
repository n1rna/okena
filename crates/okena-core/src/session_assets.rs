//! What a session produced, as okena shows it.
//!
//! Two sources feed the list. Agents register assets over MCP, and okena
//! detects the pull requests of the worktrees linked to the session's task from
//! the git poll. A linked worktree without a PR is not listed: it is where the
//! work happens, shown under WORKTREES, and becomes something the session
//! produced only once a PR is opened on it. Detected rows are derived here, each time
//! the list is read, rather than stored: a stored copy of a branch's push state
//! is stale the moment the agent pushes again, and a second registration of
//! the same PR would show twice. Matching happens only here, never when an
//! asset is saved.
//!
//! The one stored input is [`TrackedPullRequest`]: once a worktree is removed
//! there is no checkout left to poll, so a PR it produced is remembered on the
//! session. It stays listed whatever its state — an open one refreshed until it
//! closes, a merged or closed one marked so, with its last readiness — for as
//! long as the session exists. One card per branch: a checkout that is on the
//! branch again shows its own PR instead, and of several PRs from one branch
//! only the newest is listed.

use crate::api::{ApiGitStatus, CiCheckSummary, PrInfo, PrState};
use crate::harness::{AgentAsset, AgentAssetKind, PushedBranch, TrackedPullRequest};
use crate::tasks::TaskRef;

/// One row of a session's PRODUCED list.
///
/// `Default` so a caller building one names only the fields it sets, and a new
/// field does not break every literal.
#[derive(Clone, Debug, Default, PartialEq)]
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
    /// The pull request behind a PR row, with what okena knows of its
    /// mergeability and reviews.
    pub pr: Option<PrInfo>,
    /// The CI rollup of the checkout behind this row.
    pub ci: Option<CiCheckSummary>,
    /// The task this row is, when okena recorded it as one — a task an agent
    /// filed through okena's MCP. Carried through for its caption, and the
    /// key a hand-registered ticket link is matched on.
    pub task: Option<TaskRef>,
    /// Whether the agent registered it, alone or merged into a detected row.
    pub registered: bool,
}

/// How a detected branch stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetectedState {
    /// No upstream: the work exists only on this machine.
    LocalOnly,
    /// On the remote.
    Pushed {
        /// Commits not yet on the branch's own remote ref, `origin/<branch>`.
        unpushed: usize,
        /// Commits ahead of the **base branch** (`origin/<default>`, e.g.
        /// `main`) — not of the branch's own upstream, which `unpushed`
        /// already covers. Decided with the user: this says how much the
        /// branch adds and how stale it is. Do not change it to upstream.
        ahead: Option<usize>,
        /// Commits behind the base branch, likewise not the upstream.
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

/// Build the PRODUCED list: the PRs of linked checkouts first, then the PRs of
/// removed worktrees — open, merged or closed — then the branches the agent
/// pushed that neither covers and no linked checkout is on, then whatever the
/// agent registered that matched none. A registered branch that matches a PR
/// row is that row, never a second one.
pub fn derive_session_assets(
    registered: &[AgentAsset],
    checkouts: &[LinkedCheckout<'_>],
    tracked: &[TrackedPullRequest],
    pushed: &[PushedBranch],
) -> Vec<SessionAsset> {
    let mut rows: Vec<SessionAsset> = checkouts.iter().filter_map(checkout_row).collect();

    for pr in tracked {
        // One card per branch. A live checkout's row already covers its own
        // PR, or its branch's newer one; and of several tracked PRs from the
        // same branch, only the newest is listed.
        let live_covers = rows.iter().any(|r| {
            url_is(r.url.as_deref(), &pr.url)
                || (r.pr.is_some() && same_branch(r, &pr.project, pr.branch.as_deref()))
        });
        let newer_tracked = tracked.iter().any(|other| {
            other.number > pr.number
                && other.project == pr.project
                && other.branch.is_some()
                && other.branch == pr.branch
        });
        if live_covers || newer_tracked {
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
            // The worktree is gone, so there are no checks to read — only the
            // PR itself, with the mergeability read alongside it.
            pr: Some(PrInfo {
                url: pr.url.clone(),
                state: pr.state.clone(),
                number: pr.number,
                base: None,
                readiness: pr.readiness.clone(),
                readiness_unavailable: pr.readiness_unavailable,
            }),
            ci: None,
            task: None,
            registered: false,
        });
    }

    // A branch the agent pushed from a checkout okena does not track: a card
    // of its own until its PR is found, when the PR's card takes its place.
    // A branch a linked worktree is on is that worktree's, shown under
    // WORKTREES until it has a PR.
    for branch in pushed {
        let linked = checkouts.iter().any(|c| {
            checkout_branch(c) == Some(branch.branch.as_str())
                && c.project.eq_ignore_ascii_case(&branch.project)
        });
        if linked
            || rows
                .iter()
                .any(|r| same_branch(r, &branch.project, Some(&branch.branch)))
        {
            continue;
        }
        rows.push(SessionAsset {
            kind: AgentAssetKind::Branch,
            title: branch.branch.clone(),
            project: Some(branch.project.clone()),
            branch: Some(branch.branch.clone()),
            ..SessionAsset::default()
        });
    }

    let detected = rows.len();
    for asset in registered {
        // A PR it names — open, merged or closed — is already a card above:
        // the registration only lends it the agent's title.
        if let Some(i) = detected_match(&rows[..detected], asset) {
            let row = &mut rows[i];
            // The agent's title is kept; okena's state is shown. A second
            // registration of the same thing does not retitle it again.
            if !row.registered {
                row.title = asset.title.clone();
                row.registered = true;
            }
            continue;
        }
        // Registered twice and detected by neither: still one row. The row
        // that carries the task wins, since it knows which task it is, and
        // the agent's title wins over the ticket's own, which is all okena's
        // record of a filed task has.
        if let Some(row) = rows[detected..].iter_mut().find(|row| matches(asset, row)) {
            match (&row.task, &asset.task) {
                (None, Some(task)) => {
                    row.kind = asset.kind.clone();
                    row.task = Some(task.clone());
                }
                (Some(task), None) if row.title == task.title => {
                    row.title = asset.title.clone();
                }
                _ => {}
            }
            continue;
        }
        rows.push(SessionAsset {
            kind: asset.kind.clone(),
            title: asset.title.clone(),
            url: asset.url.clone(),
            project: asset.project.clone(),
            branch: asset.branch.clone(),
            task: asset.task.clone(),
            registered: true,
            ..SessionAsset::default()
        });
    }
    rows
}

/// The branch a linked checkout is on: as the poll last saw it, or the one it
/// was created on.
fn checkout_branch<'a>(c: &LinkedCheckout<'a>) -> Option<&'a str> {
    c.git
        .and_then(|g| g.branch.as_deref())
        .or(c.branch)
        .filter(|b| !b.is_empty())
}

/// A linked checkout's row: its pull request, once it has one. A checkout
/// without one — local only or pushed — gives none; it shows only under
/// WORKTREES.
fn checkout_row(c: &LinkedCheckout<'_>) -> Option<SessionAsset> {
    let git = c.git?;
    let pr = git.pr_info.as_ref()?;
    let branch = checkout_branch(c)?.to_string();
    let uncommitted = (git.lines_added > 0 || git.lines_removed > 0).then_some(LineChanges {
        added: git.lines_added,
        removed: git.lines_removed,
    });
    Some(SessionAsset {
        kind: AgentAssetKind::PullRequest,
        title: branch.clone(),
        url: Some(pr.url.clone()),
        project: Some(c.project.to_string()),
        branch: Some(branch),
        state: Some(DetectedState::PullRequest {
            number: pr.number,
            state: pr.state.clone(),
        }),
        uncommitted,
        pr: Some(pr.clone()),
        ci: git.ci_checks.clone(),
        task: None,
        registered: false,
    })
}

/// Whether a registered asset and a row describe the same thing.
///
/// The single place a match key lives, used both against detected rows and to
/// collapse repeated registrations: the same URL, or the same task. Two
/// recorded tasks are the same when their ids are. Otherwise a task is named by
/// its [`TaskKey`] — from okena's record of it, or parsed from a ticket URL —
/// so a hand-registered `…/issue/QBL-375` is the task okena recorded at
/// `…/issue/QBL-375/its-title-slug`, and a link copied before the ticket was
/// renamed still matches. The key carries where the task lives, so `#42` in
/// one Azure DevOps organization is not `#42` in another.
fn matches(asset: &AgentAsset, row: &SessionAsset) -> bool {
    if matches!(
        (asset.url.as_deref(), row.url.as_deref()),
        (Some(a), Some(b)) if same_url(a, b)
    ) {
        return true;
    }
    if let (Some(a), Some(b)) = (&asset.task, &row.task) {
        return a.id == b.id;
    }
    let key = |task: Option<&TaskRef>, url: Option<&str>| match task {
        Some(task) => recorded_task_key(task),
        None => url.and_then(task_key_from_url),
    };
    matches!(
        (
            key(asset.task.as_ref(), asset.url.as_deref()),
            key(row.task.as_ref(), row.url.as_deref()),
        ),
        (Some(a), Some(b)) if a == b
    )
}

/// A task as a ticket URL names it: the provider, where on that provider it
/// lives, and its key there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskKey {
    /// Provider id, as in [`TaskId::provider`](crate::tasks::TaskId).
    pub provider: &'static str,
    /// Lowercased: Linear's workspace slug, or the Azure DevOps organization.
    /// Not the Azure DevOps project — work item ids are unique across an
    /// organization, and okena links a work item with no project without one.
    pub scope: String,
    /// `QBL-375`, or `#42`.
    pub key: String,
}

impl std::fmt::Display for TaskKey {
    /// `linear:qblok/QBL-375`, `azure_devops:contoso#42`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sep = if self.key.starts_with('#') { "" } else { "/" };
        write!(f, "{}:{}{sep}{}", self.provider, self.scope, self.key)
    }
}

/// The key of a task okena recorded: its current key, where its URL says it
/// lives. `None` when the URL is not one of its provider's.
fn recorded_task_key(task: &TaskRef) -> Option<TaskKey> {
    let from_url = task_key_from_url(&task.url)?;
    (from_url.provider == task.id.provider).then(|| TaskKey {
        key: task.display_key.to_ascii_uppercase(),
        ..from_url
    })
}

/// The task a ticket URL names: `QBL-375` in its workspace from a Linear issue
/// URL, with or without its title slug, or `#42` in its organization from an
/// Azure DevOps work item URL. `None` for anything else, pull requests
/// included.
pub fn task_key_from_url(url: &str) -> Option<TaskKey> {
    let url = url.trim().split(['?', '#']).next()?;
    let path = url.split_once("://").map_or(url, |(_, rest)| rest);
    let mut segments = path.split('/').filter(|s| !s.is_empty());
    // Clone URLs carry a user name: `https://contoso@dev.azure.com/…`.
    let authority = segments.next()?;
    let host = authority.rsplit('@').next()?.to_ascii_lowercase();
    let rest: Vec<&str> = segments.collect();
    let after = |name: &str| {
        rest.iter()
            .position(|s| s.eq_ignore_ascii_case(name))
            .and_then(|i| rest.get(i + 1))
            .copied()
    };
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());

    if host == "linear.app" {
        // `linear.app/<workspace>/issue/<KEY>[/<slug>]`
        let [workspace, issue, key, ..] = rest.as_slice() else {
            return None;
        };
        issue.eq_ignore_ascii_case("issue").then_some(())?;
        let (team, number) = key.split_once('-')?;
        let team_ok = !team.is_empty() && team.chars().all(|c| c.is_ascii_alphanumeric());
        return (team_ok && digits(number)).then(|| TaskKey {
            provider: "linear",
            scope: workspace.to_ascii_lowercase(),
            key: key.to_ascii_uppercase(),
        });
    }
    let organization = if host == "dev.azure.com" {
        rest.first()
            .filter(|s| !s.starts_with('_'))?
            .to_ascii_lowercase()
    } else {
        host.strip_suffix(".visualstudio.com")?.to_string()
    };
    rest.iter()
        .any(|s| s.eq_ignore_ascii_case("_workitems"))
        .then_some(())?;
    let number = after("edit")?;
    digits(number).then(|| TaskKey {
        provider: "azure_devops",
        scope: organization,
        key: format!("#{number}"),
    })
}

/// The detected row a registered asset describes, if any.
///
/// By [`matches`] first. Failing that, by branch and project — but only for
/// what can live on a branch, and never for an asset whose URL matched
/// nothing unless it is a branch: any other URL names something else, a
/// document or a ticket, which would otherwise vanish into the PR row and lose
/// its link, while a branch's link names the branch, which its PR row now is.
/// The agent names the branch in `branch`, or — for a branch asset — in its
/// `…/tree/<branch>` link or as its title. With no project the branch has to be
/// unambiguous, since a task spanning several repos uses the same branch name
/// in each of them.
fn detected_match(rows: &[SessionAsset], asset: &AgentAsset) -> Option<usize> {
    if let Some(i) = rows.iter().position(|row| matches(asset, row)) {
        return Some(i);
    }
    let links_elsewhere = asset.url.is_some() && asset.kind != AgentAssetKind::Branch;
    if links_elsewhere
        || !matches!(
            asset.kind,
            AgentAssetKind::Branch | AgentAssetKind::PullRequest | AgentAssetKind::Other
        )
    {
        return None;
    }

    let branch = asset.branch.as_deref().or(match asset.kind {
        AgentAssetKind::Branch => asset
            .url
            .as_deref()
            .and_then(branch_from_url)
            .or(Some(asset.title.as_str())),
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

/// The branch a branch link names: `feat/x` in
/// `https://github.com/o/r/tree/feat/x`. `None` for any other link.
fn branch_from_url(url: &str) -> Option<&str> {
    let url = url.trim().split(['?', '#']).next()?;
    let (_, branch) = url.split_once("/tree/")?;
    Some(branch.trim_end_matches('/')).filter(|b| !b.is_empty())
}

/// A URL in the form two spellings of it share: trimmed, without a trailing
/// `/`, lowercased.
pub fn normalize_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_ascii_lowercase()
}

/// Whether two URLs name the same thing, by [`normalize_url`].
pub fn same_url(a: &str, b: &str) -> bool {
    normalize_url(a) == normalize_url(b)
}

fn url_is(a: Option<&str>, b: &str) -> bool {
    a.is_some_and(|a| same_url(a, b))
}

/// Whether `row` is on `branch` in the repo labelled `project`.
fn same_branch(row: &SessionAsset, project: &str, branch: Option<&str>) -> bool {
    branch.is_some()
        && row.branch.as_deref() == branch
        && row
            .project
            .as_deref()
            .is_some_and(|p| p.eq_ignore_ascii_case(project))
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
                readiness: None,
                readiness_unavailable: false,
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
            task: None,
        }
    }

    /// A task okena recorded when an agent filed it: titled after the ticket.
    fn filed_task(key: &str, url: &str) -> AgentAsset {
        let provider = task_key_from_url(url).map_or("linear", |k| k.provider);
        let mut asset = registered(AgentAssetKind::Task, "Split payments");
        asset.url = Some(url.into());
        asset.task = Some(TaskRef {
            id: crate::tasks::TaskId::new(provider, format!("uuid-{url}")),
            display_key: key.into(),
            title: "Split payments".into(),
            url: url.into(),
            parent_id: None,
            parent_key: None,
        });
        asset
    }

    #[test]
    fn a_worktree_without_a_pr_is_not_produced() {
        // Local only, pushed, or not polled yet: it shows under WORKTREES.
        let (local, pushed) = (git(None, None), git(Some(0), None));
        for git in [Some(&local), Some(&pushed), None] {
            let rows = derive_session_assets(&[], &[checkout("okena", git)], &[], &[]);
            assert!(rows.is_empty(), "{git:?}: {rows:?}");
        }
    }

    #[test]
    fn a_branch_with_a_pr_becomes_a_pr_row() {
        let g = git(Some(0), Some((7, PrState::Draft)));
        let rows = derive_session_assets(&[], &[checkout("okena", Some(&g))], &[], &[]);
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
    fn a_pr_row_carries_its_readiness_and_checks() {
        let mut g = git(Some(0), Some((7, PrState::Open)));
        if let Some(pr) = g.pr_info.as_mut() {
            pr.readiness = Some(crate::api::PrReadiness {
                merge_state: crate::api::MergeState::Conflicting,
                review_decision: None,
                unresolved_threads: 2,
                threads_truncated: false,
            });
        }
        g.ci_checks = Some(CiCheckSummary {
            status: crate::api::CiStatus::Failure,
            passed: 1,
            failed: 1,
            pending: 0,
            total: 2,
            checks: Vec::new(),
        });
        let rows = derive_session_assets(&[], &[checkout("okena", Some(&g))], &[], &[]);
        let readiness = rows[0].pr.as_ref().and_then(|p| p.readiness.as_ref());
        assert_eq!(readiness.map(|r| r.unresolved_threads), Some(2));
        assert_eq!(
            rows[0].ci.as_ref().map(|c| c.failed),
            Some(1),
            "the row shows the checkout's CI"
        );
    }

    #[test]
    fn a_removed_worktrees_pr_keeps_the_readiness_read_with_it() {
        let mut open = tracked(9, PrState::Open);
        open.readiness = Some(crate::api::PrReadiness {
            merge_state: crate::api::MergeState::Behind,
            review_decision: None,
            unresolved_threads: 1,
            threads_truncated: false,
        });
        let rows = derive_session_assets(&[], &[], &[open], &[]);
        let readiness = rows[0].pr.as_ref().and_then(|p| p.readiness.as_ref());
        assert_eq!(
            readiness.map(|r| r.merge_state),
            Some(crate::api::MergeState::Behind)
        );
    }

    #[test]
    fn a_session_asset_can_be_built_from_its_default() {
        let row = SessionAsset {
            title: "Design notes".into(),
            ..SessionAsset::default()
        };
        assert!(row.pr.is_none() && row.state.is_none() && !row.registered);
    }

    #[test]
    fn a_hand_registered_ticket_link_collapses_into_the_filed_task() {
        let filed = filed_task(
            "QBL-375",
            "https://linear.app/q/issue/QBL-375/session-tasks-are-recorded",
        );
        let mut by_hand = registered(AgentAssetKind::Other, "Recording filed tasks");
        by_hand.url = Some("https://linear.app/q/issue/qbl-375".into());

        for order in [[filed.clone(), by_hand.clone()], [by_hand, filed]] {
            let rows = derive_session_assets(&order, &[], &[], &[]);
            assert_eq!(rows.len(), 1, "{rows:?}");
            let row = &rows[0];
            assert_eq!(
                row.task.as_ref().map(|t| t.display_key.as_str()),
                Some("QBL-375"),
                "the row carries the task"
            );
            assert_eq!(row.kind, AgentAssetKind::Task);
            assert_eq!(row.title, "Recording filed tasks", "the agent's title");
        }
    }

    #[test]
    fn two_different_tasks_stay_two_rows() {
        let rows = derive_session_assets(
            &[
                filed_task("QBL-1", "https://linear.app/q/issue/QBL-1/a"),
                filed_task("QBL-2", "https://linear.app/q/issue/QBL-2/b"),
            ],
            &[],
            &[], &[],
        );
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn a_task_with_a_link_and_a_branch_keeps_its_own_row() {
        // The branch rule only merges what has no link of its own.
        let g = git(Some(0), Some((7, PrState::Open)));
        let mut filed = filed_task("QBL-375", "https://linear.app/q/issue/QBL-375/x");
        filed.branch = Some("feat/x".into());
        filed.project = Some("okena".into());
        let rows = derive_session_assets(&[filed], &[checkout("okena", Some(&g))], &[], &[]);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0].title, "feat/x");
        assert!(rows[0].task.is_none());
        assert!(rows[1].task.is_some());
    }

    fn ticket_link(title: &str, url: &str) -> AgentAsset {
        let mut asset = registered(AgentAssetKind::Other, title);
        asset.url = Some(url.into());
        asset
    }

    #[test]
    fn the_same_azure_devops_number_in_two_organizations_stays_two_rows() {
        let a = "https://dev.azure.com/contoso/Shop/_workitems/edit/42";
        let b = "https://dev.azure.com/fabrikam/Shop/_workitems/edit/42";
        let links = [ticket_link("contoso", a), ticket_link("fabrikam", b)];
        assert_eq!(derive_session_assets(&links, &[], &[], &[]).len(), 2);

        let filed_and_linked = [filed_task("#42", a), ticket_link("fabrikam", b)];
        for order in [
            filed_and_linked.clone(),
            [filed_and_linked[1].clone(), filed_and_linked[0].clone()],
        ] {
            assert_eq!(
                derive_session_assets(&order, &[], &[], &[]).len(),
                2,
                "{order:?}"
            );
        }
    }

    #[test]
    fn the_same_linear_key_in_two_workspaces_stays_two_rows() {
        let a = "https://linear.app/qblok/issue/ENG-12/a";
        let b = "https://linear.app/acme/issue/ENG-12/b";
        let links = [ticket_link("qblok", a), ticket_link("acme", b)];
        assert_eq!(derive_session_assets(&links, &[], &[], &[]).len(), 2);

        let filed = [filed_task("ENG-12", a), filed_task("ENG-12", b)];
        assert_eq!(derive_session_assets(&filed, &[], &[], &[]).len(), 2);

        let by_hand = [
            filed_task("ENG-12", a),
            ticket_link("acme", "https://linear.app/acme/issue/ENG-12"),
        ];
        assert_eq!(derive_session_assets(&by_hand, &[], &[], &[]).len(), 2);
    }

    #[test]
    fn a_work_item_link_collapses_into_the_filed_work_item_with_or_without_a_project() {
        // okena links a work item with no project without one.
        let filed = filed_task("#42", "https://dev.azure.com/contoso/_workitems/edit/42");
        let by_hand = ticket_link(
            "Split payments",
            "https://contoso.visualstudio.com/Web%20Shop/_workitems/edit/42/",
        );
        for order in [[filed.clone(), by_hand.clone()], [by_hand, filed]] {
            let rows = derive_session_assets(&order, &[], &[], &[]);
            assert_eq!(rows.len(), 1, "{rows:?}");
            assert!(rows[0].task.is_some());
        }
    }

    #[test]
    fn task_keys_come_from_linear_and_azure_devops_links_only() {
        let key = |url| task_key_from_url(url).map(|k| k.to_string());
        assert_eq!(
            key("https://linear.app/qblok/issue/QBL-375/session-tasks").as_deref(),
            Some("linear:qblok/QBL-375")
        );
        assert_eq!(
            key("linear.app/QBLOK/issue/qbl-375/").as_deref(),
            Some("linear:qblok/QBL-375")
        );
        assert_eq!(
            key("https://dev.azure.com/Contoso/Web%20Shop/_workitems/edit/42/").as_deref(),
            Some("azure_devops:contoso#42")
        );
        assert_eq!(
            key("https://me@dev.azure.com/contoso/_workitems/edit/42").as_deref(),
            Some("azure_devops:contoso#42")
        );
        assert_eq!(
            key("https://contoso.visualstudio.com/Shop/_workitems/edit/7?x=1").as_deref(),
            Some("azure_devops:contoso#7")
        );
        for other in [
            "https://github.com/o/r/pull/7",
            "https://linear.app/qblok/project/harness-1",
            "https://linear.app/qblok/issue/not-a-key",
            "https://linear.app/issue/QBL-375",
            "https://dev.azure.com/contoso/Shop/_git/repo/pullrequest/42",
            "https://dev.azure.com/_workitems/edit/42",
        ] {
            assert_eq!(key(other), None, "{other}");
        }
    }

    #[test]
    fn a_removed_worktrees_pr_has_no_checks_to_show() {
        let rows = derive_session_assets(&[], &[], &[tracked(9, PrState::Open)], &[]);
        assert_eq!(rows[0].pr.as_ref().map(|p| p.number), Some(9));
        assert!(rows[0].ci.is_none());
    }

    #[test]
    fn uncommitted_changes_show_on_the_row() {
        let mut g = git(Some(0), Some((7, PrState::Open)));
        g.lines_added = 4;
        let rows = derive_session_assets(&[], &[checkout("okena", Some(&g))], &[], &[]);
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
        let (a, b) = (
            git(Some(0), Some((7, PrState::Open))),
            git(Some(1), Some((8, PrState::Draft))),
        );
        let rows = derive_session_assets(
            &[],
            &[checkout("okena", Some(&a)), checkout("web", Some(&b))],
            &[], &[],
        );
        let projects: Vec<_> = rows.iter().map(|r| r.project.as_deref()).collect();
        assert_eq!(projects, [Some("okena"), Some("web")]);
    }

    #[test]
    fn a_registered_pr_merges_by_url_and_keeps_the_agents_title() {
        let g = git(Some(0), Some((7, PrState::Open)));
        let mut pr = registered(AgentAssetKind::PullRequest, "Detect session assets");
        pr.url = Some("https://github.com/o/r/pull/7/".into());
        let rows = derive_session_assets(&[pr.clone(), pr], &[checkout("okena", Some(&g))], &[], &[]);
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
        let (a, b) = (
            git(Some(0), Some((7, PrState::Open))),
            git(Some(0), Some((8, PrState::Open))),
        );
        let mut asset = registered(AgentAssetKind::Other, "web side");
        asset.branch = Some("feat/x".into());
        asset.project = Some("WEB".into());
        let rows = derive_session_assets(
            &[asset],
            &[checkout("okena", Some(&a)), checkout("web", Some(&b))],
            &[], &[],
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].title, "web side");
        assert_eq!(rows[0].title, "feat/x");
    }

    #[test]
    fn a_branch_named_only_by_title_merges_when_unambiguous() {
        let g = git(Some(0), Some((7, PrState::Open)));
        let rows = derive_session_assets(
            &[registered(AgentAssetKind::Branch, "feat/x")],
            &[checkout("okena", Some(&g))],
            &[], &[],
        );
        assert_eq!(rows.len(), 1);
        assert!(rows[0].registered);
        assert_eq!(rows[0].kind, AgentAssetKind::PullRequest);
    }

    #[test]
    fn an_ambiguous_branch_without_a_project_stays_separate() {
        let (a, b) = (
            git(Some(0), Some((7, PrState::Open))),
            git(Some(0), Some((8, PrState::Open))),
        );
        let rows = derive_session_assets(
            &[registered(AgentAssetKind::Branch, "feat/x")],
            &[checkout("okena", Some(&a)), checkout("web", Some(&b))],
            &[], &[],
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].state, None);
    }

    #[test]
    fn a_document_on_the_branch_keeps_its_own_row_and_link() {
        let g = git(Some(0), Some((7, PrState::Open)));
        let mut doc = registered(AgentAssetKind::Document, "Design notes");
        doc.url = Some("https://example.com/doc".into());
        doc.branch = Some("feat/x".into());
        doc.project = Some("okena".into());
        let mut doc_without_link = doc.clone();
        doc_without_link.url = None;
        doc_without_link.title = "Scratch notes".into();

        let rows = derive_session_assets(
            &[doc, doc_without_link],
            &[checkout("okena", Some(&g))],
            &[], &[],
        );
        assert_eq!(rows.len(), 3, "{rows:?}");
        assert_eq!(rows[0].title, "feat/x", "the branch row is not retitled");
        assert!(!rows[0].registered);
        assert_eq!(rows[1].kind, AgentAssetKind::Document);
        assert_eq!(rows[1].url.as_deref(), Some("https://example.com/doc"));
        assert_eq!(rows[2].title, "Scratch notes");
    }

    #[test]
    fn a_pr_whose_link_matched_nothing_does_not_merge_by_branch() {
        // The checkout's PR is a different one than the link names: #8 was
        // polled, #7 has not been yet.
        let g = git(Some(0), Some((8, PrState::Open)));
        let mut pr = registered(AgentAssetKind::PullRequest, "Detect assets");
        pr.url = Some("https://github.com/o/r/pull/7".into());
        pr.branch = Some("feat/x".into());
        let rows = derive_session_assets(&[pr], &[checkout("okena", Some(&g))], &[], &[]);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[1].url.as_deref(),
            Some("https://github.com/o/r/pull/7")
        );
    }

    #[test]
    fn unmatched_registrations_are_listed_after_detected_rows() {
        let g = git(Some(0), Some((7, PrState::Open)));
        let mut doc = registered(AgentAssetKind::Document, "Design notes");
        doc.url = Some("https://example.com/doc".into());
        let rows = derive_session_assets(&[doc], &[checkout("okena", Some(&g))], &[], &[]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].kind, AgentAssetKind::Document);
        assert_eq!(rows[1].state, None);
        assert_eq!(rows[1].task, None);
    }

    #[test]
    fn repeated_unmatched_registrations_collapse_under_the_first_title() {
        let mut first = registered(AgentAssetKind::Document, "Design notes");
        first.url = Some("https://example.com/Doc/".into());
        let mut again = first.clone();
        again.title = "Design notes v2".into();
        again.url = Some("https://example.com/doc".into());

        let rows = derive_session_assets(&[first, again], &[], &[], &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Design notes");
    }

    #[test]
    fn urls_match_across_case_whitespace_and_a_trailing_slash() {
        assert!(same_url(
            " https://GitHub.com/o/r/pull/7/ ",
            "https://github.com/o/r/pull/7"
        ));
        assert!(!same_url(
            "https://github.com/o/r/pull/7",
            "https://github.com/o/r/pull/70"
        ));
    }

    fn tracked(number: u32, state: PrState) -> TrackedPullRequest {
        TrackedPullRequest {
            project: "okena".into(),
            repo_path: "/p/okena".into(),
            branch: Some("feat/x".into()),
            number,
            url: format!("https://github.com/o/r/pull/{number}"),
            state,
            readiness: None,
            readiness_unavailable: false,
        }
    }

    #[test]
    fn the_pr_of_a_removed_worktree_stays_listed() {
        let mut pr = registered(AgentAssetKind::PullRequest, "Agent title");
        pr.url = Some("https://github.com/o/r/pull/9".into());
        let rows = derive_session_assets(&[pr], &[], &[tracked(9, PrState::Open)], &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Agent title");
        assert_eq!(rows[0].project.as_deref(), Some("okena"));
    }

    #[test]
    fn a_registered_pr_stays_one_card_marked_merged_or_closed() {
        // Registered, worktree removed while open, then merged or closed.
        let mut pr = registered(AgentAssetKind::PullRequest, "Agent title");
        pr.url = Some("https://github.com/o/r/pull/9/".into());

        let open =
            derive_session_assets(std::slice::from_ref(&pr), &[], &[tracked(9, PrState::Open)], &[]);
        assert_eq!(open.len(), 1);

        for finished in [PrState::Merged, PrState::Closed] {
            let rows = derive_session_assets(
                std::slice::from_ref(&pr),
                &[],
                &[tracked(9, finished.clone())], &[],
            );
            assert_eq!(rows.len(), 1, "{finished:?}: {rows:?}");
            assert_eq!(rows[0].title, "Agent title", "one card, the agent's title");
            assert_eq!(
                rows[0].state,
                Some(DetectedState::PullRequest {
                    number: 9,
                    state: finished.clone()
                })
            );
        }
    }

    #[test]
    fn a_merged_pr_nobody_registered_stays_listed() {
        let rows = derive_session_assets(&[], &[], &[tracked(9, PrState::Merged)], &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, AgentAssetKind::PullRequest);
        assert_eq!(rows[0].pr.as_ref().map(|p| p.state.clone()), Some(PrState::Merged));
    }

    #[test]
    fn one_card_per_branch_shows_its_newest_pr() {
        // A closed PR replaced by a new one on the same branch.
        let rows = derive_session_assets(
            &[],
            &[],
            &[tracked(9, PrState::Closed), tracked(12, PrState::Open)], &[],
        );
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].pr.as_ref().map(|p| p.number), Some(12));

        // The branch checked out again with its new PR: the live card covers it.
        let g = git(Some(0), Some((12, PrState::Open)));
        let rows = derive_session_assets(
            &[],
            &[checkout("okena", Some(&g))],
            &[tracked(9, PrState::Merged)], &[],
        );
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].pr.as_ref().map(|p| p.number), Some(12));

        // Checked out again with no PR yet: the merged one is still worth a
        // card, and the checkout is not one.
        let bare = git(Some(0), None);
        let rows = derive_session_assets(
            &[],
            &[checkout("okena", Some(&bare))],
            &[tracked(9, PrState::Merged)], &[],
        );
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].pr.as_ref().map(|p| p.number), Some(9));
    }

    #[test]
    fn a_tracked_pr_checked_out_again_is_not_listed_twice() {
        let g = git(Some(0), Some((9, PrState::Open)));
        let rows = derive_session_assets(
            &[],
            &[checkout("okena", Some(&g))],
            &[tracked(9, PrState::Open)], &[],
        );
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn a_pushed_branch_is_a_card_until_its_pr_takes_its_place() {
        let pushed = PushedBranch {
            project: "okena".into(),
            repo_path: "/p/okena".into(),
            branch: "feat/x".into(),
        };
        // Pushed from a worktree the agent made itself: no checkout okena
        // tracks, and no PR yet.
        let rows = derive_session_assets(&[], &[], &[], std::slice::from_ref(&pushed));
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].kind, AgentAssetKind::Branch);
        assert_eq!(rows[0].branch.as_deref(), Some("feat/x"));
        assert_eq!(rows[0].project.as_deref(), Some("okena"));

        // Its PR found and tracked: one card, the PR's.
        let rows = derive_session_assets(
            &[],
            &[],
            &[tracked(9, PrState::Open)],
            std::slice::from_ref(&pushed),
        );
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].kind, AgentAssetKind::PullRequest);
    }

    #[test]
    fn a_push_from_a_linked_worktree_is_not_a_branch_row() {
        let pushed = PushedBranch {
            project: "okena".into(),
            repo_path: "/p/okena".into(),
            branch: "feat/x".into(),
        };
        // On the branch, polled or not: the worktree's, under WORKTREES.
        let g = git(Some(0), None);
        for git in [Some(&g), None] {
            let rows = derive_session_assets(
                &[],
                &[checkout("okena", git)],
                &[],
                std::slice::from_ref(&pushed),
            );
            assert!(rows.is_empty(), "{rows:?}");
        }

        // A worktree on another branch leaves the push its own card.
        let mut other = git(Some(0), None);
        other.branch = Some("feat/y".into());
        let rows = derive_session_assets(
            &[],
            &[LinkedCheckout {
                project: "okena",
                branch: Some("feat/y"),
                git: Some(&other),
            }],
            &[],
            std::slice::from_ref(&pushed),
        );
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].kind, AgentAssetKind::Branch);
    }

    /// A branch as `okena_register_asset` records it, with the agent's title.
    fn registered_branch() -> AgentAsset {
        let mut asset = registered(AgentAssetKind::Branch, "Session asset rules");
        asset.branch = Some("feat/x".into());
        asset.project = Some("okena".into());
        asset
    }

    fn assert_the_prs_row_with_the_agents_title(rows: &[SessionAsset], number: u32) {
        assert_eq!(rows.len(), 1, "{rows:?}");
        let row = &rows[0];
        assert_eq!(row.kind, AgentAssetKind::PullRequest);
        assert_eq!(row.title, "Session asset rules");
        assert!(row.registered);
        assert_eq!(row.pr.as_ref().map(|p| p.number), Some(number));
        assert_eq!(
            row.url.as_deref(),
            Some(format!("https://github.com/o/r/pull/{number}").as_str())
        );
    }

    #[test]
    fn a_registered_branch_becomes_its_prs_row() {
        let asset = registered_branch();

        // No PR yet: the branch row the agent registered, beside no other.
        let bare = git(Some(0), None);
        let rows = derive_session_assets(
            std::slice::from_ref(&asset),
            &[checkout("okena", Some(&bare))],
            &[],
            &[],
        );
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].kind, AgentAssetKind::Branch);
        assert!(rows[0].registered);

        // `gh pr create`: one row, the PR's, with okena's state and CI.
        let mut with_pr = git(Some(0), Some((7, PrState::Open)));
        with_pr.ci_checks = Some(CiCheckSummary {
            status: crate::api::CiStatus::Success,
            passed: 1,
            failed: 0,
            pending: 0,
            total: 1,
            checks: Vec::new(),
        });
        let rows = derive_session_assets(
            std::slice::from_ref(&asset),
            &[checkout("okena", Some(&with_pr))],
            &[],
            &[],
        );
        assert_the_prs_row_with_the_agents_title(&rows, 7);
        assert!(rows[0].ci.is_some());

        // The worktree removed after: the tracked PR is still that one row.
        let rows = derive_session_assets(
            std::slice::from_ref(&asset),
            &[],
            &[tracked(9, PrState::Open)],
            &[],
        );
        assert_the_prs_row_with_the_agents_title(&rows, 9);
    }

    #[test]
    fn a_registered_branch_with_a_link_becomes_its_prs_row() {
        let mut asset = registered_branch();
        asset.url = Some("https://github.com/o/r/tree/feat/x".into());
        let mut by_link_only = asset.clone();
        by_link_only.branch = None;
        by_link_only.project = None;

        let g = git(Some(0), Some((7, PrState::Open)));
        for asset in [asset, by_link_only] {
            let rows = derive_session_assets(
                std::slice::from_ref(&asset),
                &[checkout("okena", Some(&g))],
                &[],
                &[],
            );
            assert_the_prs_row_with_the_agents_title(&rows, 7);

            // Without a PR it keeps its link.
            let rows = derive_session_assets(std::slice::from_ref(&asset), &[], &[], &[]);
            assert_eq!(rows.len(), 1, "{rows:?}");
            assert_eq!(rows[0].kind, AgentAssetKind::Branch);
            assert_eq!(
                rows[0].url.as_deref(),
                Some("https://github.com/o/r/tree/feat/x")
            );
        }
    }

    #[test]
    fn a_registered_branch_in_a_repo_with_no_known_pr_stays_a_branch_row() {
        let mut asset = registered_branch();
        asset.project = Some("web".into());
        // The PR on `feat/x` is okena's, not web's.
        let g = git(Some(0), Some((7, PrState::Open)));
        let rows = derive_session_assets(&[asset], &[checkout("okena", Some(&g))], &[], &[]);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0].kind, AgentAssetKind::PullRequest);
        assert!(!rows[0].registered);
        assert_eq!(rows[1].kind, AgentAssetKind::Branch);
        assert_eq!(rows[1].project.as_deref(), Some("web"));
    }

    #[test]
    fn branches_come_from_tree_links_only() {
        assert_eq!(
            branch_from_url("https://github.com/o/r/tree/feat/x/?tab=readme"),
            Some("feat/x")
        );
        assert_eq!(branch_from_url("https://github.com/o/r/pull/7"), None);
        assert_eq!(branch_from_url("https://github.com/o/r/tree/"), None);
    }
}
