//! CI / PR integration: GitHub PR info and CI check aggregation.
//!
//! `fetch_pr_info` / `fetch_ci_checks` run the queries behind `gh pr list
//! --head`, `gh pr checks` and `gh api .../check-runs|status` over the shared
//! HTTP bus ([`super::github`]) instead of a subprocess per query. The payload
//! mapping is pure and unit-tested. `list_pull_requests` still shells out to `gh`.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use okena_core::process::{command, safe_output_with_timeout};
use serde_json::{Value, json};

use super::github::{ApiError, GithubClient, GithubRepo, origin_repo, resolve_base_repo};
use super::status::get_pushed_sha;

/// Hard cap on the remaining `gh` invocation. `gh` can hang indefinitely —
/// auth prompts or a stalled network — and the bus kills the process when
/// this elapses.
const GH_TIMEOUT: Duration = Duration::from_secs(15);

/// Outcome of a PR lookup. `RateLimited` is kept distinct from "no PR" so the
/// poller can park its whole GitHub fan-out instead of hammering a closed door.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrFetch {
    Fetched(Option<crate::PrInfo>),
    RateLimited,
    /// No answer: a timeout, a server error, or no client to ask with. Kept
    /// apart from `Fetched(None)` so a flaky request never erases a known PR.
    Failed,
}

/// Outcome of a CI lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CiFetch {
    /// The upstream commit still matches the caller's cached result, so nothing
    /// was requested and the cached summary stays valid.
    Unchanged,
    /// A request ran. `sha` is the upstream commit the summary describes, used
    /// to skip the next fetch while it holds.
    Fetched {
        sha: Option<String>,
        summary: Option<crate::CiCheckSummary>,
    },
    RateLimited,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPullRequest {
    number: u32,
    title: String,
    head_ref_name: String,
}

/// List open pull requests that can be checked out as worktrees.
pub fn list_pull_requests(
    path: &Path,
    limit: usize,
) -> Result<Vec<okena_core::api::WorktreePullRequest>, String> {
    let limit = limit.clamp(1, 100).to_string();
    let output = safe_output_with_timeout(
        command("gh")
            .args([
                "pr",
                "list",
                "--json",
                "number,title,headRefName",
                "--limit",
                &limit,
            ])
            .current_dir(path),
        GH_TIMEOUT,
    )
    .map_err(|error| format!("Failed to run GitHub CLI: {error}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "GitHub CLI failed to list pull requests".to_string()
        } else {
            stderr
        });
    }

    parse_pull_request_list(&String::from_utf8_lossy(&output.stdout))
}

fn parse_pull_request_list(
    json: &str,
) -> Result<Vec<okena_core::api::WorktreePullRequest>, String> {
    let pull_requests: Vec<GhPullRequest> = serde_json::from_str(json)
        .map_err(|error| format!("Failed to parse pull requests: {error}"))?;
    Ok(pull_requests
        .into_iter()
        .map(|pull_request| okena_core::api::WorktreePullRequest {
            number: pull_request.number,
            title: pull_request.title,
            branch: pull_request.head_ref_name,
        })
        .collect())
}

/// Base repository plus an authenticated client, or `None` when the checkout
/// has no usable GitHub remote or no token is obtainable — the cases in which
/// `gh` used to fail.
fn github_client(path: &Path) -> Option<(GithubClient, GithubRepo)> {
    let repo = resolve_base_repo(path)?;
    let client = GithubClient::for_host(&repo.host)?;
    Some((client, repo))
}

/// The query behind `gh pr list --head <branch> --state all --limit 1`.
const PR_LOOKUP_QUERY: &str = r#"
query PullRequestList($owner: String!, $repo: String!, $headBranch: String!) {
  repository(owner: $owner, name: $repo) {
    pullRequests(
      states: [OPEN, CLOSED, MERGED],
      headRefName: $headBranch,
      first: 1,
      orderBy: {field: CREATED_AT, direction: DESC}
    ) {
      nodes { url state isDraft number baseRefName headRefOid }
    }
  }
}"#;

/// The `pullRequests.nodes[]` entry of [`PR_LOOKUP_QUERY`].
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct PrNode {
    url: String,
    state: String,
    is_draft: bool,
    number: u32,
    base_ref_name: Option<String>,
    head_ref_oid: Option<String>,
    /// Only asked for by number, where the branch is not already known.
    head_ref_name: Option<String>,
}

/// Get PR info for the current branch (if any PR exists).
///
/// Matches by head branch name in the base repository, like `gh pr list
/// --head`, rather than deriving the PR's head repository from the branch's
/// tracking remote (what `gh pr view` does): on a fork/upstream split the
/// latter reports "no pull requests found" even when a PR exists.
pub fn fetch_pr_info(path: &Path) -> PrFetch {
    let Some(branch) = super::status::get_current_branch(path) else {
        return PrFetch::Fetched(None);
    };
    let current_sha = super::status::get_head_sha(path);
    let pushed_sha = get_pushed_sha(path);
    // No token or no GitHub remote to ask: nothing was learned, so a PR the
    // caller already knows must not be read as gone.
    let Some((mut client, repo)) = github_client(path) else {
        return PrFetch::Failed;
    };

    let variables = json!({ "owner": repo.owner, "repo": repo.name, "headBranch": branch });
    match client.graphql(PR_LOOKUP_QUERY, variables) {
        Err(ApiError::RateLimited) => PrFetch::RateLimited,
        Err(ApiError::Failed) => PrFetch::Failed,
        // The repository is gone, or this token can no longer see it.
        Err(ApiError::NotFound) => PrFetch::Fetched(None),
        Ok(data) => {
            let node = data
                .pointer("/repository/pullRequests/nodes/0")
                .cloned()
                .and_then(|node| serde_json::from_value::<PrNode>(node).ok());
            PrFetch::Fetched(node.and_then(|node| {
                pr_info_from_node(node, current_sha.as_deref(), pushed_sha.as_deref())
            }))
        }
    }
}

/// The query behind `gh pr view <number>`.
const PR_BY_NUMBER_QUERY: &str = r#"
query PullRequestByNumber($owner: String!, $repo: String!, $number: Int!) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) { url state isDraft number baseRefName headRefOid headRefName }
  }
}"#;

/// Get a PR by number in the base repository of the checkout at `repo_path`.
///
/// For a PR whose worktree is gone: there is no branch left to look it up by,
/// but the repo it came from still resolves the remote and credentials. Unlike
/// [`fetch_pr_info`], a merged or closed PR is reported as such, since that is
/// exactly what the caller is waiting to hear.
pub fn fetch_pr_by_number(repo_path: &Path, number: u32) -> PrFetch {
    fetch_pr_by_number_with_head(repo_path, number).0
}

/// [`fetch_pr_by_number`], plus the branch the PR was opened from.
///
/// For a PR an agent registered by its link: whether one of the session's
/// live worktrees is on that branch decides whether its own detection already
/// covers the PR.
pub fn fetch_pr_by_number_with_head(repo_path: &Path, number: u32) -> (PrFetch, Option<String>) {
    // No request goes out, so nothing is learned — not even that it's gone.
    let Some((mut client, repo)) = github_client(repo_path) else {
        return (PrFetch::Failed, None);
    };
    let variables = json!({ "owner": repo.owner, "repo": repo.name, "number": number });
    match client.graphql(PR_BY_NUMBER_QUERY, variables) {
        Err(ApiError::RateLimited) => (PrFetch::RateLimited, None),
        Err(ApiError::Failed) => (PrFetch::Failed, None),
        // GitHub's answer for a deleted PR or repository, or one this token
        // can no longer see: it is gone, as far as anyone here can tell.
        Err(ApiError::NotFound) => (PrFetch::Fetched(None), None),
        Ok(data) => {
            let node = data
                .pointer("/repository/pullRequest")
                .cloned()
                .and_then(|node| serde_json::from_value::<PrNode>(node).ok());
            let head = node
                .as_ref()
                .and_then(|node| node.head_ref_name.clone())
                .filter(|branch| !branch.is_empty());
            (PrFetch::Fetched(node.and_then(pr_info_of)), head)
        }
    }
}

/// Like [`PR_LOOKUP_QUERY`], but a few PRs, each with its head repository and
/// its recent commits: a branch name alone does not say whose branch it is.
/// `origin` is the checkout's push remote as GitHub names it today, which
/// follows a rename or transfer the remote URL still predates.
const PR_BRANCH_LOOKUP_QUERY: &str = r#"
query PullRequestsByHead($owner: String!, $repo: String!, $headBranch: String!, $originOwner: String!, $originName: String!) {
  origin: repository(owner: $originOwner, name: $originName) { owner { login } name }
  repository(owner: $owner, name: $repo) {
    pullRequests(
      states: [OPEN, CLOSED, MERGED],
      headRefName: $headBranch,
      first: 10,
      orderBy: {field: CREATED_AT, direction: DESC}
    ) {
      nodes {
        url state isDraft number baseRefName headRefOid
        headRepositoryOwner { login }
        headRepository { name }
        commits(last: 100) { nodes { commit { oid } } }
      }
    }
  }
}"#;

/// A `pullRequests.nodes[]` entry of [`PR_BRANCH_LOOKUP_QUERY`].
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct BranchPrNode {
    #[serde(flatten)]
    pr: PrNode,
    head_repository_owner: Option<HeadOwner>,
    head_repository: Option<HeadRepository>,
    commits: Option<PrCommits>,
}

/// A PR's most recent commits, newest last.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct PrCommits {
    nodes: Vec<PrCommitNode>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct PrCommitNode {
    commit: Option<CommitOid>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct CommitOid {
    oid: String,
}

impl BranchPrNode {
    /// Whether the PR holds any of `tips` — at its head, or among its recent
    /// commits when GitHub has moved on since the local refs were fetched.
    fn holds_any(&self, tips: &[String]) -> bool {
        let head = self.pr.head_ref_oid.as_deref().map(str::trim);
        let recent = self
            .commits
            .iter()
            .flat_map(|commits| &commits.nodes)
            .filter_map(|node| node.commit.as_ref())
            .map(|commit| commit.oid.as_str());
        head.into_iter()
            .chain(recent)
            .any(|oid| tips.iter().any(|tip| tip == oid))
    }
}

/// `origin` as GitHub names it today, from [`PR_BRANCH_LOOKUP_QUERY`]'s
/// `origin` field, or the remote URL's own reading when GitHub gave none.
fn origin_as_github_sees_it(data: &Value, origin: GithubRepo) -> GithubRepo {
    let owner = data.pointer("/origin/owner/login").and_then(Value::as_str);
    let name = data.pointer("/origin/name").and_then(Value::as_str);
    match (owner, name) {
        (Some(owner), Some(name)) if !owner.is_empty() && !name.is_empty() => GithubRepo {
            host: origin.host,
            owner: owner.to_string(),
            name: name.to_string(),
        },
        _ => origin,
    }
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct HeadOwner {
    login: String,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct HeadRepository {
    name: String,
}

/// Get the newest PR that was opened from `branch` of the checkout at
/// `repo_path`, whatever its state.
///
/// For a worktree removed before its PR was ever polled: the branch name is
/// all that is left of it, and a name is not an identity. So the PR's head
/// must live in `origin` — as GitHub names it today, not a fork's branch that
/// happens to share the name — and, while the branch still exists locally or
/// on origin, hold its tip: at its head, or among its last 100 commits when
/// GitHub is ahead of the local refs ("Update branch", an applied suggestion,
/// a push from elsewhere). The default branch is never looked up: every fork
/// has one.
pub fn fetch_pr_by_branch(repo_path: &Path, branch: &str) -> PrFetch {
    let default_branch = super::branch::get_default_branch(repo_path);
    if !branch_lookup_allowed(branch, default_branch.as_deref()) {
        return PrFetch::Fetched(None);
    }
    // Without knowing where this checkout pushes, its PR cannot be told apart
    // from anyone else's on the same name.
    let Some(origin) = origin_repo(repo_path) else {
        return PrFetch::Fetched(None);
    };
    let Some((mut client, repo)) = github_client(repo_path) else {
        return PrFetch::Failed;
    };
    let tips = branch_tips(repo_path, branch);
    let variables = json!({
        "owner": repo.owner,
        "repo": repo.name,
        "headBranch": branch,
        "originOwner": origin.owner,
        "originName": origin.name,
    });
    match client.graphql(PR_BRANCH_LOOKUP_QUERY, variables) {
        Err(ApiError::RateLimited) => PrFetch::RateLimited,
        Err(ApiError::Failed) => PrFetch::Failed,
        // The repository, or origin itself, is gone: no PR of ours is there.
        Err(ApiError::NotFound) => PrFetch::Fetched(None),
        Ok(data) => {
            let head = origin_as_github_sees_it(&data, origin);
            let nodes = data
                .pointer("/repository/pullRequests/nodes")
                .cloned()
                .and_then(|nodes| serde_json::from_value::<Vec<BranchPrNode>>(nodes).ok())
                .unwrap_or_default();
            PrFetch::Fetched(pick_branch_pr(nodes, &head, &tips).and_then(pr_info_of))
        }
    }
}

/// Whether a branch is worth looking a PR up by: not blank, and never the
/// default branch.
fn branch_lookup_allowed(branch: &str, default_branch: Option<&str>) -> bool {
    let branch = branch.trim();
    !branch.is_empty() && Some(branch) != default_branch
}

/// The newest PR that is this checkout's: its head lives in `head` (origin),
/// and — when the branch's tip is still known — it holds that tip.
fn pick_branch_pr(nodes: Vec<BranchPrNode>, head: &GithubRepo, tips: &[String]) -> Option<PrNode> {
    nodes
        .into_iter()
        .find(|node| {
            let owner_matches = node
                .head_repository_owner
                .as_ref()
                .is_some_and(|owner| owner.login.eq_ignore_ascii_case(&head.owner));
            let repo_matches = node
                .head_repository
                .as_ref()
                .is_some_and(|repo| repo.name.eq_ignore_ascii_case(&head.name));
            owner_matches && repo_matches && (tips.is_empty() || node.holds_any(tips))
        })
        .map(|node| node.pr)
}

/// Where `branch` points in the repo at `repo_path`: its local head and its
/// origin tracking ref, whichever still exist.
fn branch_tips(repo_path: &Path, branch: &str) -> Vec<String> {
    let Some(repo) = crate::gix_helpers::open(repo_path) else {
        return Vec::new();
    };
    [
        format!("refs/heads/{branch}"),
        format!("refs/remotes/origin/{branch}"),
    ]
    .iter()
    .filter_map(|name| repo.rev_parse_single(name.as_str()).ok())
    .map(|id| id.detach().to_hex().to_string())
    .collect()
}

/// Map a PR node to [`PrInfo`](crate::PrInfo). Returns `None` for a closed PR
/// whose head is no longer the current branch head.
fn pr_info_from_node(
    node: PrNode,
    current_sha: Option<&str>,
    pushed_sha: Option<&str>,
) -> Option<crate::PrInfo> {
    let head_oid = node
        .head_ref_oid
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let is_closed_pr = !node.is_draft && matches!(node.state.as_str(), "MERGED" | "CLOSED");
    if is_closed_pr && !closed_pr_head_matches(head_oid, current_sha, pushed_sha) {
        return None;
    }
    pr_info_of(node)
}

/// Map a PR node to [`PrInfo`](crate::PrInfo), whatever its state.
fn pr_info_of(node: PrNode) -> Option<crate::PrInfo> {
    if !node.url.starts_with("http") {
        return None;
    }
    let state = if node.is_draft {
        crate::PrState::Draft
    } else {
        match node.state.as_str() {
            "OPEN" => crate::PrState::Open,
            "MERGED" => crate::PrState::Merged,
            "CLOSED" => crate::PrState::Closed,
            other => {
                log::warn!("Unknown PR state '{}', defaulting to Open", other);
                crate::PrState::Open
            }
        }
    };
    let base = node
        .base_ref_name
        .map(|base| base.trim().to_string())
        .filter(|base| !base.is_empty());
    Some(crate::PrInfo {
        url: node.url,
        state,
        number: node.number,
        base,
    })
}

fn closed_pr_head_matches(
    head_oid: Option<&str>,
    current_sha: Option<&str>,
    pushed_sha: Option<&str>,
) -> bool {
    let Some(head_oid) = head_oid else {
        return false;
    };
    current_sha == Some(head_oid) || pushed_sha == Some(head_oid)
}

fn timestamp_secs(iso: &str) -> Option<i64> {
    gix::date::parse(iso, None).ok().map(|t| t.seconds)
}

/// Compute elapsed milliseconds between two ISO-8601 timestamps. Returns 0
/// when either timestamp is missing or unparseable — interpreted as
/// "still running" / "unknown" by the UI.
fn compute_elapsed_ms(started: Option<&str>, completed: Option<&str>) -> u64 {
    let (Some(s), Some(c)) = (started, completed) else {
        return 0;
    };
    match (timestamp_secs(s), timestamp_secs(c)) {
        (Some(a), Some(b)) if b >= a => ((b - a) * 1000) as u64,
        _ => 0,
    }
}

/// gh's per-check buckets (`gh pr checks --json bucket`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bucket {
    Pass,
    Fail,
    Cancel,
    Pending,
    Skipping,
}

/// gh's bucket table over the state a check ended in (or is in).
fn bucket_for(state: &str) -> Bucket {
    match state {
        "SUCCESS" => Bucket::Pass,
        "SKIPPED" | "NEUTRAL" => Bucket::Skipping,
        "ERROR" | "FAILURE" | "TIMED_OUT" | "ACTION_REQUIRED" => Bucket::Fail,
        "CANCELLED" => Bucket::Cancel,
        // EXPECTED, REQUESTED, WAITING, QUEUED, PENDING, IN_PROGRESS, STALE
        _ => Bucket::Pending,
    }
}

/// One row of `gh pr checks --json ...` — what the summary is built from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GhCheck {
    bucket: Bucket,
    name: String,
    workflow: Option<String>,
    link: Option<String>,
    description: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
}

/// Roll `gh pr checks` rows up into a [`CiCheckSummary`](crate::CiCheckSummary).
/// Skipped checks stay in the per-check list (flagged via `is_skipped`) but do
/// not count toward the totals; all-skipped yields `None`.
fn summarize_checks(checks: Vec<GhCheck>) -> Option<crate::CiCheckSummary> {
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut pending = 0usize;
    let mut rows: Vec<crate::CiCheck> = Vec::with_capacity(checks.len());

    for check in checks {
        let (status, is_skipped) = match check.bucket {
            Bucket::Pass => {
                passed += 1;
                (crate::CiStatus::Success, false)
            }
            Bucket::Fail | Bucket::Cancel => {
                failed += 1;
                (crate::CiStatus::Failure, false)
            }
            Bucket::Pending => {
                pending += 1;
                (crate::CiStatus::Pending, false)
            }
            Bucket::Skipping => (crate::CiStatus::Pending, true),
        };
        rows.push(crate::CiCheck {
            name: check.name,
            workflow: check.workflow,
            status,
            is_skipped,
            link: check.link,
            description: check.description,
            elapsed_ms: compute_elapsed_ms(
                check.started_at.as_deref(),
                check.completed_at.as_deref(),
            ),
        });
    }

    let total = passed + failed + pending;
    if total == 0 {
        return None;
    }
    Some(crate::CiCheckSummary {
        status: rollup_status(failed, pending),
        passed,
        failed,
        pending,
        total,
        checks: rows,
    })
}

fn rollup_status(failed: usize, pending: usize) -> crate::CiStatus {
    if failed > 0 {
        crate::CiStatus::Failure
    } else if pending > 0 {
        crate::CiStatus::Pending
    } else {
        crate::CiStatus::Success
    }
}

/// Get CI check status for the current branch.
///
/// With a known PR number, reads the PR's status-check rollup (Actions +
/// external status checks aggregated by the PR, as `gh pr checks` does).
/// Otherwise falls back to `check-runs` + `status` on the current upstream
/// commit, which works for any pushed branch — including default branches
/// without a PR.
///
/// `unchanged_sha` is the upstream commit a *settled* cached summary describes.
/// Checks on a given commit only move while something is running, so when the
/// branch still points at that commit the whole lookup is skipped — no request.
/// This is what keeps a machine with many projects inside GitHub's hourly
/// budget: a repo parked on `main` costs one cheap local ref read per poll.
///
/// Returns `Fetched(None)` when there are no checks, when no GitHub token is
/// available, or when the repo has no GitHub remote.
pub fn fetch_ci_checks(
    path: &Path,
    pr_number: Option<u32>,
    unchanged_sha: Option<&str>,
) -> CiFetch {
    // Read locally (gix, no network) before deciding to spend a request.
    let sha = get_pushed_sha(path);
    if let (Some(sha), Some(cached)) = (sha.as_deref(), unchanged_sha)
        && sha == cached
    {
        return CiFetch::Unchanged;
    }

    match pr_number {
        Some(number) => fetch_pr_checks(path, number, sha),
        None => fetch_branch_checks(path, sha),
    }
}

/// gh's `statusCheckRollup` selection for `gh pr checks`, paged the same way.
const PR_CHECKS_QUERY: &str = r#"
query PullRequestStatusChecks($owner: String!, $repo: String!, $number: Int!, $endCursor: String) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      commits(last: 1) {
        nodes {
          commit {
            statusCheckRollup {
              contexts(first: 100, after: $endCursor) {
                nodes {
                  __typename
                  ... on StatusContext { context state targetUrl description }
                  ... on CheckRun {
                    name status conclusion startedAt completedAt detailsUrl
                    checkSuite { workflowRun { event workflow { name } } }
                  }
                }
                pageInfo { hasNextPage endCursor }
              }
            }
          }
        }
      }
    }
  }
}"#;

/// One `statusCheckRollup.contexts` node — the union gh aggregates.
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "__typename", rename_all_fields = "camelCase")]
enum RollupContext {
    CheckRun {
        #[serde(default)]
        name: String,
        #[serde(default)]
        status: String,
        conclusion: Option<String>,
        started_at: Option<String>,
        completed_at: Option<String>,
        details_url: Option<String>,
        check_suite: Option<CheckSuite>,
    },
    StatusContext {
        #[serde(default)]
        context: String,
        #[serde(default)]
        state: String,
        target_url: Option<String>,
        description: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckSuite {
    workflow_run: Option<WorkflowRun>,
}

#[derive(Debug, serde::Deserialize)]
struct WorkflowRun {
    event: Option<String>,
    workflow: Option<Workflow>,
}

#[derive(Debug, serde::Deserialize)]
struct Workflow {
    name: Option<String>,
}

/// Fetch the PR's checks the way `gh pr checks <number>` does. The explicit PR
/// number is used rather than current-branch resolution, which (like `gh pr
/// view`) misfires on fork/upstream-split repos.
fn fetch_pr_checks(path: &Path, pr_number: u32, sha: Option<String>) -> CiFetch {
    let Some((mut client, repo)) = github_client(path) else {
        return CiFetch::Fetched { sha, summary: None };
    };

    let mut contexts: Vec<RollupContext> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let variables = json!({
            "owner": repo.owner,
            "repo": repo.name,
            "number": pr_number,
            "endCursor": cursor,
        });
        let data = match client.graphql(PR_CHECKS_QUERY, variables) {
            Ok(data) => data,
            Err(ApiError::RateLimited) => return CiFetch::RateLimited,
            Err(ApiError::Failed | ApiError::NotFound) => {
                return CiFetch::Fetched { sha, summary: None };
            }
        };
        // A commit with no checks has a null rollup — nothing to read.
        let Some(page) = data
            .pointer("/repository/pullRequest/commits/nodes/0/commit/statusCheckRollup/contexts")
        else {
            break;
        };
        match page
            .get("nodes")
            .cloned()
            .map(serde_json::from_value::<Vec<RollupContext>>)
        {
            Some(Ok(nodes)) => contexts.extend(nodes),
            Some(Err(_)) => return CiFetch::Fetched { sha, summary: None },
            None => {}
        }
        let has_next = page
            .pointer("/pageInfo/hasNextPage")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        cursor = page
            .pointer("/pageInfo/endCursor")
            .and_then(Value::as_str)
            .map(String::from);
        if !has_next || cursor.is_none() {
            break;
        }
    }

    CiFetch::Fetched {
        summary: summarize_checks(aggregate_checks(contexts)),
        sha,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum DedupeKey {
    Status(String),
    Run(String),
}

/// Port of gh's `eliminateDuplicates` + `aggregateChecks`: newest run first,
/// one row per status context and per check-run `name/workflow/event`.
fn aggregate_checks(contexts: Vec<RollupContext>) -> Vec<GhCheck> {
    let mut rows: Vec<(DedupeKey, i64, GhCheck)> =
        contexts.into_iter().filter_map(rollup_row).collect();
    // gh sorts newest-first so the dedupe keeps the latest re-run.
    rows.sort_by_key(|(_, started, _)| std::cmp::Reverse(*started));
    let mut seen = HashSet::new();
    rows.into_iter()
        .filter(|(key, _, _)| seen.insert(key.clone()))
        .map(|(_, _, check)| check)
        .collect()
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.is_empty())
}

fn rollup_row(context: RollupContext) -> Option<(DedupeKey, i64, GhCheck)> {
    match context {
        RollupContext::CheckRun {
            name,
            status,
            conclusion,
            started_at,
            completed_at,
            details_url,
            check_suite,
        } => {
            let state = if status == "COMPLETED" {
                conclusion.unwrap_or_default()
            } else {
                status
            };
            let run = check_suite.and_then(|suite| suite.workflow_run);
            let workflow = run
                .as_ref()
                .and_then(|run| run.workflow.as_ref())
                .and_then(|workflow| workflow.name.clone());
            let event = run.and_then(|run| run.event);
            let key = DedupeKey::Run(format!(
                "{name}/{}/{}",
                workflow.as_deref().unwrap_or(""),
                event.as_deref().unwrap_or("")
            ));
            let started = started_at
                .as_deref()
                .and_then(timestamp_secs)
                .unwrap_or(i64::MIN);
            Some((
                key,
                started,
                GhCheck {
                    bucket: bucket_for(&state),
                    name,
                    workflow: non_empty(workflow),
                    link: non_empty(details_url),
                    description: None,
                    started_at,
                    completed_at,
                },
            ))
        }
        // gh never fills startedAt for a status context: it sorts last and
        // shows no elapsed time.
        RollupContext::StatusContext {
            context,
            state,
            target_url,
            description,
        } => Some((
            DedupeKey::Status(context.clone()),
            i64::MIN,
            GhCheck {
                bucket: bucket_for(&state),
                name: context,
                workflow: None,
                link: non_empty(target_url),
                description: non_empty(description),
                started_at: None,
                completed_at: None,
            },
        )),
        RollupContext::Other => None,
    }
}

/// Fetch CI checks for the branch's last *pushed* commit via the REST API.
/// Combines GitHub Actions check-runs with the older commit-status API
/// (which is what services like Vercel, CircleCI deploy bots, etc. still
/// use) into a single `CiCheckSummary`.
///
/// Uses the upstream (pushed) SHA, not the local HEAD: CI runs on what's on the
/// remote, so an unpushed local commit has no checks. Returns no summary
/// (skipping the API calls) when the branch has no upstream — nothing has been
/// pushed.
fn fetch_branch_checks(path: &Path, sha: Option<String>) -> CiFetch {
    let Some(sha) = sha else {
        return CiFetch::Fetched {
            sha: None,
            summary: None,
        };
    };
    let fetched = |summary| CiFetch::Fetched {
        sha: Some(sha.clone()),
        summary,
    };
    let Some((mut client, repo)) = github_client(path) else {
        return fetched(None);
    };
    let commit = format!("repos/{}/{}/commits/{sha}", repo.owner, repo.name);

    let check_runs =
        match client.rest_get_all(&format!("{commit}/check-runs?per_page=100"), "check_runs") {
            Ok(runs) => Some(runs),
            Err(ApiError::RateLimited) => return CiFetch::RateLimited,
            Err(ApiError::Failed | ApiError::NotFound) => None,
        };
    let statuses = match client.rest_get_json(&format!("{commit}/status")) {
        Ok(combined) => Some(
            combined
                .get("statuses")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        ),
        Err(ApiError::RateLimited) => return CiFetch::RateLimited,
        Err(ApiError::Failed | ApiError::NotFound) => None,
    };

    if check_runs.is_none() && statuses.is_none() {
        return fetched(None);
    }
    fetched(branch_ci_summary(
        &check_runs.unwrap_or_default(),
        &statuses.unwrap_or_default(),
    ))
}

/// Combine REST `check-runs` entries and `status.statuses` entries into a
/// unified `CiCheckSummary`. Either list may be empty (the other endpoint
/// still supplies usable data); both being empty produces `None`.
///
/// `check-runs` is the modern GitHub Actions API — bucketing matches
/// `gh pr checks` conventions (`pass`/`fail`/`pending`/`skipping`).
/// `statuses` is the legacy commit-status API used by external services
/// (Vercel, CircleCI deploy bots, …) — `state` is `success`/`failure`/
/// `error`/`pending`.
fn branch_ci_summary(check_runs: &[Value], statuses: &[Value]) -> Option<crate::CiCheckSummary> {
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut pending = 0usize;
    let mut checks: Vec<crate::CiCheck> = Vec::new();

    for run in check_runs {
        let name = run
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("(unnamed)")
            .to_string();
        let status_str = run.get("status").and_then(|v| v.as_str()).unwrap_or("");
        let conclusion = run.get("conclusion").and_then(|v| v.as_str()).unwrap_or("");
        let (status, is_skipped) = match (status_str, conclusion) {
            (_, "success") => {
                passed += 1;
                (crate::CiStatus::Success, false)
            }
            (_, "failure")
            | (_, "timed_out")
            | (_, "action_required")
            | (_, "cancelled")
            | (_, "stale")
            | (_, "startup_failure") => {
                failed += 1;
                (crate::CiStatus::Failure, false)
            }
            (_, "skipped") | (_, "neutral") => (crate::CiStatus::Pending, true),
            ("queued", _)
            | ("in_progress", _)
            | ("waiting", _)
            | ("pending", _)
            | ("requested", _) => {
                pending += 1;
                (crate::CiStatus::Pending, false)
            }
            _ => continue,
        };
        let link = run
            .get("html_url")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let description = run
            .get("output")
            .and_then(|o| o.get("summary"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let workflow = run
            .get("check_suite")
            .and_then(|s| s.get("workflow_id"))
            .and_then(|_| {
                run.get("app")
                    .and_then(|a| a.get("name"))
                    .and_then(|v| v.as_str())
            })
            .filter(|s| !s.is_empty())
            .map(String::from);
        let elapsed_ms = compute_elapsed_ms(
            run.get("started_at").and_then(|v| v.as_str()),
            run.get("completed_at").and_then(|v| v.as_str()),
        );

        checks.push(crate::CiCheck {
            name,
            workflow,
            status,
            is_skipped,
            link,
            description,
            elapsed_ms,
        });
    }

    for st in statuses {
        let name = st
            .get("context")
            .and_then(|v| v.as_str())
            .unwrap_or("(unnamed)")
            .to_string();
        let state = st.get("state").and_then(|v| v.as_str()).unwrap_or("");
        let (status, is_skipped) = match state {
            "success" => {
                passed += 1;
                (crate::CiStatus::Success, false)
            }
            "failure" | "error" => {
                failed += 1;
                (crate::CiStatus::Failure, false)
            }
            "pending" => {
                pending += 1;
                (crate::CiStatus::Pending, false)
            }
            _ => continue,
        };
        let link = st
            .get("target_url")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let description = st
            .get("description")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let elapsed_ms = compute_elapsed_ms(
            st.get("created_at").and_then(|v| v.as_str()),
            st.get("updated_at").and_then(|v| v.as_str()),
        );
        checks.push(crate::CiCheck {
            name,
            workflow: None,
            status,
            is_skipped,
            link,
            description,
            elapsed_ms,
        });
    }

    let total = passed + failed + pending;
    // Everything skipped (or nothing at all) — nothing actionable.
    if total == 0 {
        return None;
    }
    Some(crate::CiCheckSummary {
        status: rollup_status(failed, pending),
        passed,
        failed,
        pending,
        total,
        checks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_worktree_pull_requests() {
        let json = r#"[{"number":12,"title":"Remote worktree","headRefName":"feature/remote"}]"#;
        let pull_requests = super::parse_pull_request_list(json).expect("should parse");
        assert_eq!(pull_requests.len(), 1);
        assert_eq!(pull_requests[0].number, 12);
        assert_eq!(pull_requests[0].title, "Remote worktree");
        assert_eq!(pull_requests[0].branch, "feature/remote");
    }

    #[test]
    fn malformed_worktree_pull_requests_are_rejected() {
        assert!(super::parse_pull_request_list("not json").is_err());
    }

    // ─── PR node mapping tests ─────────────────────────────────────────

    fn pr_node(json: &str) -> PrNode {
        serde_json::from_str(json).expect("pr node")
    }

    #[test]
    fn pr_node_captures_base_branch() {
        let node = pr_node(
            r#"{"url":"https://github.com/o/r/pull/9","state":"OPEN","isDraft":false,"number":9,"baseRefName":"develop","headRefOid":"abc123"}"#,
        );
        let pr = pr_info_from_node(node, Some("different"), None).expect("should parse");
        assert_eq!(pr.url, "https://github.com/o/r/pull/9");
        assert_eq!(pr.state, crate::PrState::Open);
        assert_eq!(pr.number, 9);
        assert_eq!(pr.base.as_deref(), Some("develop"));
    }

    #[test]
    fn pr_node_base_absent_is_none() {
        let node = pr_node(
            r#"{"url":"https://github.com/o/r/pull/3","state":"OPEN","isDraft":false,"number":3,"baseRefName":null,"headRefOid":"abc"}"#,
        );
        let pr = pr_info_from_node(node, None, None).expect("should parse");
        assert_eq!(pr.state, crate::PrState::Open);
        assert_eq!(pr.number, 3);
        assert_eq!(pr.base, None);
    }

    #[test]
    fn pr_node_draft_overrides_state() {
        let node = pr_node(
            r#"{"url":"https://github.com/o/r/pull/5","state":"OPEN","isDraft":true,"number":5,"baseRefName":"main","headRefOid":"abc123"}"#,
        );
        let pr = pr_info_from_node(node, Some("different"), None).expect("should parse");
        assert_eq!(pr.state, crate::PrState::Draft);
        assert_eq!(pr.base.as_deref(), Some("main"));
    }

    #[test]
    fn pr_node_keeps_closed_pr_at_current_head() {
        let node = pr_node(
            r#"{"url":"https://github.com/o/r/pull/3","state":"MERGED","isDraft":false,"number":3,"baseRefName":"main","headRefOid":"abc123"}"#,
        );
        let pr = pr_info_from_node(node, Some("abc123"), None).expect("should parse");
        assert_eq!(pr.state, crate::PrState::Merged);
        assert_eq!(pr.number, 3);

        // The pushed commit counts too: a merged PR whose head is what we
        // pushed, even if HEAD moved on locally.
        let node = pr_node(
            r#"{"url":"https://github.com/o/r/pull/4","state":"CLOSED","isDraft":false,"number":4,"baseRefName":"main","headRefOid":"pushed"}"#,
        );
        let pr = pr_info_from_node(node, Some("local"), Some("pushed")).expect("should parse");
        assert_eq!(pr.state, crate::PrState::Closed);
    }

    #[test]
    fn pr_node_ignores_stale_closed_pr() {
        let node = pr_node(
            r#"{"url":"https://github.com/o/r/pull/4","state":"CLOSED","isDraft":false,"number":4,"baseRefName":"main","headRefOid":"oldsha"}"#,
        );
        assert!(pr_info_from_node(node, Some("newsha"), Some("newsha")).is_none());
    }

    #[test]
    fn pr_node_without_url_is_none() {
        assert!(pr_info_from_node(PrNode::default(), None, None).is_none());
    }

    // ─── check aggregation (gh pr checks parity) ───────────────────────

    fn contexts(json: &str) -> Vec<RollupContext> {
        serde_json::from_str(json).expect("rollup contexts")
    }

    #[test]
    fn check_run_state_uses_conclusion_only_once_completed() {
        let rows = aggregate_checks(contexts(
            r#"[
                {"__typename":"CheckRun","name":"Lint","status":"COMPLETED","conclusion":"SUCCESS","startedAt":"2024-01-01T10:00:00Z","completedAt":"2024-01-01T10:01:12Z","detailsUrl":"https://ex/1","checkSuite":{"workflowRun":{"event":"push","workflow":{"name":"CI"}}}},
                {"__typename":"CheckRun","name":"Test","status":"IN_PROGRESS","conclusion":null,"startedAt":"2024-01-01T10:00:30Z","completedAt":null,"detailsUrl":"https://ex/2","checkSuite":{"workflowRun":{"event":"push","workflow":{"name":"CI"}}}}
            ]"#,
        ));
        assert_eq!(rows.len(), 2);
        // Newest started first.
        assert_eq!(rows[0].name, "Test");
        assert_eq!(rows[0].bucket, Bucket::Pending);
        assert_eq!(rows[1].name, "Lint");
        assert_eq!(rows[1].bucket, Bucket::Pass);
        assert_eq!(rows[1].workflow.as_deref(), Some("CI"));
        assert_eq!(rows[1].link.as_deref(), Some("https://ex/1"));
        assert_eq!(rows[1].description, None);
        assert_eq!(rows[1].started_at.as_deref(), Some("2024-01-01T10:00:00Z"));
    }

    #[test]
    fn check_run_conclusions_map_to_gh_buckets() {
        let cases = [
            ("SUCCESS", Bucket::Pass),
            ("SKIPPED", Bucket::Skipping),
            ("NEUTRAL", Bucket::Skipping),
            ("FAILURE", Bucket::Fail),
            ("TIMED_OUT", Bucket::Fail),
            ("ACTION_REQUIRED", Bucket::Fail),
            ("CANCELLED", Bucket::Cancel),
            ("STALE", Bucket::Pending),
            ("STARTUP_FAILURE", Bucket::Pending),
        ];
        for (conclusion, bucket) in cases {
            let rows = aggregate_checks(contexts(&format!(
                r#"[{{"__typename":"CheckRun","name":"A","status":"COMPLETED","conclusion":"{conclusion}"}}]"#
            )));
            assert_eq!(rows[0].bucket, bucket, "{conclusion}");
        }
        for status in ["QUEUED", "IN_PROGRESS", "WAITING", "PENDING", "REQUESTED"] {
            let rows = aggregate_checks(contexts(&format!(
                r#"[{{"__typename":"CheckRun","name":"A","status":"{status}","conclusion":"SUCCESS"}}]"#
            )));
            assert_eq!(rows[0].bucket, Bucket::Pending, "{status}");
        }
    }

    #[test]
    fn status_contexts_map_like_gh() {
        let rows = aggregate_checks(contexts(
            r#"[
                {"__typename":"StatusContext","context":"vercel/deploy","state":"SUCCESS","targetUrl":"https://v/1","description":"Deployment ready"},
                {"__typename":"StatusContext","context":"ci/circle","state":"ERROR","targetUrl":null,"description":""},
                {"__typename":"StatusContext","context":"ci/pending","state":"PENDING"},
                {"__typename":"StatusContext","context":"ci/expected","state":"EXPECTED"}
            ]"#,
        ));
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].name, "vercel/deploy");
        assert_eq!(rows[0].bucket, Bucket::Pass);
        assert_eq!(rows[0].link.as_deref(), Some("https://v/1"));
        assert_eq!(rows[0].description.as_deref(), Some("Deployment ready"));
        assert_eq!(rows[0].workflow, None);
        assert_eq!(rows[0].started_at, None);
        assert_eq!(rows[1].bucket, Bucket::Fail);
        assert_eq!(rows[1].link, None);
        assert_eq!(rows[1].description, None);
        assert_eq!(rows[2].bucket, Bucket::Pending);
        assert_eq!(rows[3].bucket, Bucket::Pending);
    }

    #[test]
    fn duplicates_keep_the_latest_run_per_name_workflow_and_event() {
        let rows = aggregate_checks(contexts(
            r#"[
                {"__typename":"CheckRun","name":"Test","status":"COMPLETED","conclusion":"FAILURE","startedAt":"2024-01-01T10:00:00Z","checkSuite":{"workflowRun":{"event":"push","workflow":{"name":"CI"}}}},
                {"__typename":"CheckRun","name":"Test","status":"COMPLETED","conclusion":"SUCCESS","startedAt":"2024-01-01T11:00:00Z","checkSuite":{"workflowRun":{"event":"push","workflow":{"name":"CI"}}}},
                {"__typename":"CheckRun","name":"Test","status":"COMPLETED","conclusion":"SUCCESS","startedAt":"2024-01-01T10:30:00Z","checkSuite":{"workflowRun":{"event":"pull_request","workflow":{"name":"CI"}}}},
                {"__typename":"CheckRun","name":"Test","status":"COMPLETED","conclusion":"SUCCESS","startedAt":"2024-01-01T10:20:00Z","checkSuite":{"workflowRun":{"event":"push","workflow":{"name":"Release"}}}},
                {"__typename":"StatusContext","context":"vercel","state":"PENDING"},
                {"__typename":"StatusContext","context":"vercel","state":"SUCCESS"}
            ]"#,
        ));
        // The re-run of CI/push wins; the other event and workflow stay; the
        // status context is deduped by name (first one after the sort wins).
        let summary: Vec<(String, Option<String>, Bucket)> = rows
            .iter()
            .map(|row| (row.name.clone(), row.workflow.clone(), row.bucket))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("Test".to_string(), Some("CI".to_string()), Bucket::Pass),
                ("Test".to_string(), Some("CI".to_string()), Bucket::Pass),
                (
                    "Test".to_string(),
                    Some("Release".to_string()),
                    Bucket::Pass
                ),
                ("vercel".to_string(), None, Bucket::Pending),
            ]
        );
        assert_eq!(rows[0].started_at.as_deref(), Some("2024-01-01T11:00:00Z"));
    }

    #[test]
    fn unknown_context_types_are_ignored() {
        let rows = aggregate_checks(contexts(
            r#"[{"__typename":"Mystery","name":"x"},{"__typename":"CheckRun","name":"A","status":"COMPLETED","conclusion":"SUCCESS"}]"#,
        ));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "A");
    }

    // ─── summary rollup tests ──────────────────────────────────────────

    fn check(bucket: Bucket) -> GhCheck {
        GhCheck {
            bucket,
            name: "check".to_string(),
            workflow: None,
            link: None,
            description: None,
            started_at: None,
            completed_at: None,
        }
    }

    #[test]
    fn summary_all_pass() {
        let result = summarize_checks(vec![
            check(Bucket::Pass),
            check(Bucket::Pass),
            check(Bucket::Pass),
        ])
        .unwrap();
        assert_eq!(result.status, crate::CiStatus::Success);
        assert_eq!(result.passed, 3);
        assert_eq!(result.failed, 0);
        assert_eq!(result.pending, 0);
        assert_eq!(result.total, 3);
    }

    #[test]
    fn summary_with_failure() {
        let result = summarize_checks(vec![
            check(Bucket::Pass),
            check(Bucket::Fail),
            check(Bucket::Pass),
        ])
        .unwrap();
        assert_eq!(result.status, crate::CiStatus::Failure);
        assert_eq!(result.passed, 2);
        assert_eq!(result.failed, 1);
        assert_eq!(result.total, 3);
    }

    #[test]
    fn summary_with_pending() {
        let result = summarize_checks(vec![
            check(Bucket::Pass),
            check(Bucket::Pending),
            check(Bucket::Pending),
        ])
        .unwrap();
        assert_eq!(result.status, crate::CiStatus::Pending);
        assert_eq!(result.passed, 1);
        assert_eq!(result.pending, 2);
        assert_eq!(result.total, 3);
    }

    #[test]
    fn summary_skipping_excluded_from_total() {
        let result = summarize_checks(vec![
            check(Bucket::Pass),
            check(Bucket::Skipping),
            check(Bucket::Pass),
        ])
        .unwrap();
        assert_eq!(result.status, crate::CiStatus::Success);
        assert_eq!(result.passed, 2);
        assert_eq!(result.total, 2);
        assert_eq!(result.checks.len(), 3);
        assert!(result.checks[1].is_skipped);
    }

    #[test]
    fn summary_cancel_counts_as_failure() {
        let result = summarize_checks(vec![check(Bucket::Pass), check(Bucket::Cancel)]).unwrap();
        assert_eq!(result.status, crate::CiStatus::Failure);
        assert_eq!(result.failed, 1);
    }

    #[test]
    fn summary_empty_or_only_skipping_is_none() {
        assert!(summarize_checks(Vec::new()).is_none());
        assert!(summarize_checks(vec![check(Bucket::Skipping), check(Bucket::Skipping)]).is_none());
    }

    #[test]
    fn summary_captures_per_check_details() {
        let checks = vec![
            GhCheck {
                bucket: Bucket::Pass,
                name: "Lint".to_string(),
                workflow: Some("CI".to_string()),
                link: Some("https://ex/1".to_string()),
                description: Some("ok".to_string()),
                started_at: Some("2024-01-01T10:00:00Z".to_string()),
                completed_at: Some("2024-01-01T10:01:12Z".to_string()),
            },
            GhCheck {
                bucket: Bucket::Fail,
                name: "Test (macos)".to_string(),
                workflow: Some("CI".to_string()),
                link: Some("https://ex/2".to_string()),
                description: None,
                started_at: Some("2024-01-01T10:00:00Z".to_string()),
                completed_at: Some("2024-01-01T10:02:51Z".to_string()),
            },
            GhCheck {
                bucket: Bucket::Skipping,
                name: "Deploy".to_string(),
                workflow: Some("CI".to_string()),
                link: None,
                description: None,
                started_at: None,
                completed_at: None,
            },
        ];
        let result = summarize_checks(checks).unwrap();
        assert_eq!(result.total, 2);
        assert_eq!(result.failed, 1);
        assert_eq!(result.checks.len(), 3);

        let lint = &result.checks[0];
        assert_eq!(lint.name, "Lint");
        assert_eq!(lint.workflow.as_deref(), Some("CI"));
        assert_eq!(lint.link.as_deref(), Some("https://ex/1"));
        assert_eq!(lint.description.as_deref(), Some("ok"));
        assert_eq!(lint.elapsed_ms, 72_000);
        assert_eq!(lint.elapsed_label(), "1m12s");
        assert!(!lint.is_skipped);

        let deploy = &result.checks[2];
        assert!(deploy.is_skipped);
        assert_eq!(deploy.elapsed_ms, 0);
        assert_eq!(deploy.elapsed_label(), "\u{2014}");
    }

    // ─── branch-level CI mapping tests ─────────────────────────────────

    fn array(json: &str, key: &str) -> Vec<Value> {
        let value: Value = serde_json::from_str(json).expect("json");
        value[key].as_array().cloned().unwrap_or_default()
    }

    #[test]
    fn branch_ci_check_runs_only() {
        let json = r#"{
            "total_count": 3,
            "check_runs": [
                {"name":"Lint","status":"completed","conclusion":"success","html_url":"https://x/1","started_at":"2024-01-01T10:00:00Z","completed_at":"2024-01-01T10:00:30Z"},
                {"name":"Test","status":"completed","conclusion":"failure","html_url":"https://x/2","started_at":"2024-01-01T10:00:00Z","completed_at":"2024-01-01T10:01:00Z"},
                {"name":"Deploy","status":"in_progress","conclusion":null}
            ]
        }"#;
        let result = branch_ci_summary(&array(json, "check_runs"), &[]).unwrap();
        assert_eq!(result.status, crate::CiStatus::Failure);
        assert_eq!(result.passed, 1);
        assert_eq!(result.failed, 1);
        assert_eq!(result.pending, 1);
        assert_eq!(result.total, 3);
        assert_eq!(result.checks.len(), 3);
        assert_eq!(result.checks[0].link.as_deref(), Some("https://x/1"));
        assert_eq!(result.checks[0].elapsed_ms, 30_000);
    }

    #[test]
    fn branch_ci_skipped_and_neutral_excluded_from_total() {
        let json = r#"{
            "check_runs": [
                {"name":"A","status":"completed","conclusion":"success"},
                {"name":"B","status":"completed","conclusion":"skipped"},
                {"name":"C","status":"completed","conclusion":"neutral"}
            ]
        }"#;
        let result = branch_ci_summary(&array(json, "check_runs"), &[]).unwrap();
        assert_eq!(result.status, crate::CiStatus::Success);
        assert_eq!(result.passed, 1);
        assert_eq!(result.total, 1);
        // Skipped/neutral still appear in the per-check list, marked as skipped.
        assert_eq!(result.checks.len(), 3);
        assert!(result.checks.iter().filter(|c| c.is_skipped).count() == 2);
    }

    #[test]
    fn branch_ci_statuses_only() {
        let json = r#"{
            "state": "success",
            "statuses": [
                {"context":"vercel/deploy","state":"success","target_url":"https://v/1","description":"ok","created_at":"2024-01-01T10:00:00Z","updated_at":"2024-01-01T10:00:42Z"},
                {"context":"netlify","state":"pending"}
            ]
        }"#;
        let result = branch_ci_summary(&[], &array(json, "statuses")).unwrap();
        assert_eq!(result.status, crate::CiStatus::Pending);
        assert_eq!(result.passed, 1);
        assert_eq!(result.pending, 1);
        assert_eq!(result.total, 2);
        assert_eq!(result.checks[0].name, "vercel/deploy");
        assert_eq!(result.checks[0].elapsed_ms, 42_000);
    }

    #[test]
    fn branch_ci_combines_runs_and_statuses() {
        let runs =
            r#"{"check_runs":[{"name":"Lint","status":"completed","conclusion":"success"}]}"#;
        let statuses = r#"{"statuses":[{"context":"vercel/deploy","state":"failure"}]}"#;
        let result =
            branch_ci_summary(&array(runs, "check_runs"), &array(statuses, "statuses")).unwrap();
        assert_eq!(result.status, crate::CiStatus::Failure);
        assert_eq!(result.passed, 1);
        assert_eq!(result.failed, 1);
        assert_eq!(result.total, 2);
        assert_eq!(result.checks.len(), 2);
    }

    #[test]
    fn branch_ci_both_empty_returns_none() {
        assert!(branch_ci_summary(&[], &[]).is_none());
    }

    #[test]
    fn branch_ci_only_skipped_returns_none() {
        let runs = r#"{"check_runs":[{"name":"A","status":"completed","conclusion":"skipped"}]}"#;
        assert!(branch_ci_summary(&array(runs, "check_runs"), &[]).is_none());
    }

    #[test]
    fn unchanged_upstream_commit_skips_the_lookup_entirely() {
        // A repo whose upstream still points at the cached commit must not
        // touch the API at all — this is the skip that keeps the poller
        // inside GitHub's hourly budget.
        let (_bare_tmp, bare) = {
            let tmp = tempfile::tempdir().expect("create temp dir");
            let path = tmp.path().join("origin.git");
            std::process::Command::new("git")
                .args(["init", "--bare", "-b", "main"])
                .arg(&path)
                .output()
                .expect("git init --bare");
            (tmp, path)
        };
        let (_tmp, repo) = super::super::test_support::init_temp_repo();
        super::super::test_support::git_in(
            &repo,
            &["remote", "add", "origin", bare.to_str().expect("path")],
        );
        super::super::test_support::git_in(&repo, &["push", "-u", "origin", "main"]);

        let sha = super::get_pushed_sha(&repo).expect("branch has an upstream");
        assert_eq!(
            super::fetch_ci_checks(&repo, None, Some(&sha)),
            super::CiFetch::Unchanged
        );
    }

    #[test]
    fn branch_without_upstream_never_reaches_the_api() {
        // Nothing is pushed, so there is nothing CI could have run on.
        let (_tmp, repo) = super::super::test_support::init_temp_repo();
        assert_eq!(
            super::fetch_ci_checks(&repo, None, None),
            super::CiFetch::Fetched {
                sha: None,
                summary: None
            }
        );
    }

    fn branch_node(owner: &str, repo: &str, oid: &str, number: u32) -> super::BranchPrNode {
        serde_json::from_value(serde_json::json!({
            "url": format!("https://github.com/me/okena/pull/{number}"),
            "state": "OPEN",
            "isDraft": false,
            "number": number,
            "headRefOid": oid,
            "headRepositoryOwner": { "login": owner },
            "headRepository": { "name": repo },
        }))
        .expect("branch node")
    }

    fn origin() -> super::GithubRepo {
        super::GithubRepo {
            host: "github.com".into(),
            owner: "me".into(),
            name: "okena".into(),
        }
    }

    #[test]
    fn a_forks_pr_on_the_same_branch_name_is_not_claimed() {
        let fork_only = vec![branch_node("someone", "okena", "abc", 12)];
        assert!(super::pick_branch_pr(fork_only, &origin(), &[]).is_none());

        // The fork's PR is newer, but only ours is ours.
        let both = vec![
            branch_node("someone", "okena", "abc", 12),
            branch_node("Me", "Okena", "abc", 7),
        ];
        assert_eq!(
            super::pick_branch_pr(both, &origin(), &[]).map(|pr| pr.number),
            Some(7)
        );
    }

    #[test]
    fn a_pr_whose_head_is_not_the_branch_tip_is_not_claimed() {
        let tips = vec!["def".to_string()];
        let stale = vec![branch_node("me", "okena", "abc", 7)];
        assert!(super::pick_branch_pr(stale, &origin(), &tips).is_none());

        let both = vec![
            branch_node("me", "okena", "abc", 7),
            branch_node("me", "okena", "def", 5),
        ];
        assert_eq!(
            super::pick_branch_pr(both, &origin(), &tips).map(|pr| pr.number),
            Some(5)
        );
    }

    #[test]
    fn the_default_branch_is_never_looked_up() {
        assert!(!super::branch_lookup_allowed("main", Some("main")));
        assert!(!super::branch_lookup_allowed("  ", Some("main")));
        assert!(super::branch_lookup_allowed("chore/qbl-1-x", Some("main")));
        assert!(super::branch_lookup_allowed("fix-typo", None));
    }

    #[test]
    fn a_branch_that_is_gone_everywhere_has_no_tips() {
        let (_tmp, repo) = super::super::test_support::init_temp_repo();
        assert!(super::branch_tips(&repo, "never-existed").is_empty());
        assert_eq!(super::branch_tips(&repo, "main").len(), 1);
    }

    fn with_commits(mut node: super::BranchPrNode, oids: &[&str]) -> super::BranchPrNode {
        let nodes: Vec<_> = oids
            .iter()
            .map(|oid| serde_json::json!({ "commit": { "oid": oid } }))
            .collect();
        node.commits =
            Some(serde_json::from_value(serde_json::json!({ "nodes": nodes })).expect("commits"));
        node
    }

    #[test]
    fn a_pr_github_moved_ahead_of_the_local_tip_is_still_claimed() {
        // "Update branch", an applied suggestion, a bot push: GitHub's head is
        // newer than anything fetched locally, but it still holds the tip.
        let tips = vec!["def".to_string()];
        let ahead = with_commits(branch_node("me", "okena", "zzz", 7), &["abc", "def", "zzz"]);
        assert_eq!(
            super::pick_branch_pr(vec![ahead], &origin(), &tips).map(|pr| pr.number),
            Some(7)
        );

        // A reused branch name: an old PR never held the tip at all.
        let old = with_commits(branch_node("me", "okena", "yyy", 3), &["xxx", "yyy"]);
        assert!(super::pick_branch_pr(vec![old], &origin(), &tips).is_none());
    }

    #[test]
    fn a_renamed_or_transferred_origin_is_compared_as_github_names_it() {
        let data = serde_json::json!({
            "origin": { "owner": { "login": "new-owner" }, "name": "okena-renamed" },
        });
        let resolved = super::origin_as_github_sees_it(&data, origin());
        assert_eq!(resolved.owner, "new-owner");
        assert_eq!(resolved.name, "okena-renamed");
        assert_eq!(resolved.host, "github.com");

        let moved = vec![branch_node("new-owner", "okena-renamed", "abc", 7)];
        assert!(super::pick_branch_pr(moved, &resolved, &[]).is_some());

        // Nothing resolved: the remote URL's own reading stands.
        assert_eq!(
            super::origin_as_github_sees_it(&serde_json::json!({ "origin": null }), origin()),
            origin()
        );
    }
}
