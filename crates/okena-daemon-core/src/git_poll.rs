//! GPUI-free git-status polling for the headless daemon.
//!
//! Projects in a client's declared viewport (`SetVisibleProjects`), or owning a
//! terminal streamed by a client that never declared one, stay on the
//! responsive tier: HEAD every 250ms and full status every 5s. A declared
//! viewport is the whole truth for that client — the desktop subscribes to
//! every terminal it mirrors, so its subscriptions say nothing about what is
//! on screen — and any declared viewport supersedes the workspace's own window
//! state: visibility is client-owned (`window-layout.json`), so the daemon's
//! copy is a stale legacy set, consulted only while nobody has declared one.
//! Everything else uses bounded fallback cadences (2s HEAD, 30s full status).
//! Explicit actions and detected HEAD changes still trigger an immediate
//! targeted refresh. Cached statuses for projects not selected in a cycle
//! remain published, so tiering changes freshness rather than visibility.
//!
//! The GitHub PR/CI fan-out is deliberately *narrower* than the local tier: it
//! covers only that visible set (plus explicitly requested ones), is scheduled
//! per project by [`GithubPollSchedule`], skips any project whose upstream
//! commit hasn't moved since its last settled result, and parks itself when
//! GitHub reports the API rate limit as exhausted.
//!
//! Two exceptions widen it, both feeding an agent session's PRODUCED list: a
//! worktree linked to the session's task has its PR and checks polled while
//! hidden, on the same per-project schedule (once merged or closed, its PR
//! slowly and its checks not at all);
//! and an open PR whose worktree was removed is handed to that session and
//! polled PR-only, by repo and number, on the settled PR cadence until it is
//! merged or closed.
//!
//! A third also feeds that list: a branch the session's agent pushed — seen
//! through its hooks, from a worktree it made itself or its own checkout — is
//! looked up by branch in the repository's main checkout, until a PR is found
//! and tracked by number like any other.
//!
//! A fourth feeds the project info panel: every github.com repository behind a
//! project has its open pull requests listed — all of them, from anyone —
//! whether or not anything shows it, on the settled PR cadence. The list is
//! keyed by repository, so a repo open as a project and as its worktrees is
//! asked once, and each result replaces the list on every one of them.
//!
//! A PR's mergeability and review threads ride on the PR request itself, which
//! runs on the settled PR cadence whatever the commit, so they cost nothing
//! extra and a conflict or a resolved thread shows within a cadence. The checks
//! request keeps its head-commit skip unconditionally.
//!
//! What that costs: each linked worktree adds one PR request per PR cadence
//! (~60s), plus its checks on their own cadence — none while its pushed commit
//! holds — until it is removed, whether or not its session is still running —
//! the poller cannot see that. Once its PR is merged or closed that drops to
//! one PR request every ~10 minutes, in case another PR replaces it, and no
//! checks requests until one does. Each open tracked PR
//! adds one request per cadence until it closes, or until GitHub has said
//! often enough that it does not exist; no answer at all never drops it. A
//! worktree removed before its PR was seen costs one lookup by branch, retried
//! a cadence apart, and a PR link an agent registers that nothing here knows
//! costs one lookup by number. Merged and closed tracked PRs are kept on their
//! session, never polled again, for as long as it exists. Each repository's list costs one
//! request per page of 25 open PRs per PR cadence, plus one for any PR with
//! more than 100 checks. All of it shares the fan-out's concurrency limit and
//! rate-limit gate.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use okena_core::api::ApiGitStatus;
use okena_core::git_poll::{GitPollTrigger, GithubPollSchedule};
use okena_core::harness::TrackedPullRequest;
use okena_core::process::{Lane, with_lane};
use okena_core::session_assets::{normalize_url, same_url};
use okena_git::repository::{CiFetch, PrFetch, RepoPrsFetch};
use okena_git::{self as git, GitStatus, HeadSnapshot};
use okena_workspace::state::Workspace;
use parking_lot::Mutex;
use tokio::sync::{Semaphore, mpsc, watch};

use crate::workspace_cx::DaemonWorkspaceCx;

/// Responsive full-status cadence for projects on the responsive tier.
const GIT_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Hidden projects receive a full fallback scan every 6 responsive cycles (30s).
const HIDDEN_GIT_POLL_EVERY_N_CYCLES: u64 = 6;
/// Responsive HEAD cadence for projects on the responsive tier.
const HEAD_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Hidden projects receive a cheap HEAD fallback scan every 8 ticks (2s).
const HIDDEN_HEAD_POLL_EVERY_N_TICKS: u64 = 8;
/// How many projects the GitHub fan-out talks to at once. Going wider only
/// parks more blocking-pool threads on the network; going narrower (the
/// previous strictly sequential loop) made a full pass outlast its own cadence
/// and let passes pile up on top of each other.
const GH_FANOUT_CONCURRENCY: usize = 4;

/// Project the local [`GitStatus`] onto the slimmer wire type pushed to remote
/// clients. GPUI-free reimplementation of `okena-views-git`'s `to_api`.
fn to_api(s: &GitStatus) -> ApiGitStatus {
    ApiGitStatus {
        branch: s.branch.clone(),
        lines_added: s.lines_added,
        lines_removed: s.lines_removed,
        pr_info: s.pr_info.clone(),
        ci_checks: s.ci_checks.clone(),
        ahead: s.ahead,
        behind: s.behind,
        unpushed: s.unpushed,
        review_base: s.review_base.clone(),
        default_branch: s.default_branch.clone(),
        repo_pull_requests: s.repo_pull_requests.clone(),
    }
}

#[derive(Default)]
struct TriggerAccumulator {
    /// HEAD changed locally; invalidates in-flight results from the old commit.
    head_change_ids: HashSet<String>,
    /// Unconditional GitHub refreshes. Used when existing PR/CI cache is invalid.
    force_gh_ids: HashSet<String>,
    /// Conditional refreshes. These become forced only if PR/CI cache is absent.
    candidate_gh_ids: HashSet<String>,
    /// Projects whose cached PR/CI belongs to a previous branch.
    invalidate_gh_ids: HashSet<String>,
    /// Agent sessions that registered an asset; their worktrees' PRs are due.
    session_asset_ids: HashSet<String>,
}

impl TriggerAccumulator {
    fn record(&mut self, trigger: GitPollTrigger) {
        let Some(project_id) = trigger.project_id else {
            return;
        };
        if trigger.linked_worktrees {
            self.session_asset_ids.insert(project_id);
            return;
        }
        if trigger.invalidate_github {
            self.invalidate_gh_ids.insert(project_id.clone());
            self.force_gh_ids.insert(project_id);
        } else if trigger.poll_github {
            self.candidate_gh_ids.insert(project_id);
        } else {
            self.head_change_ids.insert(project_id);
        }
    }

    fn local_status_ids(&self) -> HashSet<String> {
        self.head_change_ids
            .iter()
            .chain(&self.force_gh_ids)
            .chain(&self.candidate_gh_ids)
            .cloned()
            .collect()
    }

    fn clear(&mut self) {
        self.head_change_ids.clear();
        self.force_gh_ids.clear();
        self.candidate_gh_ids.clear();
        self.invalidate_gh_ids.clear();
        self.session_asset_ids.clear();
    }
}

/// A message from a running GitHub pass back to the poll loop.
enum GithubPassMessage {
    /// One project's outcome, sent the moment that project finishes. A pass
    /// used to publish nothing until its slowest repo returned, so a 0.4s PR
    /// lookup could sit behind another project's 15s request timeout.
    Project(GithubPollResult),
    /// A removed worktree's PR, under its schedule key.
    TrackedPr { key: String, fetch: PrFetch },
    /// A removed worktree's branch, looked up once under its key.
    RemovedBranch { key: String, fetch: PrFetch },
    /// A registered PR link, looked up by number under its key, with the
    /// checkout it was looked up through.
    RegisteredPr {
        key: String,
        fetch: PrFetch,
        found: Option<RegisteredFound>,
    },
    /// A repository's open pull requests, under its schedule key.
    RepoPrs { key: String, fetch: RepoPrsFetch },
    /// A branch a session's agent pushed, looked up by branch under its key.
    PushedBranch { key: String, fetch: PrFetch },
    /// The pass is over; carries the ids it held so they can be polled again.
    Finished(HashSet<String>),
}

struct GithubPollResult {
    head_generations: HashMap<String, u64>,
    branches: HashMap<String, Option<String>>,
    pr_infos: HashMap<String, Option<git::PrInfo>>,
    ci: HashMap<String, CiFetch>,
    /// GitHub refused at least one call because the rate limit is exhausted.
    rate_limited: bool,
    /// At least one call actually reached GitHub, so the rate-limit backoff can
    /// be cleared. A pass of nothing but cache hits proves nothing.
    reached_github: bool,
}

/// One project's slot in a GitHub pass.
struct ProjectPoll {
    id: String,
    path: String,
    want_pr: bool,
    want_ci: bool,
    /// Upstream commit whose CI result the poller already holds; the fetch is
    /// skipped while the branch still points at it.
    ci_skip_sha: Option<String>,
    /// PR number from a previous pass, used when this pass isn't re-fetching it.
    cached_pr_number: Option<u32>,
    /// A removed worktree's PR, fetched by this number from the repo at `path`
    /// rather than by the branch checked out there.
    tracked_pr: Option<u32>,
    /// A worktree removed before its PR was seen: this branch is looked up
    /// once, in the repo at `path`.
    removed_branch: Option<String>,
    /// A PR link an agent registered that nothing known covers: looked up by
    /// number through whichever checkout is in its repository.
    registered_pr: Option<RegisteredTarget>,
    /// Every open PR of the repository behind `path`, rather than anything
    /// about its branch.
    repo_prs: bool,
}

/// Union of declared viewports (`SetVisibleProjects`). The workspace's own window
/// state counts only while no connection has declared one: visibility is
/// client-owned (`window-layout.json`), so the daemon's copy is a stale legacy set.
fn visible_project_ids(
    workspace: &Workspace,
    remote_visible_projects: &RwLock<HashMap<u64, HashSet<String>>>,
) -> HashSet<String> {
    match remote_visible_projects.read() {
        Ok(declared) if !declared.is_empty() => declared.values().flatten().cloned().collect(),
        _ => workspace.all_visible_project_ids(),
    }
}

/// Visible projects plus any owning a terminal streamed by a connection that
/// has not declared a viewport. A declared viewport is the whole truth for that
/// connection, so its subscriptions are ignored (see the module docs).
fn streaming_project_ids(
    workspace: &Workspace,
    remote_subscribed_terminals: &RwLock<HashMap<u64, HashSet<String>>>,
    remote_visible_projects: &RwLock<HashMap<u64, HashSet<String>>>,
) -> HashSet<String> {
    let mut relevant = visible_project_ids(workspace, remote_visible_projects);
    let (Ok(subscribed), Ok(declared)) = (
        remote_subscribed_terminals.read(),
        remote_visible_projects.read(),
    ) else {
        return relevant;
    };
    let undeclared = subscribed
        .iter()
        .filter(|(connection_id, _)| !declared.contains_key(connection_id));
    for (_, terminal_ids) in undeclared {
        for terminal_id in terminal_ids {
            if let Some(project) = workspace.find_project_for_terminal(terminal_id) {
                relevant.insert(project.id.clone());
            }
        }
    }
    relevant
}

fn select_status_poll_ids(
    active_ids: &HashSet<String>,
    relevant_ids: &HashSet<String>,
    forced_ids: &HashSet<String>,
    newly_relevant_ids: &HashSet<String>,
    cadence_due: bool,
    poll_hidden: bool,
) -> HashSet<String> {
    if poll_hidden {
        return active_ids.clone();
    }

    let mut selected = HashSet::new();
    if cadence_due {
        selected.extend(relevant_ids.iter().cloned());
    }
    selected.extend(forced_ids.iter().cloned());
    selected.extend(newly_relevant_ids.iter().cloned());
    selected.retain(|id| active_ids.contains(id));
    selected
}

fn merge_status_results(
    previous: &HashMap<String, GitStatus>,
    active_ids: &HashSet<String>,
    attempted: HashMap<String, Option<GitStatus>>,
) -> HashMap<String, GitStatus> {
    let mut merged = previous.clone();
    merged.retain(|id, _| active_ids.contains(id));
    for (id, status) in attempted {
        match status {
            Some(status) if active_ids.contains(&id) => {
                merged.insert(id, status);
            }
            _ => {
                merged.remove(&id);
            }
        }
    }
    merged
}

/// Poll only each repository's symbolic HEAD and commit id, waking the full
/// status loop when either changes. This never reads the index or worktree.
pub async fn run_git_head_poll(
    workspace: Arc<Mutex<Workspace>>,
    remote_subscribed_terminals: Arc<RwLock<HashMap<u64, HashSet<String>>>>,
    remote_visible_projects: Arc<RwLock<HashMap<u64, HashSet<String>>>>,
    trigger_tx: mpsc::UnboundedSender<GitPollTrigger>,
) {
    let mut previous = HashMap::<String, HeadSnapshot>::new();
    let mut tick = 0u64;
    let mut interval = tokio::time::interval(HEAD_POLL_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;
        if trigger_tx.is_closed() {
            return;
        }

        let (projects, relevant_ids): (Vec<(String, String)>, HashSet<String>) = {
            let workspace = workspace.lock();
            let relevant = streaming_project_ids(
                &workspace,
                &remote_subscribed_terminals,
                &remote_visible_projects,
            );
            let projects = workspace
                .projects()
                .iter()
                .map(|project| (project.id.clone(), project.path.clone()))
                .collect();
            (projects, relevant)
        };
        let active_ids: HashSet<String> = projects.iter().map(|(id, _)| id.clone()).collect();
        let poll_hidden = tick.is_multiple_of(HIDDEN_HEAD_POLL_EVERY_N_TICKS);
        tick = tick.wrapping_add(1);
        let projects: Vec<_> = projects
            .into_iter()
            .filter(|(id, _)| poll_hidden || relevant_ids.contains(id))
            .collect();
        let snapshots = tokio::task::spawn_blocking(move || {
            projects
                .into_iter()
                .filter_map(|(id, path)| {
                    with_lane(Lane::Poll, || git::get_head_snapshot(Path::new(&path)))
                        .map(|snapshot| (id, snapshot))
                })
                .collect()
        })
        .await;
        let Ok(snapshots) = snapshots else {
            log::warn!("git HEAD poll task panicked");
            continue;
        };

        // `active_ids` deliberately includes unsampled hidden projects so their
        // prior snapshots survive fast-tier ticks and later changes are detected.
        for id in update_head_snapshots(&mut previous, &active_ids, snapshots) {
            if trigger_tx.send(GitPollTrigger::head_change(id)).is_err() {
                return;
            }
        }
    }
}

fn update_head_snapshots<T: PartialEq>(
    previous: &mut HashMap<String, T>,
    active_ids: &HashSet<String>,
    snapshots: HashMap<String, T>,
) -> Vec<String> {
    previous.retain(|id, _| active_ids.contains(id));
    snapshots
        .into_iter()
        .filter_map(|(id, snapshot)| {
            let changed = previous.get(&id).is_some_and(|old| old != &snapshot);
            if changed {
                previous.insert(id.clone(), snapshot);
                Some(id)
            } else {
                previous.insert(id, snapshot);
                None
            }
        })
        .collect()
}

/// Pick this cycle's GitHub slots.
///
/// Rules: a project earns a slot only if it is *visible* (or explicitly asked
/// for), only when its own schedule says it is due — one repo with running CI
/// no longer drags every other repo onto the fast cadence — and only when no
/// running pass already covers it.
///
/// `urgent_only` is set while another pass is still running. A cadence pass
/// waits its turn (passes used to stack copies of themselves), but a project
/// someone explicitly forced — a branch switch — jumps straight out rather than
/// waiting for that pass plus the next cadence tick.
///
/// A worktree linked to an agent session (`linked_ids`) earns a slot even
/// while hidden, so the session's PRODUCED list sees its PR and checks. Its
/// checks stay on their own schedule, skipped while the pushed commit holds.
/// Once its PR is merged or closed the PR is only watched in case another
/// replaces it, so that slot comes round every
/// `FINISHED_LINKED_PR_EVERY_N_CYCLES` and asks for no checks at all. The CI
/// summary last fetched is kept, not cleared: nothing shows it for a finished
/// PR, and checks resume once the PR poll finds an open or draft PR.
///
/// A PR-only force (`force_pr`) asks for the PR on the next pass without
/// making a hidden project count as shown, so it never costs a CI request.
#[allow(clippy::too_many_arguments)]
fn select_github_polls(
    projects: &[(String, String)],
    visible_ids: &HashSet<String>,
    linked_ids: &HashSet<String>,
    schedule: &GithubPollSchedule,
    pr_infos: &HashMap<String, Option<git::PrInfo>>,
    cycle: u64,
    cadence_due: bool,
    in_flight: &HashSet<String>,
    urgent_only: bool,
) -> Vec<ProjectPoll> {
    let forced = |id: &str| schedule.is_urgent(id) || schedule.is_pr_urgent(id);
    projects
        .iter()
        .filter(|(id, _)| visible_ids.contains(id) || forced(id) || linked_ids.contains(id))
        .filter(|(id, _)| !in_flight.contains(id))
        .filter(|(id, _)| !urgent_only || forced(id))
        .filter_map(|(id, path)| {
            let shown = visible_ids.contains(id) || schedule.is_urgent(id);
            let finished_hidden = !shown && pr_finished(pr_infos, id);
            let want_pr = if finished_hidden {
                schedule.pr_due_every(id, cycle, cadence_due, FINISHED_LINKED_PR_EVERY_N_CYCLES)
            } else {
                schedule.pr_due(id, cycle, cadence_due)
            };
            // A finished PR's checks show nowhere. Left undispatched, they come
            // due the moment the slow PR poll finds an open PR again.
            let want_ci = !finished_hidden && schedule.ci_due(id, cycle, cadence_due);
            (want_pr || want_ci).then(|| ProjectPoll {
                id: id.clone(),
                path: path.clone(),
                want_pr,
                want_ci,
                ci_skip_sha: schedule.ci_skip_sha(id, cycle),
                cached_pr_number: pr_infos
                    .get(id)
                    .and_then(|pr| pr.as_ref())
                    .map(|pr| pr.number),
                tracked_pr: None,
                removed_branch: None,
                registered_pr: None,
                repo_prs: false,
            })
        })
        .collect()
}

/// Whether the PR last fetched for `id` is merged or closed.
fn pr_finished(pr_infos: &HashMap<String, Option<git::PrInfo>>, id: &str) -> bool {
    pr_infos
        .get(id)
        .and_then(Option::as_ref)
        .is_some_and(|pr| matches!(pr.state, git::PrState::Merged | git::PrState::Closed))
}

/// What one project's GitHub slot produced.
struct ProjectOutcome {
    pr: Option<PrFetch>,
    ci: Option<CiFetch>,
    /// For a registered PR: the checkout it was looked up through.
    registered: Option<RegisteredFound>,
    /// For a repository's slot: its open pull requests.
    repo_prs: Option<RepoPrsFetch>,
}

/// Run one project's PR and CI lookups back to back on a bus worker.
///
/// Paired rather than run as two separate passes so the CI call can use the PR
/// number this pass just fetched, and so a project costs one blocking task
/// instead of two.
fn poll_one_project(poll: &ProjectPoll) -> ProjectOutcome {
    with_lane(Lane::Poll, || {
        let path = Path::new(&poll.path);
        if let Some(target) = &poll.registered_pr {
            return lookup_registered_pr(target);
        }
        if poll.repo_prs {
            return ProjectOutcome {
                pr: None,
                ci: None,
                registered: None,
                repo_prs: Some(git::repository::fetch_open_pull_requests(path)),
            };
        }
        if let Some(number) = poll.tracked_pr {
            return ProjectOutcome {
                pr: Some(git::repository::fetch_pr_by_number(path, number)),
                ci: None,
                registered: None,
                repo_prs: None,
            };
        }
        if let Some(branch) = &poll.removed_branch {
            return ProjectOutcome {
                pr: Some(git::repository::fetch_pr_by_branch(path, branch)),
                ci: None,
                registered: None,
                repo_prs: None,
            };
        }
        // Repos with no GitHub remote can never have PRs or checks; skipping
        // them here keeps the whole GitHub machinery off non-GitHub projects.
        if !git::repository::has_github_remote(path) {
            return ProjectOutcome {
                pr: poll.want_pr.then_some(PrFetch::Fetched(None)),
                ci: poll.want_ci.then_some(CiFetch::Fetched {
                    sha: None,
                    summary: None,
                }),
                registered: None,
                repo_prs: None,
            };
        }

        let pr = poll.want_pr.then(|| git::repository::fetch_pr_info(path));
        let pr_number = match &pr {
            Some(PrFetch::Fetched(info)) => info.as_ref().map(|info| info.number),
            _ => poll.cached_pr_number,
        };

        // A rate-limited PR call means the CI call would only be refused too.
        let ci = if matches!(pr, Some(PrFetch::RateLimited)) {
            None
        } else {
            poll.want_ci.then(|| {
                git::repository::fetch_ci_checks(path, pr_number, poll.ci_skip_sha.as_deref())
            })
        };

        ProjectOutcome {
            pr,
            ci,
            registered: None,
            repo_prs: None,
        }
    })
}

/// Look a registered PR up by number, through whichever checkout is in the
/// repository its link names.
fn lookup_registered_pr(target: &RegisteredTarget) -> ProjectOutcome {
    let link = &target.link;
    let checkout = target.candidates.iter().find(|(_, path)| {
        git::repository::github_repo(Path::new(path)).is_some_and(|repo| link.names(&repo))
    });
    let Some((project, repo_path)) = checkout else {
        // No checkout here is in that repository: nothing to ask with, and so
        // nothing this session could have produced.
        return ProjectOutcome {
            pr: Some(PrFetch::Fetched(None)),
            ci: None,
            registered: None,
            repo_prs: None,
        };
    };
    let (fetch, head_branch) =
        git::repository::fetch_pr_by_number_with_head(Path::new(repo_path), link.number);
    ProjectOutcome {
        pr: Some(fetch),
        ci: None,
        registered: Some(RegisteredFound {
            project: project.clone(),
            repo_path: repo_path.clone(),
            head_branch,
        }),
        repo_prs: None,
    }
}

/// Run one GitHub pass, emitting each project's outcome on `result_tx` as soon as
/// that project returns and a [`GithubPassMessage::Finished`] when all have.
///
/// Streaming rather than returning one aggregate is deliberate: a GitHub round
/// trip per repo ranges from well under a second to the 15s request cap, and
/// an aggregate held every badge in the pass hostage to its slowest repo.
async fn poll_github(
    polls: Vec<ProjectPoll>,
    head_generations: HashMap<String, u64>,
    branches: HashMap<String, Option<String>>,
    result_tx: mpsc::UnboundedSender<GithubPassMessage>,
) {
    let pass_ids: HashSet<String> = polls.iter().map(|poll| poll.id.clone()).collect();

    let permits = Arc::new(Semaphore::new(GH_FANOUT_CONCURRENCY));
    let mut tasks = tokio::task::JoinSet::new();
    for poll in polls {
        let permits = permits.clone();
        tasks.spawn(async move {
            // Bounded so a large workspace doesn't queue dozens of blocking
            // tasks that all end up waiting on the same four bus workers.
            let _permit = permits.acquire_owned().await;
            let id = poll.id.clone();
            let outcome = tokio::task::spawn_blocking(move || poll_one_project(&poll)).await;
            (id, outcome)
        });
    }

    while let Some(joined) = tasks.join_next().await {
        let Ok((id, outcome)) = joined else {
            continue;
        };
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                log::warn!("GitHub poll task failed for {id}: {error}");
                continue;
            }
        };

        // A repository, not a checkout: its list is all there is.
        if id.starts_with(REPO_PRS_KEY_PREFIX) {
            if let Some(fetch) = outcome.repo_prs
                && result_tx
                    .send(GithubPassMessage::RepoPrs { key: id, fetch })
                    .is_err()
            {
                return;
            }
            continue;
        }

        // Not a checkout: no HEAD or branch to guard against, just a PR.
        if id.starts_with(TRACKED_PR_KEY_PREFIX)
            || id.starts_with(REMOVED_BRANCH_KEY_PREFIX)
            || id.starts_with(REGISTERED_PR_KEY_PREFIX)
            || id.starts_with(PUSHED_BRANCH_KEY_PREFIX)
        {
            let Some(fetch) = outcome.pr else {
                continue;
            };
            let message = if id.starts_with(TRACKED_PR_KEY_PREFIX) {
                GithubPassMessage::TrackedPr { key: id, fetch }
            } else if id.starts_with(PUSHED_BRANCH_KEY_PREFIX) {
                GithubPassMessage::PushedBranch { key: id, fetch }
            } else if id.starts_with(REMOVED_BRANCH_KEY_PREFIX) {
                GithubPassMessage::RemovedBranch { key: id, fetch }
            } else {
                GithubPassMessage::RegisteredPr {
                    key: id,
                    fetch,
                    found: outcome.registered,
                }
            };
            if result_tx.send(message).is_err() {
                return;
            }
            continue;
        }

        let mut pr_infos = HashMap::new();
        let mut ci = HashMap::new();
        let mut rate_limited = false;
        let mut reached_github = false;

        match outcome.pr {
            Some(PrFetch::Fetched(info)) => {
                reached_github = true;
                pr_infos.insert(id.clone(), info);
            }
            Some(PrFetch::RateLimited) => rate_limited = true,
            // No answer: keep the PR already known rather than read it as gone.
            Some(PrFetch::Failed) | None => {}
        }
        match outcome.ci {
            Some(CiFetch::RateLimited) => rate_limited = true,
            Some(fetch) => {
                reached_github |= matches!(fetch, CiFetch::Fetched { .. });
                ci.insert(id.clone(), fetch);
            }
            None => {}
        }

        // The staleness guard on the receiving side looks both up by id, so a
        // single-entry map carries everything this project's result needs. The
        // entries must exist even when empty — a missing branch reads as a
        // mismatch and the result would be dropped.
        let result = GithubPollResult {
            head_generations: HashMap::from([(
                id.clone(),
                head_generations.get(&id).copied().unwrap_or_default(),
            )]),
            branches: HashMap::from([(id.clone(), branches.get(&id).cloned().flatten())]),
            pr_infos,
            ci,
            rate_limited,
            reached_github,
        };
        if result_tx.send(GithubPassMessage::Project(result)).is_err() {
            return;
        }
    }

    let _ = result_tx.send(GithubPassMessage::Finished(pass_ids));
}

#[allow(clippy::too_many_arguments)]
fn apply_github_result(
    result: GithubPollResult,
    cycle: u64,
    current_head_generations: &HashMap<String, u64>,
    schedule: &mut GithubPollSchedule,
    pr_infos: &mut HashMap<String, Option<git::PrInfo>>,
    ci_checks: &mut HashMap<String, Option<git::CiCheckSummary>>,
    last: &mut HashMap<String, GitStatus>,
    git_status_tx: &watch::Sender<HashMap<String, ApiGitStatus>>,
    state_version: &watch::Sender<u64>,
) {
    let GithubPollResult {
        head_generations,
        branches,
        pr_infos: fetched_pr_infos,
        ci: fetched_ci,
        rate_limited,
        reached_github,
    } = result;

    // Guarded because results now arrive per project: without it, five refused
    // projects in one pass would double the backoff five times over instead of
    // once. One step per cycle keeps the doubling tied to elapsed time.
    if rate_limited {
        if !schedule.is_rate_limited(cycle) {
            schedule.note_rate_limited(cycle);
            log::warn!(
                "GitHub API rate limit hit; PR/CI polling paused for {} cycles",
                schedule.rate_limit_backoff_cycles()
            );
        }
    } else if reached_github {
        schedule.note_request_succeeded();
    }

    let is_current = |id: &str| {
        let expected_generation = head_generations.get(id).copied().unwrap_or_default();
        let current_generation = current_head_generations
            .get(id)
            .copied()
            .unwrap_or_default();
        let expected_branch = branches.get(id);
        let current_branch = last.get(id).map(|status| &status.branch);
        expected_generation == current_generation && expected_branch == current_branch
    };

    for (id, pr_info) in fetched_pr_infos {
        if is_current(&id) {
            schedule.record_pr(&id, cycle);
            // Readiness comes with the PR: each fetch replaces it whole.
            pr_infos.insert(id, pr_info);
        }
    }
    for (id, fetch) in fetched_ci {
        if !is_current(&id) {
            continue;
        }
        match fetch {
            CiFetch::Unchanged => schedule.record_ci_unchanged(&id, cycle),
            CiFetch::Fetched { sha, summary } => {
                let pending = summary
                    .as_ref()
                    .is_some_and(|summary| summary.status.is_pending());
                schedule.record_ci(&id, cycle, pending, sha);
                ci_checks.insert(id, summary);
            }
            // Refusals never make it this far — they set `rate_limited` instead.
            CiFetch::RateLimited => {}
        }
    }

    let mut enriched = last.clone();
    for (id, status) in &mut enriched {
        status.pr_info = pr_infos.get(id).cloned().flatten();
        status.ci_checks = ci_checks.get(id).cloned().flatten();
    }
    publish(last, &enriched, git_status_tx, state_version);
}

/// Schedule key of a removed worktree's PR. Keyed by URL, so two sessions
/// tracking the same PR share one request.
const TRACKED_PR_KEY_PREFIX: &str = "tracked-pr:";

/// Key of a removed worktree's one-shot branch lookup, by worktree id.
const REMOVED_BRANCH_KEY_PREFIX: &str = "removed-branch:";

/// Consecutive lookups of a tracked PR in which GitHub answered that it does
/// not exist before it is dropped. No answer — offline, no token, a server
/// error — never counts: the worktree is gone, so a real PR dropped over an
/// outage could never come back.
const TRACKED_PR_LOOKUP_ATTEMPTS: u32 = 10;

/// Tries at a removed worktree's branch lookup when GitHub gives no answer,
/// each a PR cadence apart so one short blip cannot use them all.
const REMOVED_BRANCH_LOOKUP_ATTEMPTS: u32 = 5;

/// How often a hidden linked worktree whose PR is merged or closed is checked
/// for a PR replacing it (~10 minutes).
const FINISHED_LINKED_PR_EVERY_N_CYCLES: u64 = 120;

/// A worktree linked to an agent session's task.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SessionLink {
    session_id: String,
    /// Repo label: the worktree's parent project.
    project: String,
    /// The parent repo's checkout, which outlives the worktree.
    repo_path: String,
    branch: Option<String>,
}

/// Every worktree linked to an agent session's task, by project id.
///
/// Linked when any task the worktree was started for is any task the session
/// covers: a session started on several picked tasks (QBL-384/390) owns the
/// worktrees of its second and later tasks as much as its first's.
fn session_links(workspace: &Workspace) -> HashMap<String, SessionLink> {
    let projects = workspace.projects();
    projects
        .iter()
        .filter_map(|p| {
            let info = p.worktree_info.as_ref()?;
            let session = projects.iter().find(|s| {
                s.id != p.id
                    && s.is_agent_session()
                    && p.linked_tasks().any(|t| s.works_on(&t.id.external_id))
            })?;
            // Without its parent there is no checkout left to reach GitHub
            // through once the worktree goes.
            let parent = workspace.project(&info.parent_project_id)?;
            Some((
                p.id.clone(),
                SessionLink {
                    session_id: session.id.clone(),
                    project: parent.name.clone(),
                    repo_path: parent.path.clone(),
                    branch: Some(info.branch_name.clone()).filter(|b| !b.is_empty()),
                },
            ))
        })
        .collect()
}

/// A removed worktree's PR, due a refresh by number.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TrackedPoll {
    key: String,
    repo_path: String,
    number: u32,
}

/// Every PR a session is tracking, once each however many sessions track it.
fn tracked_pr_polls(workspace: &Workspace) -> Vec<TrackedPoll> {
    let mut seen = HashSet::new();
    workspace
        .projects()
        .iter()
        .filter_map(|p| p.agent.as_ref())
        .flat_map(|agent| &agent.tracked_prs)
        // A merged or closed PR keeps its last state and is not polled again.
        .filter(|pr| !pr.is_finished() && seen.insert(pr.url.clone()))
        .map(|pr| TrackedPoll {
            key: format!("{TRACKED_PR_KEY_PREFIX}{}", pr.url),
            repo_path: pr.repo_path.clone(),
            number: pr.number,
        })
        .collect()
}

/// Remember the PR last seen on each linked worktree.
///
/// Kept apart from the PR cache, which is cleared the moment a status read
/// fails — and a worktree being removed fails its status read before it
/// leaves the workspace.
fn remember_linked_prs(
    known: &mut HashMap<String, (SessionLink, git::PrInfo)>,
    links: &HashMap<String, SessionLink>,
    pr_infos: &HashMap<String, Option<git::PrInfo>>,
) {
    for (id, link) in links {
        match pr_infos.get(id) {
            Some(Some(pr)) => {
                known.insert(id.clone(), (link.clone(), pr.clone()));
            }
            Some(None) => {
                known.remove(id);
            }
            None => {}
        }
    }
}

/// Record a removed worktree's PR on its session, whatever its state. An open
/// PR is refreshed until it closes; a merged or closed one stays on the
/// session's card, with its last state, for as long as the session exists.
/// Returns whether the session changed.
fn record_removed_pr(
    projects: &mut [okena_state::ProjectData],
    link: &SessionLink,
    pr: git::PrInfo,
) -> bool {
    let Some(session) = projects.iter_mut().find(|p| p.id == link.session_id) else {
        return false;
    };
    let agent = session.agent.get_or_insert_with(Default::default);
    let before = agent.tracked_prs.clone();
    match agent.tracked_prs.iter_mut().find(|t| t.url == pr.url) {
        Some(existing) => {
            existing.state = pr.state;
            existing.readiness = pr.readiness;
            existing.readiness_unavailable = pr.readiness_unavailable;
        }
        None => agent.tracked_prs.push(TrackedPullRequest {
            project: link.project.clone(),
            repo_path: link.repo_path.clone(),
            branch: link.branch.clone(),
            number: pr.number,
            url: pr.url,
            state: pr.state,
            readiness: pr.readiness,
            readiness_unavailable: pr.readiness_unavailable,
        }),
    }
    agent.tracked_prs != before
}

/// Hand the PRs of removed worktrees to their sessions. Returns whether any
/// session changed.
fn track_removed_prs(
    projects: &mut [okena_state::ProjectData],
    known: &mut HashMap<String, (SessionLink, git::PrInfo)>,
    active_ids: &HashSet<String>,
) -> bool {
    let removed: Vec<String> = known
        .keys()
        .filter(|id| !active_ids.contains(*id))
        .cloned()
        .collect();
    let mut changed = false;
    for id in removed {
        if let Some((link, pr)) = known.remove(&id) {
            changed |= record_removed_pr(projects, &link, pr);
        }
    }
    changed
}

/// Linked worktrees that left the workspace with no PR the poller had seen,
/// keyed for their one lookup by branch. The PR may have been opened moments
/// before the worktree went — the usual clean-up flow — so "never seen" is
/// not "none".
fn unseen_removed_worktrees(
    links_prev: &HashMap<String, SessionLink>,
    known: &HashMap<String, (SessionLink, git::PrInfo)>,
    active_ids: &HashSet<String>,
) -> Vec<(String, SessionLink)> {
    links_prev
        .iter()
        .filter(|(id, link)| {
            !active_ids.contains(*id) && !known.contains_key(*id) && link.branch.is_some()
        })
        .map(|(id, link)| (format!("{REMOVED_BRANCH_KEY_PREFIX}{id}"), link.clone()))
        .collect()
}

/// Apply a tracked PR's refresh to every session tracking it. Once merged or
/// closed it keeps that state on the session and is not polled again.
/// Returns whether anything changed.
fn apply_tracked_pr(
    projects: &mut [okena_state::ProjectData],
    url: &str,
    pr: &git::PrInfo,
) -> bool {
    let mut changed = false;
    for agent in projects.iter_mut().filter_map(|p| p.agent.as_mut()) {
        let before = agent.tracked_prs.clone();
        for t in agent.tracked_prs.iter_mut().filter(|t| t.url == url) {
            t.state = pr.state.clone();
            t.readiness = pr.readiness.clone();
            t.readiness_unavailable = pr.readiness_unavailable;
        }
        changed |= agent.tracked_prs != before;
    }
    changed
}

/// Forget a tracked PR nobody can answer for any more. Returns whether any
/// session changed.
fn drop_tracked_pr(projects: &mut [okena_state::ProjectData], url: &str) -> bool {
    let mut changed = false;
    for agent in projects.iter_mut().filter_map(|p| p.agent.as_mut()) {
        let before = agent.tracked_prs.len();
        agent.tracked_prs.retain(|t| t.url != url);
        changed |= agent.tracked_prs.len() != before;
    }
    changed
}

/// A removed worktree waiting for its lookup by branch.
struct RemovedLookup {
    link: SessionLink,
    /// Lookups that came back with no answer.
    attempts: u32,
}

/// Persist and broadcast a change the poller made to workspace data.
fn notify_workspace(workspace: &mut Workspace, workspace_tick: &watch::Sender<u64>) {
    let (hook_runner, hook_monitor) = (None, None);
    workspace.notify_data(&mut DaemonWorkspaceCx::new(
        workspace_tick,
        &hook_runner,
        &hook_monitor,
    ));
}

/// Note a rate-limit refusal, once per cycle as a checkout's result does.
fn note_rate_limited(schedule: &mut GithubPollSchedule, cycle: u64) {
    if !schedule.is_rate_limited(cycle) {
        schedule.note_rate_limited(cycle);
        log::warn!(
            "GitHub API rate limit hit; PR/CI polling paused for {} cycles",
            schedule.rate_limit_backoff_cycles()
        );
    }
}

/// Apply one tracked PR's refresh: the same rate-limit gate and cadence as a
/// checkout's PR, then the result onto the sessions tracking it.
///
/// Only a request that went out and came back clears the rate-limit backoff.
/// No answer changes nothing — not the session, not the count. GitHub saying
/// the PR is not there keeps what the session shows, until
/// `TRACKED_PR_LOOKUP_ATTEMPTS` such answers in a row say it is not coming
/// back and polling it forever would only cost everyone else's requests.
fn apply_tracked_pr_result(
    key: &str,
    fetch: PrFetch,
    cycle: u64,
    schedule: &mut GithubPollSchedule,
    failures: &mut HashMap<String, u32>,
    workspace: &Mutex<Workspace>,
    workspace_tick: &watch::Sender<u64>,
) {
    let Some(url) = key.strip_prefix(TRACKED_PR_KEY_PREFIX) else {
        return;
    };
    let answer = match fetch {
        PrFetch::RateLimited => {
            note_rate_limited(schedule, cycle);
            return;
        }
        PrFetch::Failed => return,
        PrFetch::Fetched(pr) => {
            schedule.note_request_succeeded();
            schedule.record_pr(key, cycle);
            pr
        }
    };
    let mut ws = workspace.lock();
    let changed = match answer {
        Some(pr) => {
            failures.remove(key);
            apply_tracked_pr(&mut ws.data.projects, url, &pr)
        }
        None => {
            let misses = failures.entry(key.to_string()).or_default();
            *misses += 1;
            if *misses < TRACKED_PR_LOOKUP_ATTEMPTS {
                return;
            }
            failures.remove(key);
            log::warn!(
                "no longer tracking {url}: GitHub reported it missing {TRACKED_PR_LOOKUP_ATTEMPTS} times in a row"
            );
            drop_tracked_pr(&mut ws.data.projects, url)
        }
    };
    if changed {
        notify_workspace(&mut ws, workspace_tick);
    }
}

/// Apply a removed worktree's lookup by branch. A PR found is recorded on its
/// session; no PR ends the lookup. No answer is tried again, a PR cadence
/// later (the dispatch already pushed its schedule on), a few times; a
/// refusal waits out the rate-limit gate.
#[allow(clippy::too_many_arguments)]
fn apply_removed_branch_result(
    key: &str,
    fetch: PrFetch,
    cycle: u64,
    schedule: &mut GithubPollSchedule,
    pending: &mut HashMap<String, RemovedLookup>,
    workspace: &Mutex<Workspace>,
    workspace_tick: &watch::Sender<u64>,
) {
    match fetch {
        PrFetch::RateLimited => note_rate_limited(schedule, cycle),
        PrFetch::Failed => {
            if let Some(lookup) = pending.get_mut(key) {
                lookup.attempts += 1;
                if lookup.attempts >= REMOVED_BRANCH_LOOKUP_ATTEMPTS {
                    pending.remove(key);
                }
            }
        }
        PrFetch::Fetched(pr) => {
            schedule.note_request_succeeded();
            let (Some(lookup), Some(pr)) = (pending.remove(key), pr) else {
                return;
            };
            let mut ws = workspace.lock();
            if record_removed_pr(&mut ws.data.projects, &lookup.link, pr) {
                notify_workspace(&mut ws, workspace_tick);
            }
        }
    }
}

/// This cycle's removed-worktree lookups. The first goes out at once (it is
/// queued with a PR-only force); a retry waits its schedule, so a wake right
/// after a failure does not burn another attempt. Never twice at once.
fn select_removed_branch_polls(
    pending: &HashMap<String, RemovedLookup>,
    schedule: &GithubPollSchedule,
    cycle: u64,
    cadence_due: bool,
    in_flight: &HashSet<String>,
) -> Vec<ProjectPoll> {
    pending
        .iter()
        .filter(|(key, _)| !in_flight.contains(*key))
        .filter(|(key, _)| schedule.pr_due(key, cycle, cadence_due))
        .filter_map(|(key, lookup)| {
            Some(ProjectPoll {
                id: key.clone(),
                path: lookup.link.repo_path.clone(),
                want_pr: true,
                want_ci: false,
                ci_skip_sha: None,
                cached_pr_number: None,
                tracked_pr: None,
                removed_branch: Some(lookup.link.branch.clone()?),
                registered_pr: None,
                repo_prs: false,
            })
        })
        .collect()
}

/// Key of a lookup by number for a PR link an agent registered, by session
/// and link.
const REGISTERED_PR_KEY_PREFIX: &str = "registered-pr:";

/// A pull request link, `https://<host>/<owner>/<repo>/pull/<number>`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PrLink {
    /// Normalised as a remote's host is, so `www.github.com` is `github.com`.
    host: String,
    owner: String,
    repo: String,
    number: u32,
}

impl PrLink {
    /// Whether this link names a PR of `repo`: same host, same `owner/name`.
    fn names(&self, repo: &git::repository::GithubRepo) -> bool {
        repo.host == self.host
            && repo.owner.eq_ignore_ascii_case(&self.owner)
            && repo.name.eq_ignore_ascii_case(&self.repo)
    }
}

/// Read a pull request link, or `None` for any other link.
fn parse_pr_link(url: &str) -> Option<PrLink> {
    let (_, rest) = url.trim().split_once("://")?;
    let mut parts = rest.trim_end_matches('/').split('/');
    let authority = parts.next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    if host.is_empty() {
        return None;
    }
    let owner = parts.next().filter(|part| !part.is_empty())?;
    let repo = parts.next().filter(|part| !part.is_empty())?;
    if parts.next()? != "pull" {
        return None;
    }
    let number = parts.next()?.split(['#', '?']).next()?.parse().ok()?;
    Some(PrLink {
        host: git::repository::normalize_github_host(host),
        owner: owner.to_string(),
        repo: repo.to_string(),
        number,
    })
}

/// What a registered-PR lookup needs on the blocking pool: the PR, and the
/// repo checkouts one of which is in its repository.
#[derive(Clone, Debug)]
struct RegisteredTarget {
    link: PrLink,
    /// `(label, path)` of every repo checkout in the workspace.
    candidates: Vec<(String, String)>,
}

/// The checkout a registered PR was looked up through, and its head branch.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RegisteredFound {
    project: String,
    repo_path: String,
    head_branch: Option<String>,
}

/// A PR link an agent registered that nothing okena knows covers, waiting for
/// its lookup by number.
struct RegisteredLookup {
    session_id: String,
    url: String,
    target: RegisteredTarget,
    /// Lookups that came back with no answer.
    attempts: u32,
}

fn registered_key(lookup: &RegisteredLookup) -> String {
    format!(
        "{REGISTERED_PR_KEY_PREFIX}{}|{}",
        lookup.session_id,
        normalize_url(&lookup.url)
    )
}

/// PR links the agents of `session_ids` registered that nothing known covers:
/// no live worktree's PR, no tracked PR, no tombstone.
///
/// Such a link is a PR whose worktree went before okena saw it — merged, the
/// checkout removed, the link registered at wrap-up — or one whose record was
/// pruned before it was registered. Nothing else would ever look at it, so it
/// would sit in the list with no state for good.
fn unknown_registered_prs(
    workspace: &Workspace,
    session_ids: &HashSet<String>,
    pr_infos: &HashMap<String, Option<git::PrInfo>>,
    links: &HashMap<String, SessionLink>,
) -> Vec<RegisteredLookup> {
    let candidates: Vec<(String, String)> = workspace
        .projects()
        .iter()
        .filter(|p| p.worktree_info.is_none() && !p.is_any_agent_session())
        .map(|p| (p.name.clone(), p.path.clone()))
        .collect();
    let mut unknown = Vec::new();
    for session in workspace
        .projects()
        .iter()
        .filter(|p| session_ids.contains(&p.id))
    {
        let Some(agent) = session.agent.as_ref() else {
            continue;
        };
        let live: Vec<&str> = links
            .iter()
            .filter(|(_, link)| link.session_id == session.id)
            .filter_map(|(id, _)| pr_infos.get(id)?.as_ref())
            .map(|pr| pr.url.as_str())
            .collect();
        for asset in &agent.assets {
            let Some(url) = asset.url.as_deref() else {
                continue;
            };
            let Some(link) = parse_pr_link(url) else {
                continue;
            };
            let known = live.iter().any(|live| same_url(live, url))
                || agent.tracked_prs.iter().any(|t| same_url(&t.url, url));
            if known {
                continue;
            }
            unknown.push(RegisteredLookup {
                session_id: session.id.clone(),
                url: url.to_string(),
                target: RegisteredTarget {
                    link,
                    candidates: candidates.clone(),
                },
                attempts: 0,
            });
        }
    }
    unknown
}

/// Apply a registered PR's lookup by number. A PR one of the session's live
/// worktrees is on is left to that worktree's own detection; any other is
/// recorded like a removed worktree's — open tracked, merged or closed kept as
/// a tombstone, since the asset names it. No answer is tried again a cadence
/// later, a few times; a refusal waits out the rate-limit gate.
#[allow(clippy::too_many_arguments)]
fn apply_registered_pr_result(
    key: &str,
    fetch: PrFetch,
    found: Option<RegisteredFound>,
    cycle: u64,
    schedule: &mut GithubPollSchedule,
    pending: &mut HashMap<String, RegisteredLookup>,
    links: &HashMap<String, SessionLink>,
    workspace: &Mutex<Workspace>,
    workspace_tick: &watch::Sender<u64>,
) {
    match fetch {
        PrFetch::RateLimited => note_rate_limited(schedule, cycle),
        PrFetch::Failed => {
            if let Some(lookup) = pending.get_mut(key) {
                lookup.attempts += 1;
                if lookup.attempts >= REMOVED_BRANCH_LOOKUP_ATTEMPTS {
                    pending.remove(key);
                }
            }
        }
        PrFetch::Fetched(pr) => {
            // With no checkout in its repository, no request went out.
            if found.is_some() {
                schedule.note_request_succeeded();
            }
            let Some(lookup) = pending.remove(key) else {
                return;
            };
            let (Some(pr), Some(found)) = (pr, found) else {
                return;
            };
            let covered = found.head_branch.as_deref().is_some_and(|head| {
                links.values().any(|link| {
                    link.session_id == lookup.session_id
                        && link.repo_path == found.repo_path
                        && link.branch.as_deref() == Some(head)
                })
            });
            if covered {
                return;
            }
            let link = SessionLink {
                session_id: lookup.session_id,
                project: found.project,
                repo_path: found.repo_path,
                branch: found.head_branch,
            };
            let mut ws = workspace.lock();
            if record_removed_pr(&mut ws.data.projects, &link, pr) {
                notify_workspace(&mut ws, workspace_tick);
            }
        }
    }
}

/// This cycle's registered-PR lookups, like the removed-worktree ones: the
/// first at once, retries on their schedule, never twice at once.
fn select_registered_pr_polls(
    pending: &HashMap<String, RegisteredLookup>,
    schedule: &GithubPollSchedule,
    cycle: u64,
    cadence_due: bool,
    in_flight: &HashSet<String>,
) -> Vec<ProjectPoll> {
    pending
        .iter()
        .filter(|(key, _)| !in_flight.contains(*key))
        .filter(|(key, _)| schedule.pr_due(key, cycle, cadence_due))
        .map(|(key, lookup)| ProjectPoll {
            id: key.clone(),
            path: String::new(),
            want_pr: true,
            want_ci: false,
            ci_skip_sha: None,
            cached_pr_number: None,
            tracked_pr: None,
            removed_branch: None,
            registered_pr: Some(lookup.target.clone()),
            repo_prs: false,
        })
        .collect()
}

/// Pick this cycle's tracked-PR slots: settled PR cadence only, never urgent.
fn select_tracked_pr_polls(
    tracked: &[TrackedPoll],
    schedule: &GithubPollSchedule,
    cycle: u64,
    cadence_due: bool,
    in_flight: &HashSet<String>,
) -> Vec<ProjectPoll> {
    tracked
        .iter()
        .filter(|t| !in_flight.contains(&t.key))
        .filter(|t| schedule.pr_due(&t.key, cycle, cadence_due))
        .map(|t| ProjectPoll {
            id: t.key.clone(),
            path: t.repo_path.clone(),
            want_pr: true,
            want_ci: false,
            ci_skip_sha: None,
            cached_pr_number: None,
            tracked_pr: Some(t.number),
            removed_branch: None,
            registered_pr: None,
            repo_prs: false,
        })
        .collect()
}

/// Schedule key prefix of a branch a session's agent pushed, by session, repo
/// and branch.
const PUSHED_BRANCH_KEY_PREFIX: &str = "pushed-branch:";

/// A branch an agent session pushed, due a lookup by branch.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PushedPoll {
    key: String,
    link: SessionLink,
    /// Every PR recorded for this branch is merged or closed: the branch is
    /// only watched, slowly, for a PR that replaces it.
    finished: bool,
}

/// Every branch a session's agent pushed that nothing else looks after: not a
/// live linked worktree's (that worktree's own poll covers it), and with no
/// open PR the session already tracks (tracked PRs refresh by number).
///
/// This is what reaches a worktree the agent made itself or a branch in the
/// session's own checkout, which no task links — and, since the branch is
/// looked up in the repository's main checkout, it keeps working after that
/// checkout is gone.
fn pushed_branch_polls(
    workspace: &Workspace,
    links: &HashMap<String, SessionLink>,
) -> Vec<PushedPoll> {
    let mut out = Vec::new();
    for session in workspace.projects() {
        let Some(agent) = session.agent.as_ref() else {
            continue;
        };
        for pushed in &agent.pushed_branches {
            let on_branch = |repo_path: &str, branch: Option<&str>| {
                repo_path == pushed.repo_path && branch == Some(pushed.branch.as_str())
            };
            let live = links.values().any(|link| {
                link.session_id == session.id
                    && on_branch(&link.repo_path, link.branch.as_deref())
            });
            let tracked: Vec<&TrackedPullRequest> = agent
                .tracked_prs
                .iter()
                .filter(|t| on_branch(&t.repo_path, t.branch.as_deref()))
                .collect();
            if live || tracked.iter().any(|t| !t.is_finished()) {
                continue;
            }
            out.push(PushedPoll {
                key: format!(
                    "{PUSHED_BRANCH_KEY_PREFIX}{}|{}|{}",
                    session.id, pushed.repo_path, pushed.branch
                ),
                link: SessionLink {
                    session_id: session.id.clone(),
                    project: pushed.project.clone(),
                    repo_path: pushed.repo_path.clone(),
                    branch: Some(pushed.branch.clone()),
                },
                finished: !tracked.is_empty(),
            });
        }
    }
    out
}

/// This cycle's pushed-branch lookups: on the PR cadence, or every
/// `FINISHED_LINKED_PR_EVERY_N_CYCLES` once the branch's PR has finished;
/// at once when the session's hook asked; never twice at once.
fn select_pushed_branch_polls(
    pushed: &[PushedPoll],
    schedule: &GithubPollSchedule,
    cycle: u64,
    cadence_due: bool,
    in_flight: &HashSet<String>,
) -> Vec<ProjectPoll> {
    pushed
        .iter()
        .filter(|p| !in_flight.contains(&p.key))
        .filter(|p| {
            if p.finished {
                schedule.pr_due_every(
                    &p.key,
                    cycle,
                    cadence_due,
                    FINISHED_LINKED_PR_EVERY_N_CYCLES,
                )
            } else {
                schedule.pr_due(&p.key, cycle, cadence_due)
            }
        })
        .map(|p| ProjectPoll {
            id: p.key.clone(),
            path: p.link.repo_path.clone(),
            want_pr: true,
            want_ci: false,
            ci_skip_sha: None,
            cached_pr_number: None,
            tracked_pr: None,
            removed_branch: p.link.branch.clone(),
            registered_pr: None,
            repo_prs: false,
        })
        .collect()
}

/// Apply a pushed branch's lookup: a PR found is recorded on its session, to
/// be refreshed by number from then on and kept once it finishes. No PR yet
/// waits for the next cadence; no answer changes nothing; a refusal waits out
/// the rate-limit gate.
fn apply_pushed_branch_result(
    key: &str,
    fetch: PrFetch,
    cycle: u64,
    schedule: &mut GithubPollSchedule,
    pushed: &[PushedPoll],
    workspace: &Mutex<Workspace>,
    workspace_tick: &watch::Sender<u64>,
) {
    match fetch {
        PrFetch::RateLimited => note_rate_limited(schedule, cycle),
        PrFetch::Failed => {}
        PrFetch::Fetched(pr) => {
            schedule.note_request_succeeded();
            schedule.record_pr(key, cycle);
            let (Some(pr), Some(poll)) = (pr, pushed.iter().find(|p| p.key == key)) else {
                return;
            };
            let mut ws = workspace.lock();
            if record_removed_pr(&mut ws.data.projects, &poll.link, pr) {
                notify_workspace(&mut ws, workspace_tick);
            }
        }
    }
}

/// Schedule key prefix of a repository's open-PR list; the rest is the
/// repository's [`git::repository::github_repo_key`].
const REPO_PRS_KEY_PREFIX: &str = "repo-prs:";

/// Every github.com repository behind the workspace's projects, by schedule
/// key, with the checkout to ask through: the first in workspace order, so a
/// repository open as a project and as several worktrees is asked once.
fn repo_pr_targets(
    projects: &[(String, String)],
    repo_keys: &HashMap<String, Option<String>>,
) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    projects
        .iter()
        .filter_map(|(id, path)| {
            let repo = repo_keys.get(id)?.as_ref()?;
            seen.insert(repo.clone())
                .then(|| (format!("{REPO_PRS_KEY_PREFIX}{repo}"), path.clone()))
        })
        .collect()
}

/// This cycle's repository list slots: on the settled PR cadence, whether or
/// not anything shows the repository, and never twice at once.
fn select_repo_pr_polls(
    targets: &[(String, String)],
    schedule: &GithubPollSchedule,
    cycle: u64,
    cadence_due: bool,
    in_flight: &HashSet<String>,
) -> Vec<ProjectPoll> {
    targets
        .iter()
        .filter(|(key, _)| !in_flight.contains(key))
        .filter(|(key, _)| schedule.pr_due(key, cycle, cadence_due))
        .map(|(key, path)| ProjectPoll {
            id: key.clone(),
            path: path.clone(),
            want_pr: true,
            want_ci: false,
            ci_skip_sha: None,
            cached_pr_number: None,
            tracked_pr: None,
            removed_branch: None,
            registered_pr: None,
            repo_prs: true,
        })
        .collect()
}

/// The open PRs project `id` shows: its repository's list, when it has a
/// github.com repository and the list has been fetched.
fn repo_list_for(
    id: &str,
    repo_keys: &HashMap<String, Option<String>>,
    repo_prs: &HashMap<String, Option<Vec<git::RepoPullRequest>>>,
) -> Option<Vec<git::RepoPullRequest>> {
    let repo = repo_keys.get(id)?.as_ref()?;
    repo_prs.get(repo).cloned().flatten()
}

/// Apply a repository's open-PR list: the same rate-limit gate and cadence as
/// a checkout's PR, then the list onto every project in that repository.
///
/// The list replaces the one before it whole, so a PR merged or closed since
/// is gone and one opened since is there. No answer keeps the list shown: a
/// list with a page missing would drop PRs that are still open.
#[allow(clippy::too_many_arguments)]
fn apply_repo_prs_result(
    key: &str,
    fetch: RepoPrsFetch,
    cycle: u64,
    schedule: &mut GithubPollSchedule,
    repo_keys: &HashMap<String, Option<String>>,
    repo_prs: &mut HashMap<String, Option<Vec<git::RepoPullRequest>>>,
    last: &mut HashMap<String, GitStatus>,
    git_status_tx: &watch::Sender<HashMap<String, ApiGitStatus>>,
    state_version: &watch::Sender<u64>,
) {
    let Some(repo) = key.strip_prefix(REPO_PRS_KEY_PREFIX) else {
        return;
    };
    let list = match fetch {
        RepoPrsFetch::RateLimited => {
            note_rate_limited(schedule, cycle);
            return;
        }
        RepoPrsFetch::Failed => return,
        RepoPrsFetch::Fetched(list) => {
            // With no client to ask with, no request went out.
            if list.is_some() {
                schedule.note_request_succeeded();
            }
            schedule.record_pr(key, cycle);
            list
        }
    };
    repo_prs.insert(repo.to_string(), list);
    let mut enriched = last.clone();
    for (id, status) in &mut enriched {
        status.repo_pull_requests = repo_list_for(id, repo_keys, repo_prs);
    }
    publish(last, &enriched, git_status_tx, state_version);
}

/// Run the daemon git-status poll loop until the server is gone: every `watch`
/// receiver dropped and every trigger sender dropped. A daemon no client has
/// subscribed to yet keeps polling, so a status is ready when one does.
///
/// Each cycle snapshots all local projects and their current relevance, selects
/// only due or explicitly triggered repositories, and runs their gix work on the
/// blocking pool. Results merge into the prior cache so skipped hidden projects
/// remain published. The independent wall-clock interval keeps 5s/30s deadlines
/// stable even when targeted triggers wake the loop between cadence ticks.
/// PR/CI lookups retain their existing visible-project adaptive cadence.
///
/// `workspace_tick` is for the one thing this loop writes back: the open PRs
/// of removed worktrees, which sessions keep until they close.
///
/// Bumps `state_version` on a real change so a snapshot/broadcast observer can
/// react; the *primary* output is the `git_status_tx` watch.
pub async fn run_git_poll(
    workspace: Arc<Mutex<Workspace>>,
    git_status_tx: Arc<watch::Sender<HashMap<String, ApiGitStatus>>>,
    state_version: watch::Sender<u64>,
    workspace_tick: watch::Sender<u64>,
    remote_subscribed_terminals: Arc<RwLock<HashMap<u64, HashSet<String>>>>,
    remote_visible_projects: Arc<RwLock<HashMap<u64, HashSet<String>>>>,
    mut trigger_rx: mpsc::UnboundedReceiver<GitPollTrigger>,
) {
    // Last-published per-project statuses, kept across cycles so we only
    // re-broadcast + bump on real change. Keyed by the richer `GitStatus`
    // (which derives `PartialEq`) — the GUI's `commit_statuses` compares the
    // same type. `ApiGitStatus` (the wire projection) has no `PartialEq`.
    let mut last: HashMap<String, GitStatus> = HashMap::new();

    // Across-cycle PR/CI caches keyed by project ID, mirroring the GUI watcher's
    // `pr_infos` / `ci_checks`. The expensive GitHub fan-out only runs on the
    // cadence below; between those cycles the cached values are merged into every
    // status so the badges don't blank. Merge (not replace) on update so a
    // project that drops out of the visible set keeps its last-known PR/CI.
    let mut pr_infos: HashMap<String, Option<git::PrInfo>> = HashMap::new();
    let mut ci_checks: HashMap<String, Option<git::CiCheckSummary>> = HashMap::new();
    // Per-project GitHub cadence, commit-level result caching and the rate-limit
    // gate. Replaces the old global "is anything pending?" flag, which put every
    // project on the fast cadence as soon as one repo had CI running.
    let mut schedule = GithubPollSchedule::default();
    // Projects a running GitHub pass currently holds. Passes used to be spawned
    // unconditionally, so a fan-out slower than its own cadence stacked copies
    // of itself; tracking the ids (rather than a bare flag) keeps that
    // protection while letting an explicitly forced project start its own pass
    // instead of waiting out the one in progress.
    let mut github_in_flight: HashSet<String> = HashSet::new();
    let mut cycle: u64 = 0;
    let mut trigger_acc = TriggerAccumulator::default();
    let mut known_streaming_ids: HashSet<String> = HashSet::new();
    let mut trigger_rx_closed = false;
    let mut head_generations: HashMap<String, u64> = HashMap::new();
    // Worktrees linked to agent sessions as of the last cycle, and the PR each
    // last showed — what a removed worktree hands to its session.
    let mut session_links_prev: HashMap<String, SessionLink> = HashMap::new();
    let mut known_linked_prs: HashMap<String, (SessionLink, git::PrInfo)> = HashMap::new();
    // Removed worktrees with no PR seen, each awaiting one lookup by branch;
    // and each tracked PR's lookups in a row that came back without it.
    let mut removed_lookups: HashMap<String, RemovedLookup> = HashMap::new();
    let mut tracked_failures: HashMap<String, u32> = HashMap::new();
    // PR links agents registered that nothing known covers, each awaiting one
    // lookup by number.
    let mut registered_lookups: HashMap<String, RegisteredLookup> = HashMap::new();
    // Each project's github.com repository (`None`: it has none), read with
    // its status when first seen and again on the hidden cadence; and each
    // repository's open PRs, by that key.
    let mut repo_keys: HashMap<String, Option<String>> = HashMap::new();
    let mut repo_prs: HashMap<String, Option<Vec<git::RepoPullRequest>>> = HashMap::new();
    let (github_result_tx, mut github_result_rx) = mpsc::unbounded_channel();
    // Consume `interval`'s immediate first tick. Subsequent ticks stay anchored
    // to wall time, so targeted wakes cannot postpone periodic refreshes.
    let mut cadence = tokio::time::interval(GIT_POLL_INTERVAL);
    cadence.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    cadence.tick().await;
    let mut cadence_due = true;

    loop {
        drain_git_poll_triggers(&mut trigger_rx, &mut trigger_acc, &mut trigger_rx_closed);
        for id in &trigger_acc.head_change_ids {
            // Bump the generation so in-flight results for the old commit are
            // discarded. The cached CI summary is deliberately *not* dropped:
            // checks belong to the last pushed commit, which a local commit
            // doesn't move — and dropping it both blanked the badge and forced
            // a refetch on every commit.
            *head_generations.entry(id.clone()).or_default() += 1;
        }
        clear_github_cache_for_ids(
            &trigger_acc.invalidate_gh_ids,
            &mut pr_infos,
            &mut ci_checks,
        );

        // ── 1. Snapshot relevance and choose this cycle's local work ─────────
        let (projects, visible_ids, streaming_ids, links, tracked_prs) = {
            let workspace = workspace.lock();
            let visible = visible_project_ids(&workspace, &remote_visible_projects);
            let streaming = streaming_project_ids(
                &workspace,
                &remote_subscribed_terminals,
                &remote_visible_projects,
            );
            let projects: Vec<(String, String)> = workspace
                .projects()
                .iter()
                .map(|project| (project.id.clone(), project.path.clone()))
                .collect();
            (
                projects,
                visible,
                streaming,
                session_links(&workspace),
                tracked_pr_polls(&workspace),
            )
        };
        let active_ids: HashSet<String> = projects.iter().map(|(id, _)| id.clone()).collect();

        // A linked worktree that left the workspace hands its open PR to its
        // session. Read before the caches below forget the project.
        remember_linked_prs(&mut known_linked_prs, &session_links_prev, &pr_infos);
        for (key, link) in
            unseen_removed_worktrees(&session_links_prev, &known_linked_prs, &active_ids)
        {
            if let std::collections::hash_map::Entry::Vacant(slot) =
                removed_lookups.entry(key.clone())
            {
                slot.insert(RemovedLookup { link, attempts: 0 });
                schedule.force_pr(&key);
            }
        }
        if known_linked_prs.keys().any(|id| !active_ids.contains(id)) {
            let mut ws = workspace.lock();
            if track_removed_prs(&mut ws.data.projects, &mut known_linked_prs, &active_ids) {
                notify_workspace(&mut ws, &workspace_tick);
            }
        }
        known_linked_prs.retain(|id, _| links.contains_key(id));
        let linked_ids: HashSet<String> = links.keys().cloned().collect();
        // Branches agents pushed from checkouts no task links.
        let pushed_polls = {
            let ws = workspace.lock();
            pushed_branch_polls(&ws, &links)
        };
        // An agent just registered an asset, pushed, or ended a turn: fetch
        // its worktrees' and pushed branches' PRs now, so a PR it just opened
        // shows in seconds and is matched rather than listed twice meanwhile.
        for (id, link) in &links {
            if trigger_acc.session_asset_ids.contains(&link.session_id) {
                schedule.force_pr(id);
            }
        }
        for poll in &pushed_polls {
            if trigger_acc.session_asset_ids.contains(&poll.link.session_id) {
                schedule.force_pr(&poll.key);
            }
        }
        // A PR link registered with nothing known behind it — its worktree
        // went before okena saw the PR — gets one lookup by number.
        if !trigger_acc.session_asset_ids.is_empty() {
            let unknown = {
                let ws = workspace.lock();
                unknown_registered_prs(&ws, &trigger_acc.session_asset_ids, &pr_infos, &links)
            };
            for lookup in unknown {
                let key = registered_key(&lookup);
                if let std::collections::hash_map::Entry::Vacant(slot) =
                    registered_lookups.entry(key.clone())
                {
                    slot.insert(lookup);
                    schedule.force_pr(&key);
                }
            }
        }
        session_links_prev = links;

        pr_infos.retain(|id, _| active_ids.contains(id));
        ci_checks.retain(|id, _| active_ids.contains(id));
        head_generations.retain(|id, _| active_ids.contains(id));
        known_streaming_ids.retain(|id| active_ids.contains(id));
        repo_keys.retain(|id, _| active_ids.contains(id));
        let live_repos: HashSet<String> = repo_keys.values().flatten().cloned().collect();
        repo_prs.retain(|repo, _| live_repos.contains(repo));
        // Tracked PRs, branch lookups and repository lists have no project of
        // their own; their keys keep their slot, and with it their cadence.
        let scheduled_ids: HashSet<String> = active_ids
            .iter()
            .cloned()
            .chain(tracked_prs.iter().map(|t| t.key.clone()))
            .chain(removed_lookups.keys().cloned())
            .chain(registered_lookups.keys().cloned())
            .chain(pushed_polls.iter().map(|p| p.key.clone()))
            .chain(
                live_repos
                    .iter()
                    .map(|repo| format!("{REPO_PRS_KEY_PREFIX}{repo}")),
            )
            .collect();
        schedule.retain(&scheduled_ids);
        tracked_failures.retain(|key, _| scheduled_ids.contains(key));

        let newly_relevant_ids: HashSet<String> = streaming_ids
            .difference(&known_streaming_ids)
            .cloned()
            .collect();
        known_streaming_ids = streaming_ids.clone();
        let forced_local_ids = trigger_acc.local_status_ids();
        let poll_hidden =
            cycle == 0 || (cadence_due && cycle.is_multiple_of(HIDDEN_GIT_POLL_EVERY_N_CYCLES));
        let status_poll_ids = select_status_poll_ids(
            &active_ids,
            &streaming_ids,
            &forced_local_ids,
            &newly_relevant_ids,
            cadence_due,
            poll_hidden,
        );

        // Explicit actions steer the GitHub schedule: a branch switch invalidates
        // what we hold, while merely showing a project is only worth a fetch
        // when we hold no PR/CI result for it yet.
        for id in &trigger_acc.invalidate_gh_ids {
            schedule.force(id);
        }
        for id in &trigger_acc.candidate_gh_ids {
            let has_cached_result = pr_infos.contains_key(id) && ci_checks.contains_key(id);
            schedule.force_if_unfetched(id, has_cached_result);
        }

        // ── 2. Refresh selected statuses and merge into the published cache ──
        let mut attempted: HashMap<String, Option<GitStatus>> = HashMap::new();
        for (id, path) in projects
            .iter()
            .filter(|(id, _)| status_poll_ids.contains(id))
        {
            let id = id.clone();
            let path = path.clone();
            // A remote rarely changes: read once, then with the hidden sweep.
            let resolve_repo = poll_hidden || !repo_keys.contains_key(&id);
            let status = tokio::task::spawn_blocking(move || {
                with_lane(Lane::Poll, || {
                    let path = Path::new(&path);
                    let status = git::refresh_git_status(path);
                    let repo = (resolve_repo && status.is_some())
                        .then(|| git::repository::github_repo_key(path));
                    (status, repo)
                })
            })
            .await;
            match status {
                Ok((Some(mut status), repo)) => {
                    if let Some(repo) = repo {
                        repo_keys.insert(id.clone(), repo);
                    }
                    // Inject whatever PR/CI we already have cached so a still-fresh
                    // badge doesn't blank between GitHub cadence cycles.
                    status.pr_info = pr_infos.get(&id).cloned().flatten();
                    status.ci_checks = ci_checks.get(&id).cloned().flatten();
                    status.repo_pull_requests = repo_list_for(&id, &repo_keys, &repo_prs);
                    attempted.insert(id, Some(status));
                }
                Ok((None, _)) => {
                    repo_keys.remove(&id);
                    attempted.insert(id, None);
                }
                Err(error) => {
                    // Preserve the last published value on a panicked blocking
                    // task; the next cadence or targeted trigger retries it.
                    log::error!("git status poll task panicked for {id}: {error}");
                }
            }
        }
        let missing_status_ids: HashSet<String> = attempted
            .iter()
            .filter_map(|(id, status)| status.is_none().then_some(id.clone()))
            .collect();
        clear_github_cache_for_ids(&missing_status_ids, &mut pr_infos, &mut ci_checks);
        let mut new_statuses = merge_status_results(&last, &active_ids, attempted);

        let branch_changes = branch_changed_ids(&last, &new_statuses);
        if !branch_changes.is_empty() {
            clear_github_cache_for_ids(&branch_changes, &mut pr_infos, &mut ci_checks);
            for id in &branch_changes {
                if let Some(status) = new_statuses.get_mut(id) {
                    status.pr_info = None;
                    status.ci_checks = None;
                }
                // Cached PR/CI described the branch we just left.
                schedule.force(id);
            }
        }

        // ── 3. Publish the basic status map on change — BEFORE the slow GitHub calls
        // git status comes from gix (fast, in-process); PR/CI come from the GitHub
        // API (network, and can stall). Publishing here means a stuck request can
        // never block the branch/diff badge from appearing.
        publish(&mut last, &new_statuses, &git_status_tx, &state_version);

        // Stop once the server is down: no `watch` receiver left AND no one left
        // to send triggers. Receivers alone are not enough — none exists until
        // the first client subscribes, and the first cycle routinely finishes
        // before that, which used to end polling for the daemon's lifetime.
        if git_status_tx.is_closed() && trigger_rx_closed {
            log::trace!("git poll loop exiting: no status receivers or trigger senders left");
            return;
        }

        // ── 4. Start GitHub PR/CI fan-out without blocking local git refreshes ─
        // Only visible projects (plus anything explicitly asked for) and only
        // while no pass is already running and GitHub isn't refusing us.
        if !schedule.is_rate_limited(cycle) {
            // While a pass runs, only forced projects earn a second one — a
            // branch switch shouldn't have to wait out the pass in progress
            // and then the next cadence tick on top of it.
            let urgent_only = !github_in_flight.is_empty();
            let mut polls = select_github_polls(
                &projects,
                &visible_ids,
                &linked_ids,
                &schedule,
                &pr_infos,
                cycle,
                cadence_due,
                &github_in_flight,
                urgent_only,
            );
            if !urgent_only {
                polls.extend(select_tracked_pr_polls(
                    &tracked_prs,
                    &schedule,
                    cycle,
                    cadence_due,
                    &github_in_flight,
                ));
                polls.extend(select_repo_pr_polls(
                    &repo_pr_targets(&projects, &repo_keys),
                    &schedule,
                    cycle,
                    cadence_due,
                    &github_in_flight,
                ));
            }
            // Not held back by a running pass: each goes out when it is due.
            polls.extend(select_removed_branch_polls(
                &removed_lookups,
                &schedule,
                cycle,
                cadence_due,
                &github_in_flight,
            ));
            polls.extend(select_registered_pr_polls(
                &registered_lookups,
                &schedule,
                cycle,
                cadence_due,
                &github_in_flight,
            ));
            polls.extend(select_pushed_branch_polls(
                &pushed_polls,
                &schedule,
                cycle,
                cadence_due,
                &github_in_flight,
            ));

            log::trace!(
                "GitHub poll cycle={cycle}: {} projects, {} visible, {} due",
                projects.len(),
                visible_ids.len(),
                polls.len()
            );
            if !polls.is_empty() {
                // Push each project's next due cycle forward before the pass
                // leaves, so the cycles it spans don't queue it again.
                for poll in &polls {
                    if poll.want_pr {
                        schedule.pr_dispatched(&poll.id, cycle);
                    }
                    if poll.want_ci {
                        schedule.ci_dispatched(&poll.id, cycle);
                    }
                }
                let poll_generations = polls
                    .iter()
                    .map(|poll| {
                        (
                            poll.id.clone(),
                            head_generations.get(&poll.id).copied().unwrap_or_default(),
                        )
                    })
                    .collect();
                let poll_branches = polls
                    .iter()
                    .map(|poll| {
                        (
                            poll.id.clone(),
                            new_statuses
                                .get(&poll.id)
                                .and_then(|status| status.branch.clone()),
                        )
                    })
                    .collect();
                let result_tx = github_result_tx.clone();
                github_in_flight.extend(polls.iter().map(|poll| poll.id.clone()));
                tokio::spawn(poll_github(
                    polls,
                    poll_generations,
                    poll_branches,
                    result_tx,
                ));
            }
        }

        trigger_acc.clear();
        if cadence_due {
            cycle = cycle.wrapping_add(1);
        }
        cadence_due = false;
        loop {
            tokio::select! {
                biased;
                _ = cadence.tick() => {
                    cadence_due = true;
                    break;
                }
                trigger = trigger_rx.recv(), if !trigger_rx_closed => {
                    match trigger {
                        Some(trigger) => {
                            trigger_acc.record(trigger);
                            break;
                        }
                        None => trigger_rx_closed = true,
                    }
                }
                Some(message) = github_result_rx.recv() => {
                    match message {
                        // Applied and published the moment it lands, so a badge
                        // never waits on the rest of its pass.
                        GithubPassMessage::Project(result) => apply_github_result(
                            result,
                            cycle,
                            &head_generations,
                            &mut schedule,
                            &mut pr_infos,
                            &mut ci_checks,
                            &mut last,
                            &git_status_tx,
                            &state_version,
                        ),
                        GithubPassMessage::TrackedPr { key, fetch } => apply_tracked_pr_result(
                            &key,
                            fetch,
                            cycle,
                            &mut schedule,
                            &mut tracked_failures,
                            &workspace,
                            &workspace_tick,
                        ),
                        GithubPassMessage::RemovedBranch { key, fetch } => {
                            apply_removed_branch_result(
                                &key,
                                fetch,
                                cycle,
                                &mut schedule,
                                &mut removed_lookups,
                                &workspace,
                                &workspace_tick,
                            )
                        }
                        GithubPassMessage::RegisteredPr { key, fetch, found } => {
                            apply_registered_pr_result(
                                &key,
                                fetch,
                                found,
                                cycle,
                                &mut schedule,
                                &mut registered_lookups,
                                &session_links_prev,
                                &workspace,
                                &workspace_tick,
                            )
                        }
                        GithubPassMessage::PushedBranch { key, fetch } => {
                            apply_pushed_branch_result(
                                &key,
                                fetch,
                                cycle,
                                &mut schedule,
                                &pushed_polls,
                                &workspace,
                                &workspace_tick,
                            )
                        }
                        GithubPassMessage::RepoPrs { key, fetch } => apply_repo_prs_result(
                            &key,
                            fetch,
                            cycle,
                            &mut schedule,
                            &repo_keys,
                            &mut repo_prs,
                            &mut last,
                            &git_status_tx,
                            &state_version,
                        ),
                        GithubPassMessage::Finished(ids) => {
                            for id in &ids {
                                github_in_flight.remove(id);
                            }
                            // A force this pass was covering (a branch switch
                            // detected mid-pass) is now dispatchable — go round
                            // instead of idling until the next cadence tick.
                            if schedule.has_urgent() {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
}

fn drain_git_poll_triggers(
    trigger_rx: &mut mpsc::UnboundedReceiver<GitPollTrigger>,
    trigger_acc: &mut TriggerAccumulator,
    trigger_rx_closed: &mut bool,
) {
    if *trigger_rx_closed {
        return;
    }
    loop {
        match trigger_rx.try_recv() {
            Ok(trigger) => {
                trigger_acc.record(trigger);
            }
            Err(mpsc::error::TryRecvError::Empty) => break,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                *trigger_rx_closed = true;
                break;
            }
        }
    }
}

fn branch_changed_ids(
    last: &HashMap<String, GitStatus>,
    new_statuses: &HashMap<String, GitStatus>,
) -> HashSet<String> {
    new_statuses
        .iter()
        .filter_map(|(id, status)| {
            last.get(id)
                .filter(|prev| prev.branch != status.branch)
                .map(|_| id.clone())
        })
        .collect()
}

fn clear_github_cache_for_ids(
    ids: &HashSet<String>,
    pr_infos: &mut HashMap<String, Option<git::PrInfo>>,
    ci_checks: &mut HashMap<String, Option<git::CiCheckSummary>>,
) {
    for id in ids {
        pr_infos.remove(id);
        ci_checks.remove(id);
    }
}

/// Broadcast the slimmed `ApiGitStatus` map into `git_status_tx` and bump
/// `state_version`, but only on a real change. `last` holds the previously
/// published richer `GitStatus` map (the GUI's `commit_statuses` change check);
/// no-ops when `new_statuses` equals it, so re-committing the same data is free.
fn publish(
    last: &mut HashMap<String, GitStatus>,
    new_statuses: &HashMap<String, GitStatus>,
    git_status_tx: &watch::Sender<HashMap<String, ApiGitStatus>>,
    state_version: &watch::Sender<u64>,
) {
    if new_statuses == last {
        return;
    }
    *last = new_statuses.clone();
    let api_statuses: HashMap<String, ApiGitStatus> =
        last.iter().map(|(id, s)| (id.clone(), to_api(s))).collect();
    git_status_tx.send_replace(api_statuses);
    state_version.send_modify(|v| *v += 1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::empty_workspace_data;

    /// With no projects and no external `watch` receiver, the first cycle does
    /// its empty snapshot, publishes nothing (unchanged), detects the closed
    /// channel, and the loop ends — without touching any real repository or
    /// sleeping. Exercises the snapshot → no-change → channel-closed-detection
    /// path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_git_poll_stops_when_channel_closed() {
        let workspace = Arc::new(Mutex::new(Workspace::new(empty_workspace_data())));
        let (tx, rx) = watch::channel(HashMap::<String, ApiGitStatus>::new());
        let git_status_tx = Arc::new(tx);
        let (state_version, _svrx) = watch::channel(0u64);

        // Drop the only external receiver up front so the first `is_closed()`
        // check returns immediately (no 5s sleep, deterministic).
        drop(rx);

        let subscribed = Arc::new(RwLock::new(HashMap::new()));
        let client_visible = Arc::new(RwLock::new(HashMap::new()));
        // The server holds the trigger senders; it is gone, so are they.
        let (trigger_tx, trigger_rx) = mpsc::unbounded_channel();
        drop(trigger_tx);
        run_git_poll(
            workspace,
            git_status_tx.clone(),
            state_version,
            watch::Sender::new(0),
            subscribed,
            client_visible,
            trigger_rx,
        )
        .await;

        // No projects → nothing was published; the channel holds the initial map.
        assert!(git_status_tx.borrow().is_empty());
    }

    /// The regression: a daemon starts polling before any client has
    /// subscribed to its statuses. With the server still up (it holds the
    /// trigger senders), the loop must keep going rather than read "no
    /// receivers yet" as "server gone" — which stopped git status and every
    /// GitHub fetch for the daemon's whole lifetime.
    #[tokio::test(start_paused = true)]
    async fn polling_survives_a_start_with_no_client_subscribed_yet() {
        let workspace = Arc::new(Mutex::new(Workspace::new(empty_workspace_data())));
        let git_status_tx = Arc::new(watch::Sender::new(HashMap::<String, ApiGitStatus>::new()));
        let (state_version, _svrx) = watch::channel(0u64);
        let (trigger_tx, trigger_rx) = mpsc::unbounded_channel();
        let poll = tokio::spawn(run_git_poll(
            workspace,
            git_status_tx.clone(),
            state_version,
            watch::Sender::new(0),
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(HashMap::new())),
            trigger_rx,
        ));

        // Several cadence cycles with no receiver: still polling.
        for _ in 0..4 {
            tokio::time::sleep(GIT_POLL_INTERVAL).await;
            tokio::task::yield_now().await;
        }
        assert!(!poll.is_finished(), "polling stopped before any client came");

        // The server goes away: the loop ends on its next cycle.
        drop(trigger_tx);
        tokio::time::sleep(GIT_POLL_INTERVAL * 2).await;
        tokio::time::timeout(GIT_POLL_INTERVAL * 2, poll)
            .await
            .expect("the loop ends once the server is gone")
            .expect("the poll task does not panic");
    }

    #[test]
    fn branch_changed_ids_only_reports_existing_branch_changes() {
        let mut last = HashMap::new();
        last.insert(
            "same".to_string(),
            GitStatus {
                branch: Some("main".to_string()),
                ..GitStatus::default()
            },
        );
        last.insert(
            "changed".to_string(),
            GitStatus {
                branch: Some("main".to_string()),
                ..GitStatus::default()
            },
        );

        let mut new_statuses = HashMap::new();
        new_statuses.insert(
            "same".to_string(),
            GitStatus {
                branch: Some("main".to_string()),
                ..GitStatus::default()
            },
        );
        new_statuses.insert(
            "changed".to_string(),
            GitStatus {
                branch: Some("feature".to_string()),
                ..GitStatus::default()
            },
        );
        new_statuses.insert(
            "new".to_string(),
            GitStatus {
                branch: Some("main".to_string()),
                ..GitStatus::default()
            },
        );

        let changed = branch_changed_ids(&last, &new_statuses);
        assert_eq!(changed, HashSet::from(["changed".to_string()]));
    }

    #[test]
    fn clear_github_cache_for_ids_removes_pr_and_ci_entries() {
        let mut prs = HashMap::from([("p1".to_string(), None), ("p2".to_string(), None)]);
        let mut checks = HashMap::from([("p1".to_string(), None), ("p3".to_string(), None)]);

        clear_github_cache_for_ids(
            &HashSet::from(["p1".to_string(), "missing".to_string()]),
            &mut prs,
            &mut checks,
        );

        assert!(!prs.contains_key("p1"));
        assert!(prs.contains_key("p2"));
        assert!(!checks.contains_key("p1"));
        assert!(checks.contains_key("p3"));
    }

    #[test]
    fn trigger_accumulator_keeps_visible_projects_conditional() {
        let mut acc = TriggerAccumulator::default();
        acc.record(GitPollTrigger::head_change("committed".to_string()));
        acc.record(GitPollTrigger::project_visible("visible".to_string()));
        acc.record(GitPollTrigger::branch_change("switched".to_string()));
        acc.record(GitPollTrigger::visibility_changed());

        assert!(acc.candidate_gh_ids.contains("visible"));
        assert!(acc.force_gh_ids.contains("switched"));
        assert!(acc.invalidate_gh_ids.contains("switched"));
        assert!(acc.head_change_ids.contains("committed"));
        assert!(!acc.force_gh_ids.contains("committed"));
        assert!(!acc.force_gh_ids.contains("visible"));
        assert_eq!(
            acc.local_status_ids(),
            HashSet::from([
                "committed".to_string(),
                "visible".to_string(),
                "switched".to_string(),
            ])
        );
    }

    #[test]
    fn status_poll_selection_respects_tiers_and_targeted_wakes() {
        let active = HashSet::from(["visible".to_string(), "hidden".to_string()]);
        let relevant = HashSet::from(["visible".to_string()]);
        let hidden = HashSet::from(["hidden".to_string()]);
        let empty = HashSet::new();

        assert_eq!(
            select_status_poll_ids(&active, &relevant, &empty, &empty, true, true),
            active,
            "startup and hidden fallback cycles scan every active project"
        );
        assert_eq!(
            select_status_poll_ids(&active, &relevant, &empty, &empty, true, false),
            relevant,
            "ordinary cadence scans only relevant projects"
        );
        assert_eq!(
            select_status_poll_ids(&active, &relevant, &hidden, &empty, false, false),
            hidden,
            "targeted hidden refreshes do not wait for fallback cadence"
        );
        assert_eq!(
            select_status_poll_ids(&active, &empty, &empty, &hidden, false, false),
            hidden,
            "promotion to the relevant tier refreshes immediately"
        );
    }

    #[test]
    fn merging_targeted_statuses_retains_unpolled_and_prunes_deleted() {
        let previous = HashMap::from([
            (
                "visible".to_string(),
                GitStatus {
                    branch: Some("main".to_string()),
                    ..GitStatus::default()
                },
            ),
            (
                "hidden".to_string(),
                GitStatus {
                    branch: Some("main".to_string()),
                    ..GitStatus::default()
                },
            ),
            ("deleted".to_string(), GitStatus::default()),
        ]);
        let active = HashSet::from([
            "visible".to_string(),
            "hidden".to_string(),
            "not-a-repo".to_string(),
        ]);
        let attempted = HashMap::from([
            (
                "hidden".to_string(),
                Some(GitStatus {
                    branch: Some("feature".to_string()),
                    ..GitStatus::default()
                }),
            ),
            ("not-a-repo".to_string(), None),
        ]);

        let merged = merge_status_results(&previous, &active, attempted);
        assert_eq!(
            merged
                .get("visible")
                .and_then(|status| status.branch.as_deref()),
            Some("main"),
            "unpolled active status stays published"
        );
        assert_eq!(
            merged
                .get("hidden")
                .and_then(|status| status.branch.as_deref()),
            Some("feature")
        );
        assert!(!merged.contains_key("deleted"));
        assert!(!merged.contains_key("not-a-repo"));
    }

    #[test]
    fn unsampled_head_snapshots_survive_fast_tier_ticks() {
        let mut previous = HashMap::from([
            ("hidden".to_string(), "old".to_string()),
            ("deleted".to_string(), "old".to_string()),
        ]);
        let active = HashSet::from(["hidden".to_string()]);

        assert!(update_head_snapshots(&mut previous, &active, HashMap::new()).is_empty());
        assert_eq!(previous.get("hidden").map(String::as_str), Some("old"));
        assert!(!previous.contains_key("deleted"));

        let changed = update_head_snapshots(
            &mut previous,
            &active,
            HashMap::from([("hidden".to_string(), "new".to_string())]),
        );
        assert_eq!(changed, vec!["hidden".to_string()]);
    }

    fn project_with_terminal(id: &str, terminal_id: &str) -> okena_state::ProjectData {
        okena_state::ProjectData {
            id: id.to_string(),
            name: "Project".to_string(),
            path: "/tmp".to_string(),
            layout: Some(okena_state::LayoutNode::Terminal {
                terminal_id: Some(terminal_id.to_string()),
                minimized: false,
                detached: false,
                shell_type: Default::default(),
                zoom_level: 1.0,
            }),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            task_ref: None,
            also_tasks: Vec::new(),
            spec_change: None,
            knowledge_root: None,
            project_scan: None,
            task_draft: None,
            custom_session: None,
            agent_purpose: None,
            agent: None,
            folder_color: Default::default(),
            hooks: Default::default(),
            connection_id: None,
            service_terminals: HashMap::new(),
            default_shell: None,
            hook_terminals: HashMap::new(),
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
            creating_progress: None,
            verification_runs: Vec::new(),
        }
    }

    fn workspace_data_with_projects(projects: &[(&str, &str)]) -> okena_state::WorkspaceData {
        let mut data = empty_workspace_data();
        for (id, terminal_id) in projects {
            data.projects.push(project_with_terminal(id, terminal_id));
            data.project_order.push(id.to_string());
        }
        data
    }

    /// Every project is hidden in the daemon's own window, so relevance comes
    /// only from what connected clients declare or subscribe to.
    fn workspace_with_hidden_projects(projects: &[(&str, &str)]) -> Workspace {
        let mut data = workspace_data_with_projects(projects);
        for (id, _) in projects {
            data.main_window.hidden_project_ids.insert(id.to_string());
        }
        Workspace::new(data)
    }

    /// Every project is visible in the daemon's own persisted window state —
    /// the legacy copy a desktop client never updates.
    fn workspace_with_daemon_visible_projects(projects: &[(&str, &str)]) -> Workspace {
        Workspace::new(workspace_data_with_projects(projects))
    }

    /// The regression this whole path exists for: a desktop client keeps its
    /// own visibility (client-side window ids, `window-layout.json`) and never
    /// writes to the daemon's copy, so a project hidden here can be the very
    /// one on screen. The client's declaration has to win.
    #[test]
    fn client_declared_projects_enter_the_gh_scope() {
        let workspace = workspace_with_hidden_projects(&[("on-screen", "t-on-screen")]);

        let nothing_declared = RwLock::new(HashMap::new());
        assert!(!visible_project_ids(&workspace, &nothing_declared).contains("on-screen"));

        let declared = RwLock::new(HashMap::from([(
            7u64,
            HashSet::from(["on-screen".to_string()]),
        )]));
        assert!(visible_project_ids(&workspace, &declared).contains("on-screen"));
    }

    #[test]
    fn every_clients_viewport_counts() {
        let workspace = Workspace::new(empty_workspace_data());
        let declared = RwLock::new(HashMap::from([
            (1u64, HashSet::from(["desktop".to_string()])),
            (2u64, HashSet::from(["phone".to_string()])),
        ]));
        let visible = visible_project_ids(&workspace, &declared);
        assert!(visible.contains("desktop") && visible.contains("phone"));
    }

    /// A desktop client's hides live in its own `window-layout.json`; the
    /// daemon's persisted window state is a stale copy that must not widen the
    /// responsive tier once any client has said what it renders.
    #[test]
    fn a_declared_viewport_supersedes_the_daemons_own_window_state() {
        let workspace =
            workspace_with_daemon_visible_projects(&[("stale", "t-stale"), ("shown", "t-shown")]);
        let declared = RwLock::new(HashMap::from([(
            1u64,
            HashSet::from(["shown".to_string()]),
        )]));

        assert_eq!(
            visible_project_ids(&workspace, &declared),
            HashSet::from(["shown".to_string()])
        );
        let no_subscriptions = RwLock::new(HashMap::new());
        assert_eq!(
            streaming_project_ids(&workspace, &no_subscriptions, &declared),
            HashSet::from(["shown".to_string()])
        );

        declared.write().unwrap().insert(1, HashSet::new());
        assert!(visible_project_ids(&workspace, &declared).is_empty());
    }

    /// A headless daemon serving clients that never declare a viewport (TUI,
    /// CLI, nobody at all) still has only its own window state to go by.
    #[test]
    fn the_daemons_own_window_state_counts_while_nobody_has_declared() {
        let workspace = workspace_with_daemon_visible_projects(&[("stale", "t-stale")]);
        let nothing_declared = RwLock::new(HashMap::new());

        assert!(visible_project_ids(&workspace, &nothing_declared).contains("stale"));
    }

    #[test]
    fn the_gh_fan_out_follows_the_declared_viewport() {
        let workspace =
            workspace_with_daemon_visible_projects(&[("stale", "t-stale"), ("shown", "t-shown")]);
        let declared = RwLock::new(HashMap::from([(
            1u64,
            HashSet::from(["shown".to_string()]),
        )]));
        let visible = visible_project_ids(&workspace, &declared);
        let projects: Vec<(String, String)> = workspace
            .projects()
            .iter()
            .map(|project| (project.id.clone(), project.path.clone()))
            .collect();

        let polls = select_github_polls(
            &projects,
            &visible,
            &HashSet::new(),
            &GithubPollSchedule::default(),
            &HashMap::new(),
            1,
            true,
            &HashSet::new(),
            false,
        );

        let polled: Vec<&str> = polls.iter().map(|poll| poll.id.as_str()).collect();
        assert_eq!(polled, ["shown"]);
    }

    /// The desktop subscribes to every terminal it mirrors, so its
    /// subscriptions must not drag hidden projects onto the responsive tier.
    #[test]
    fn a_declared_viewport_silences_that_connections_subscriptions() {
        let workspace =
            workspace_with_hidden_projects(&[("shown", "t-shown"), ("background", "t-background")]);
        let subscribed = RwLock::new(HashMap::from([(
            1u64,
            HashSet::from(["t-shown".to_string(), "t-background".to_string()]),
        )]));
        let declared = RwLock::new(HashMap::from([(
            1u64,
            HashSet::from(["shown".to_string()]),
        )]));

        let relevant = streaming_project_ids(&workspace, &subscribed, &declared);
        assert_eq!(relevant, HashSet::from(["shown".to_string()]));
    }

    #[test]
    fn an_empty_declared_viewport_still_counts_as_declared() {
        let workspace = workspace_with_hidden_projects(&[("background", "t-background")]);
        let subscribed = RwLock::new(HashMap::from([(
            1u64,
            HashSet::from(["t-background".to_string()]),
        )]));
        let declared = RwLock::new(HashMap::from([(1u64, HashSet::new())]));

        assert!(streaming_project_ids(&workspace, &subscribed, &declared).is_empty());
    }

    /// TUI/CLI streaming clients have no viewport to declare; what they stream
    /// is what they show.
    #[test]
    fn subscriptions_promote_projects_for_undeclared_connections() {
        let workspace =
            workspace_with_hidden_projects(&[("shown", "t-shown"), ("background", "t-background")]);
        let subscribed = RwLock::new(HashMap::from([
            (
                1u64,
                HashSet::from(["t-shown".to_string(), "t-background".to_string()]),
            ),
            (2u64, HashSet::from(["t-background".to_string()])),
        ]));
        let declared = RwLock::new(HashMap::from([(
            1u64,
            HashSet::from(["shown".to_string()]),
        )]));

        let relevant = streaming_project_ids(&workspace, &subscribed, &declared);
        assert_eq!(
            relevant,
            HashSet::from(["shown".to_string(), "background".to_string()])
        );
    }

    #[test]
    fn a_project_entering_a_viewport_is_fetched_off_cadence() {
        let workspace = workspace_with_hidden_projects(&[("shown", "t-shown")]);
        let active = HashSet::from(["shown".to_string()]);
        let subscribed = RwLock::new(HashMap::from([(
            1u64,
            HashSet::from(["t-shown".to_string()]),
        )]));
        let declared = RwLock::new(HashMap::from([(1u64, HashSet::new())]));

        let known = streaming_project_ids(&workspace, &subscribed, &declared);
        assert!(known.is_empty());

        declared
            .write()
            .unwrap()
            .insert(1, HashSet::from(["shown".to_string()]));
        let relevant = streaming_project_ids(&workspace, &subscribed, &declared);
        let newly_relevant: HashSet<String> = relevant.difference(&known).cloned().collect();
        let empty = HashSet::new();
        assert_eq!(
            select_status_poll_ids(&active, &relevant, &empty, &newly_relevant, false, false),
            HashSet::from(["shown".to_string()])
        );
    }

    /// Build an `apply_github_result` fixture: one project, one CI outcome.
    fn github_result(generation: u64, branch: &str, ci: CiFetch) -> GithubPollResult {
        GithubPollResult {
            head_generations: HashMap::from([("p1".to_string(), generation)]),
            branches: HashMap::from([("p1".to_string(), Some(branch.to_string()))]),
            pr_infos: HashMap::from([("p1".to_string(), None)]),
            ci: HashMap::from([("p1".to_string(), ci)]),
            rate_limited: false,
            reached_github: true,
        }
    }

    fn fetched(sha: &str) -> CiFetch {
        CiFetch::Fetched {
            sha: Some(sha.to_string()),
            summary: None,
        }
    }

    struct ApplyHarness {
        schedule: GithubPollSchedule,
        pr_infos: HashMap<String, Option<git::PrInfo>>,
        ci_checks: HashMap<String, Option<git::CiCheckSummary>>,
        last: HashMap<String, GitStatus>,
        git_status_tx: watch::Sender<HashMap<String, ApiGitStatus>>,
        state_version: watch::Sender<u64>,
        _rx: watch::Receiver<HashMap<String, ApiGitStatus>>,
        _state_rx: watch::Receiver<u64>,
    }

    impl ApplyHarness {
        fn new() -> Self {
            let (git_status_tx, _rx) = watch::channel(HashMap::new());
            let (state_version, _state_rx) = watch::channel(0);
            Self {
                schedule: GithubPollSchedule::default(),
                pr_infos: HashMap::new(),
                ci_checks: HashMap::new(),
                last: HashMap::from([(
                    "p1".to_string(),
                    GitStatus {
                        branch: Some("main".to_string()),
                        ..GitStatus::default()
                    },
                )]),
                git_status_tx,
                state_version,
                _rx,
                _state_rx,
            }
        }

        fn apply(
            &mut self,
            result: GithubPollResult,
            cycle: u64,
            generations: &HashMap<String, u64>,
        ) {
            apply_github_result(
                result,
                cycle,
                generations,
                &mut self.schedule,
                &mut self.pr_infos,
                &mut self.ci_checks,
                &mut self.last,
                &self.git_status_tx,
                &self.state_version,
            );
        }
    }

    #[test]
    fn github_results_apply_only_to_the_captured_head() {
        let mut harness = ApplyHarness::new();
        let current_generations = HashMap::from([("p1".to_string(), 2)]);

        harness.apply(
            github_result(1, "main", fetched("abc")),
            5,
            &current_generations,
        );
        harness.apply(
            github_result(2, "feature", fetched("abc")),
            5,
            &current_generations,
        );
        assert!(!harness.pr_infos.contains_key("p1"));
        assert!(!harness.ci_checks.contains_key("p1"));

        harness.apply(
            github_result(2, "main", fetched("abc")),
            5,
            &current_generations,
        );
        assert!(harness.pr_infos.contains_key("p1"));
        assert!(harness.ci_checks.contains_key("p1"));
    }

    #[test]
    fn settled_result_arms_the_commit_skip_for_the_next_poll() {
        let mut harness = ApplyHarness::new();
        let generations = HashMap::from([("p1".to_string(), 0)]);

        harness.apply(github_result(0, "main", fetched("abc")), 5, &generations);

        assert_eq!(
            harness.schedule.ci_skip_sha("p1", 6).as_deref(),
            Some("abc")
        );
        // Settled → back on the slow cadence, not the pending one.
        assert!(!harness.schedule.ci_due("p1", 10, true));
        assert!(harness.schedule.ci_due("p1", 17, true));
    }

    #[test]
    fn a_skipped_fetch_keeps_the_cached_summary() {
        let mut harness = ApplyHarness::new();
        let generations = HashMap::from([("p1".to_string(), 0)]);
        harness.ci_checks.insert(
            "p1".to_string(),
            Some(git::CiCheckSummary {
                status: git::CiStatus::Success,
                passed: 1,
                failed: 0,
                pending: 0,
                total: 1,
                checks: Vec::new(),
            }),
        );

        harness.apply(
            github_result(0, "main", CiFetch::Unchanged),
            5,
            &generations,
        );

        assert!(
            harness.ci_checks.get("p1").is_some_and(Option::is_some),
            "an unchanged commit must not blank the badge"
        );
    }

    // ─── Repository PR lists ───────────────────────────────────────────

    fn repo_pr(number: u32) -> git::RepoPullRequest {
        git::RepoPullRequest {
            pr: git::PrInfo {
                url: format!("https://github.com/o/r/pull/{number}"),
                state: git::PrState::Open,
                number,
                base: Some("main".to_string()),
                readiness: None,
                readiness_unavailable: false,
            },
            title: format!("PR {number}"),
            author: Some("someone".to_string()),
            head: format!("feat/{number}"),
            ci: None,
        }
    }

    /// A repo `o/r` open as a project and as a worktree, another repo, and a
    /// checkout with no github.com repository.
    fn repo_key_map() -> HashMap<String, Option<String>> {
        HashMap::from([
            ("repo".to_string(), Some("o/r".to_string())),
            ("worktree".to_string(), Some("o/r".to_string())),
            ("other".to_string(), Some("o/s".to_string())),
            ("local".to_string(), None),
        ])
    }

    fn repo_projects() -> Vec<(String, String)> {
        ["repo", "worktree", "other", "local"]
            .iter()
            .map(|id| (id.to_string(), format!("/tmp/{id}")))
            .collect()
    }

    #[test]
    fn a_repository_open_as_a_project_and_a_worktree_is_asked_once() {
        assert_eq!(
            repo_pr_targets(&repo_projects(), &repo_key_map()),
            [
                ("repo-prs:o/r".to_string(), "/tmp/repo".to_string()),
                ("repo-prs:o/s".to_string(), "/tmp/other".to_string()),
            ],
            "one slot per repository, none for a checkout without one"
        );
    }

    #[test]
    fn every_repository_is_listed_on_the_settled_pr_cadence_shown_or_not() {
        // Nothing here is visible, linked or forced: the list is polled anyway.
        let targets = repo_pr_targets(&repo_projects(), &repo_key_map());
        let mut schedule = GithubPollSchedule::default();
        let none = HashSet::new();

        assert!(
            select_repo_pr_polls(&targets, &schedule, 0, true, &none).is_empty(),
            "not at startup"
        );
        let polls = select_repo_pr_polls(&targets, &schedule, 1, true, &none);
        assert_eq!(polls.len(), 2);
        assert!(polls.iter().all(|p| p.repo_prs && p.want_pr && !p.want_ci));

        for poll in &polls {
            schedule.pr_dispatched(&poll.id, 1);
        }
        assert!(select_repo_pr_polls(&targets, &schedule, 12, true, &none).is_empty());
        assert_eq!(
            select_repo_pr_polls(&targets, &schedule, 13, true, &none).len(),
            2,
            "due again one PR cadence later"
        );
        assert!(
            select_repo_pr_polls(&targets, &schedule, 13, false, &none).is_empty(),
            "nothing between cadence ticks"
        );
        let running = HashSet::from(["repo-prs:o/r".to_string()]);
        let polls = select_repo_pr_polls(&targets, &schedule, 13, true, &running);
        assert_eq!(polls.len(), 1, "never twice at once");
        assert_eq!(polls[0].id, "repo-prs:o/s");
    }

    struct RepoHarness {
        schedule: GithubPollSchedule,
        repo_prs: HashMap<String, Option<Vec<git::RepoPullRequest>>>,
        last: HashMap<String, GitStatus>,
        git_status_tx: watch::Sender<HashMap<String, ApiGitStatus>>,
        state_version: watch::Sender<u64>,
        _state_rx: watch::Receiver<u64>,
    }

    impl RepoHarness {
        fn new() -> Self {
            let (git_status_tx, _) = watch::channel(HashMap::new());
            let (state_version, _state_rx) = watch::channel(0);
            Self {
                schedule: GithubPollSchedule::default(),
                repo_prs: HashMap::new(),
                last: repo_projects()
                    .into_iter()
                    .map(|(id, _)| (id, GitStatus::default()))
                    .collect(),
                git_status_tx,
                state_version,
                _state_rx,
            }
        }

        fn apply(&mut self, fetch: RepoPrsFetch, cycle: u64) {
            apply_repo_prs_result(
                "repo-prs:o/r",
                fetch,
                cycle,
                &mut self.schedule,
                &repo_key_map(),
                &mut self.repo_prs,
                &mut self.last,
                &self.git_status_tx,
                &self.state_version,
            );
        }

        /// The numbers project `id` publishes, `None` for no list.
        fn published(&self, id: &str) -> Option<Vec<u32>> {
            self.git_status_tx
                .borrow()
                .get(id)?
                .repo_pull_requests
                .as_ref()
                .map(|prs| prs.iter().map(|p| p.pr.number).collect())
        }
    }

    #[test]
    fn a_list_replaces_the_last_one_on_every_checkout_of_its_repository() {
        let mut harness = RepoHarness::new();

        harness.apply(RepoPrsFetch::Fetched(Some(vec![repo_pr(1), repo_pr(2)])), 1);
        assert_eq!(harness.published("repo"), Some(vec![1, 2]));
        assert_eq!(
            harness.published("worktree"),
            Some(vec![1, 2]),
            "a worktree shows its repository's list"
        );
        assert_eq!(harness.published("other"), None, "another repository");
        assert_eq!(harness.published("local"), None, "no github.com repository");
        assert!(!harness.schedule.pr_due("repo-prs:o/r", 12, true));
        assert!(harness.schedule.pr_due("repo-prs:o/r", 13, true));

        // #1 merged, #3 opened: one result later the list says so.
        let version = *harness.state_version.borrow();
        harness.apply(RepoPrsFetch::Fetched(Some(vec![repo_pr(2), repo_pr(3)])), 13);
        assert_eq!(harness.published("repo"), Some(vec![2, 3]));
        assert_eq!(harness.published("worktree"), Some(vec![2, 3]));
        assert!(
            *harness.state_version.borrow() > version,
            "clients are told"
        );
    }

    #[test]
    fn no_answer_keeps_the_list_and_a_refusal_parks_the_fan_out() {
        let mut harness = RepoHarness::new();
        harness.apply(RepoPrsFetch::Fetched(Some(vec![repo_pr(1)])), 1);

        harness.apply(RepoPrsFetch::Failed, 13);
        assert_eq!(harness.published("repo"), Some(vec![1]));

        harness.apply(RepoPrsFetch::RateLimited, 13);
        assert_eq!(harness.published("repo"), Some(vec![1]));
        assert!(harness.schedule.is_rate_limited(13));

        // A token lost, or the repository gone from view: no section at all.
        harness.apply(RepoPrsFetch::Fetched(None), 30);
        assert_eq!(harness.published("repo"), None);
        assert_eq!(harness.published("worktree"), None);
    }

    fn projects() -> Vec<(String, String)> {
        vec![
            ("visible".to_string(), "/tmp/visible".to_string()),
            ("hidden".to_string(), "/tmp/hidden".to_string()),
        ]
    }

    #[test]
    fn hidden_projects_never_earn_a_gh_slot() {
        let visible = HashSet::from(["visible".to_string()]);
        let schedule = GithubPollSchedule::default();

        let polls = select_github_polls(
            &projects(),
            &visible,
            &HashSet::new(),
            &schedule,
            &HashMap::new(),
            1,
            true,
            &HashSet::new(),
            false,
        );

        assert_eq!(polls.len(), 1);
        assert_eq!(polls[0].id, "visible");
    }

    #[test]
    fn an_explicit_request_reaches_a_hidden_project() {
        let visible = HashSet::new();
        let mut schedule = GithubPollSchedule::default();
        schedule.force("hidden");

        // Off-cadence too: an explicit action shouldn't wait for the next tick.
        let polls = select_github_polls(
            &projects(),
            &visible,
            &HashSet::new(),
            &schedule,
            &HashMap::new(),
            4,
            false,
            &HashSet::new(),
            false,
        );

        assert_eq!(polls.len(), 1);
        assert_eq!(polls[0].id, "hidden");
    }

    #[test]
    fn a_project_a_running_pass_holds_is_not_polled_twice() {
        let visible = HashSet::from(["visible".to_string()]);
        let schedule = GithubPollSchedule::default();
        let in_flight = HashSet::from(["visible".to_string()]);

        let polls = select_github_polls(
            &projects(),
            &visible,
            &HashSet::new(),
            &schedule,
            &HashMap::new(),
            1,
            true,
            &in_flight,
            false,
        );

        assert!(polls.is_empty());
    }

    #[test]
    fn a_forced_project_starts_its_own_pass_while_another_runs() {
        // "visible" is mid-pass, so this cycle is urgent-only: the ordinary due
        // project waits, the branch-switched one goes out now.
        let visible = HashSet::from(["visible".to_string(), "hidden".to_string()]);
        let mut schedule = GithubPollSchedule::default();
        schedule.force("hidden");
        let in_flight = HashSet::from(["visible".to_string()]);

        let polls = select_github_polls(
            &projects(),
            &visible,
            &HashSet::new(),
            &schedule,
            &HashMap::new(),
            1,
            true,
            &in_flight,
            true,
        );

        assert_eq!(polls.len(), 1);
        assert_eq!(polls[0].id, "hidden");
    }

    #[test]
    fn a_settled_project_carries_its_commit_so_the_fetch_can_be_skipped() {
        let visible = HashSet::from(["visible".to_string()]);
        let mut schedule = GithubPollSchedule::default();
        schedule.record_pr("visible", 1);
        schedule.record_ci("visible", 1, false, Some("abc".to_string()));

        // Nothing due yet on the settled cadence…
        assert!(
            select_github_polls(
                &projects(),
                &visible,
                &HashSet::new(),
                &schedule,
                &HashMap::new(),
                5,
                true,
                &HashSet::new(),
                false,
            )
            .is_empty()
        );

        // …and when it is, the cached commit rides along.
        let polls = select_github_polls(
            &projects(),
            &visible,
            &HashSet::new(),
            &schedule,
            &HashMap::new(),
            13,
            true,
            &HashSet::new(),
            false,
        );
        assert_eq!(polls.len(), 1);
        assert_eq!(polls[0].ci_skip_sha.as_deref(), Some("abc"));
    }

    #[test]
    fn one_busy_repo_does_not_speed_up_the_others() {
        let visible = HashSet::from(["visible".to_string(), "hidden".to_string()]);
        let mut schedule = GithubPollSchedule::default();
        schedule.record_pr("visible", 1);
        schedule.record_pr("hidden", 1);
        schedule.record_ci("visible", 1, true, None); // CI running
        schedule.record_ci("hidden", 1, false, Some("abc".to_string())); // settled

        let polls = select_github_polls(
            &projects(),
            &visible,
            &HashSet::new(),
            &schedule,
            &HashMap::new(),
            4,
            true,
            &HashSet::new(),
            false,
        );

        assert_eq!(polls.len(), 1, "only the repo with running CI is due");
        assert_eq!(polls[0].id, "visible");
        assert!(polls[0].want_ci && !polls[0].want_pr);
    }

    #[test]
    fn rate_limited_pass_parks_further_polling() {
        let mut harness = ApplyHarness::new();
        let generations = HashMap::from([("p1".to_string(), 0)]);
        let mut result = github_result(0, "main", fetched("abc"));
        result.rate_limited = true;
        result.reached_github = false;

        harness.apply(result, 5, &generations);

        assert!(harness.schedule.is_rate_limited(6));
    }

    /// A repo, an agent session on QBL-1, its worktree, and a worktree of the
    /// same repo on no task at all.
    fn linked_workspace() -> Workspace {
        let task = serde_json::json!({
            "id": { "provider": "linear", "external_id": "u1" },
            "display_key": "QBL-1", "title": "t", "url": "http://x",
        });
        let mut data = empty_workspace_data();
        for project in [
            serde_json::json!({ "id": "repo", "name": "okena", "path": "/p/okena" }),
            serde_json::json!({
                "id": "session", "name": "QBL-1", "path": "/p", "task_ref": task,
            }),
            serde_json::json!({
                "id": "wt", "name": "okena (QBL-1)", "path": "/p/wt", "task_ref": task,
                "worktree_info": {
                    "parent_project_id": "repo", "worktree_path": "/p/wt", "branch_name": "feat/x",
                },
            }),
            serde_json::json!({
                "id": "stray", "name": "okena (other)", "path": "/p/stray",
                "worktree_info": {
                    "parent_project_id": "repo", "worktree_path": "/p/stray", "branch_name": "other",
                },
            }),
        ] {
            let project: okena_state::ProjectData = serde_json::from_value(project).unwrap();
            data.project_order.push(project.id.clone());
            data.projects.push(project);
        }
        Workspace::new(data)
    }

    fn pr(number: u32, state: git::PrState) -> git::PrInfo {
        git::PrInfo {
            url: format!("https://github.com/o/r/pull/{number}"),
            state,
            number,
            base: None,
            readiness: None,
            readiness_unavailable: false,
        }
    }

    fn track(ws: &mut Workspace, session_id: &str, number: u32, state: git::PrState) {
        let session = ws
            .data
            .projects
            .iter_mut()
            .find(|p| p.id == session_id)
            .unwrap();
        session
            .agent
            .get_or_insert_with(Default::default)
            .tracked_prs
            .push(TrackedPullRequest {
                project: "okena".into(),
                repo_path: "/p/okena".into(),
                branch: Some("feat/x".into()),
                number,
                url: pr(number, state.clone()).url,
                state,
                readiness: None,
                readiness_unavailable: false,
            });
    }

    fn tracked_of<'a>(ws: &'a Workspace, session_id: &str) -> &'a [TrackedPullRequest] {
        ws.project(session_id)
            .and_then(|p| p.agent.as_ref())
            .map(|a| a.tracked_prs.as_slice())
            .unwrap_or_default()
    }

    fn ids(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn only_worktrees_of_a_sessions_task_are_linked() {
        let links = session_links(&linked_workspace());
        assert_eq!(links.len(), 1, "{links:?}");
        let link = &links["wt"];
        assert_eq!(link.session_id, "session");
        assert_eq!(link.project, "okena");
        assert_eq!(link.repo_path, "/p/okena");
        assert_eq!(link.branch.as_deref(), Some("feat/x"));
    }

    #[test]
    fn a_worktree_on_a_sessions_second_task_is_linked() {
        // One agent started on QBL-1 with QBL-2 picked alongside: Start work
        // made a worktree for QBL-2 as well, and it is this session's too.
        let task = |id: &str, key: &str| {
            serde_json::json!({
                "id": { "provider": "linear", "external_id": id },
                "display_key": key, "title": "t", "url": "http://x",
            })
        };
        let mut data = empty_workspace_data();
        for project in [
            serde_json::json!({ "id": "repo", "name": "okena", "path": "/p/okena" }),
            serde_json::json!({
                "id": "session", "name": "QBL-1", "path": "/p",
                "task_ref": task("u1", "QBL-1"), "also_tasks": [task("u2", "QBL-2")],
            }),
            serde_json::json!({
                "id": "wt2", "name": "okena (QBL-2)", "path": "/p/wt2",
                "task_ref": task("u2", "QBL-2"),
                "worktree_info": {
                    "parent_project_id": "repo", "worktree_path": "/p/wt2", "branch_name": "feat/qbl-2",
                },
            }),
            serde_json::json!({
                "id": "other", "name": "okena (QBL-9)", "path": "/p/other",
                "task_ref": task("u9", "QBL-9"),
                "worktree_info": {
                    "parent_project_id": "repo", "worktree_path": "/p/other", "branch_name": "feat/qbl-9",
                },
            }),
        ] {
            let project: okena_state::ProjectData = serde_json::from_value(project).unwrap();
            data.project_order.push(project.id.clone());
            data.projects.push(project);
        }

        let links = session_links(&Workspace::new(data));
        assert_eq!(links.len(), 1, "{links:?}");
        assert_eq!(links["wt2"].session_id, "session");
        assert_eq!(links["wt2"].branch.as_deref(), Some("feat/qbl-2"));
    }

    fn record_push(ws: &mut Workspace, session_id: &str, branch: &str) {
        ws.data
            .projects
            .iter_mut()
            .find(|p| p.id == session_id)
            .unwrap()
            .agent
            .get_or_insert_with(Default::default)
            .pushed_branches
            .push(okena_core::harness::PushedBranch {
                project: "okena".into(),
                repo_path: "/p/okena".into(),
                branch: branch.into(),
            });
    }

    #[test]
    fn a_pushed_branch_nothing_else_covers_is_looked_up_by_branch() {
        let mut ws = linked_workspace();
        // `feat/x` is the linked worktree's own branch: its poll covers it.
        record_push(&mut ws, "session", "feat/x");
        // `feat/own` came from a worktree the agent made itself.
        record_push(&mut ws, "session", "feat/own");
        let links = session_links(&ws);

        let pushed = pushed_branch_polls(&ws, &links);
        assert_eq!(pushed.len(), 1, "{pushed:?}");
        assert!(pushed[0].key.ends_with("|/p/okena|feat/own"));
        assert!(!pushed[0].finished);

        let mut schedule = GithubPollSchedule::default();
        let none = HashSet::new();
        let polls = select_pushed_branch_polls(&pushed, &schedule, 1, true, &none);
        assert_eq!(polls.len(), 1);
        assert_eq!(polls[0].removed_branch.as_deref(), Some("feat/own"));
        assert_eq!(polls[0].path, "/p/okena", "looked up in the main checkout");
        assert!(polls[0].want_pr && !polls[0].want_ci);

        // A push or turn end the hook reported: looked up off cadence.
        schedule.pr_dispatched(&pushed[0].key, 1);
        assert!(select_pushed_branch_polls(&pushed, &schedule, 3, false, &none).is_empty());
        schedule.force_pr(&pushed[0].key);
        assert_eq!(
            select_pushed_branch_polls(&pushed, &schedule, 3, false, &none).len(),
            1
        );
    }

    #[test]
    fn a_pushed_branchs_pr_is_recorded_then_tracked_by_number_and_kept() {
        let mut ws = linked_workspace();
        record_push(&mut ws, "session", "feat/own");
        let links = session_links(&ws);
        let pushed = pushed_branch_polls(&ws, &links);
        let key = pushed[0].key.clone();
        let ws = Mutex::new(ws);
        let tick = watch::Sender::new(0);
        let mut schedule = GithubPollSchedule::default();

        apply_pushed_branch_result(
            &key,
            PrFetch::Fetched(Some(pr(21, git::PrState::Open))),
            5,
            &mut schedule,
            &pushed,
            &ws,
            &tick,
        );
        {
            let ws = ws.lock();
            let tracked = tracked_of(&ws, "session");
            assert_eq!(tracked.len(), 1);
            assert_eq!(tracked[0].number, 21);
            assert_eq!(tracked[0].branch.as_deref(), Some("feat/own"));
            assert_eq!(tracked[0].repo_path, "/p/okena");
            // Now refreshed by number; the branch lookup stands down.
            assert!(pushed_branch_polls(&ws, &links).is_empty());
            assert_eq!(tracked_pr_polls(&ws).len(), 1);
        }

        // Merged: kept, and the branch is only watched slowly for a new PR.
        let url = pr(21, git::PrState::Merged).url;
        let mut ws = ws.into_inner();
        assert!(apply_tracked_pr(
            &mut ws.data.projects,
            &url,
            &pr(21, git::PrState::Merged)
        ));
        assert_eq!(tracked_of(&ws, "session")[0].state, git::PrState::Merged);
        let pushed = pushed_branch_polls(&ws, &links);
        assert_eq!(pushed.len(), 1);
        assert!(pushed[0].finished);
        schedule.pr_dispatched(&pushed[0].key, 10);
        let none = HashSet::new();
        assert!(select_pushed_branch_polls(&pushed, &schedule, 30, true, &none).is_empty());
        assert_eq!(
            select_pushed_branch_polls(&pushed, &schedule, 130, true, &none).len(),
            1
        );
    }

    #[test]
    fn a_pushed_branch_lookup_with_no_answer_records_nothing() {
        let mut ws = linked_workspace();
        record_push(&mut ws, "session", "feat/own");
        let links = session_links(&ws);
        let pushed = pushed_branch_polls(&ws, &links);
        let key = pushed[0].key.clone();
        let ws = Mutex::new(ws);
        let tick = watch::Sender::new(0);
        let mut schedule = GithubPollSchedule::default();

        for fetch in [PrFetch::Failed, PrFetch::Fetched(None)] {
            apply_pushed_branch_result(&key, fetch, 5, &mut schedule, &pushed, &ws, &tick);
            assert!(tracked_of(&ws.lock(), "session").is_empty());
        }
        apply_pushed_branch_result(
            &key,
            PrFetch::RateLimited,
            6,
            &mut schedule,
            &pushed,
            &ws,
            &tick,
        );
        assert!(schedule.is_rate_limited(6));
    }

    #[test]
    fn a_hidden_linked_worktree_polls_its_pr_and_checks() {
        let polls = select_github_polls(
            &projects(),
            &HashSet::new(),
            &ids(&["hidden"]),
            &GithubPollSchedule::default(),
            &HashMap::new(),
            1,
            true,
            &HashSet::new(),
            false,
        );
        assert_eq!(polls.len(), 1);
        assert_eq!(polls[0].id, "hidden");
        assert!(polls[0].want_pr && polls[0].want_ci);
    }

    fn open_pr() -> git::PrInfo {
        git::PrInfo {
            url: "https://github.com/o/r/pull/7".into(),
            state: git::PrState::Open,
            number: 7,
            base: None,
            readiness: None,
            readiness_unavailable: false,
        }
    }

    /// An open PR with a conflict and `unresolved_threads` open threads.
    fn with_threads(unresolved_threads: usize) -> git::PrInfo {
        git::PrInfo {
            readiness: Some(git::PrReadiness {
                merge_state: git::MergeState::Conflicting,
                review_decision: None,
                unresolved_threads,
                threads_truncated: false,
            }),
            ..open_pr()
        }
    }

    fn readiness_of(harness: &ApplyHarness) -> Option<git::PrReadiness> {
        harness
            .pr_infos
            .get("p1")
            .cloned()
            .flatten()
            .and_then(|pr| pr.readiness)
    }

    #[test]
    fn readiness_comes_with_the_pr_and_the_next_fetch_replaces_it() {
        // Two unresolved threads, then one is resolved: the next PR fetch
        // shows 1, with the checks skipped both times.
        let mut harness = ApplyHarness::new();
        let generations = HashMap::from([("p1".to_string(), 0)]);

        let mut result = github_result(0, "main", CiFetch::Unchanged);
        result.pr_infos = HashMap::from([("p1".to_string(), Some(with_threads(2)))]);
        harness.apply(result, 5, &generations);
        assert_eq!(
            readiness_of(&harness).map(|r| r.unresolved_threads),
            Some(2)
        );
        assert!(
            harness.last["p1"]
                .pr_info
                .as_ref()
                .is_some_and(|pr| pr.readiness.is_some()),
            "and it is published"
        );

        let mut refresh = github_result(0, "main", CiFetch::Unchanged);
        refresh.pr_infos = HashMap::from([("p1".to_string(), Some(with_threads(1)))]);
        harness.apply(refresh, 17, &generations);
        assert_eq!(
            readiness_of(&harness).map(|r| r.unresolved_threads),
            Some(1)
        );
    }

    #[test]
    fn the_checks_skip_is_armed_whatever_the_pr_says() {
        // A conflict or open threads cost no checks requests while the pushed
        // commit holds: readiness comes from the PR request instead.
        let mut harness = ApplyHarness::new();
        let generations = HashMap::from([("p1".to_string(), 0)]);
        let mut result = github_result(0, "main", fetched("abc"));
        result.pr_infos = HashMap::from([("p1".to_string(), Some(with_threads(2)))]);
        harness.apply(result, 5, &generations);

        assert_eq!(
            harness.schedule.ci_skip_sha("p1", 6).as_deref(),
            Some("abc")
        );
    }

    #[test]
    fn a_failed_checks_fetch_does_not_arm_the_skip() {
        let mut harness = ApplyHarness::new();
        let generations = HashMap::from([("p1".to_string(), 0)]);
        harness.apply(
            github_result(
                0,
                "main",
                CiFetch::Fetched {
                    sha: None,
                    summary: None,
                },
            ),
            5,
            &generations,
        );
        assert_eq!(harness.schedule.ci_skip_sha("p1", 6), None);
    }

    #[test]
    fn a_removed_worktree_hands_its_open_pr_to_its_session() {
        let mut ws = linked_workspace();
        let links = session_links(&ws);
        let mut known = HashMap::new();
        remember_linked_prs(
            &mut known,
            &links,
            &HashMap::from([("wt".to_string(), Some(pr(9, git::PrState::Open)))]),
        );
        // Removal fails the status read before the project goes, which clears
        // the PR cache. What was last seen has to survive that.
        remember_linked_prs(&mut known, &links, &HashMap::new());

        assert!(track_removed_prs(
            &mut ws.data.projects,
            &mut known,
            &ids(&["repo", "session", "stray"]),
        ));
        let tracked = tracked_of(&ws, "session");
        assert_eq!(tracked.len(), 1);
        assert_eq!(tracked[0].number, 9);
        assert_eq!(tracked[0].repo_path, "/p/okena");
        assert_eq!(tracked[0].project, "okena");
        assert!(known.is_empty());
    }

    #[test]
    fn a_worktree_that_is_still_there_hands_over_nothing() {
        let mut ws = linked_workspace();
        let link = session_links(&ws)["wt"].clone();
        let mut known = HashMap::from([("wt".to_string(), (link, pr(9, git::PrState::Open)))]);

        assert!(!track_removed_prs(
            &mut ws.data.projects,
            &mut known,
            &ids(&["repo", "session", "wt", "stray"]),
        ));
        assert_eq!(known.len(), 1);
    }

    fn register(ws: &mut Workspace, session_id: &str, number: u32) {
        let session = ws
            .data
            .projects
            .iter_mut()
            .find(|p| p.id == session_id)
            .unwrap();
        session
            .agent
            .get_or_insert_with(Default::default)
            .assets
            .push(okena_core::harness::AgentAsset {
                kind: okena_core::harness::AgentAssetKind::PullRequest,
                title: "Agent title".into(),
                url: Some(format!("{}/", pr(number, git::PrState::Open).url)),
                project: None,
                branch: None,
                created_at: 0,
                task: None,
            });
    }

    #[test]
    fn a_removed_worktrees_merged_pr_stays_on_its_session() {
        // Kept whether or not the agent registered it, so the card still says
        // Merged after the worktree is gone. Never polled again.
        for registered in [false, true] {
            let mut ws = linked_workspace();
            if registered {
                register(&mut ws, "session", 9);
            }
            let link = session_links(&ws)["wt"].clone();
            let mut known =
                HashMap::from([("wt".to_string(), (link, pr(9, git::PrState::Merged)))]);

            assert!(track_removed_prs(
                &mut ws.data.projects,
                &mut known,
                &ids(&["repo", "session"]),
            ));
            let tracked = tracked_of(&ws, "session");
            assert_eq!(tracked.len(), 1, "{registered}");
            assert_eq!(tracked[0].state, git::PrState::Merged);
            assert!(tracked_pr_polls(&ws).is_empty());
        }
    }

    #[test]
    fn a_removed_pr_already_tracked_takes_the_newer_state() {
        let mut ws = linked_workspace();
        track(&mut ws, "session", 9, git::PrState::Open);
        register(&mut ws, "session", 9);
        let link = session_links(&ws)["wt"].clone();

        assert!(record_removed_pr(
            &mut ws.data.projects,
            &link,
            pr(9, git::PrState::Closed)
        ));
        let tracked = tracked_of(&ws, "session");
        assert_eq!(tracked.len(), 1);
        assert_eq!(tracked[0].state, git::PrState::Closed);
    }

    #[test]
    fn a_pr_a_session_already_tracks_is_not_tracked_twice() {
        let mut ws = linked_workspace();
        track(&mut ws, "session", 9, git::PrState::Open);
        let link = session_links(&ws)["wt"].clone();
        let mut known = HashMap::from([("wt".to_string(), (link, pr(9, git::PrState::Open)))]);

        assert!(!track_removed_prs(
            &mut ws.data.projects,
            &mut known,
            &ids(&["repo", "session"]),
        ));
        assert_eq!(tracked_of(&ws, "session").len(), 1);
    }

    #[test]
    fn a_tracked_pr_follows_its_state_and_stays_once_merged() {
        for registered in [false, true] {
            let mut ws = linked_workspace();
            track(&mut ws, "session", 9, git::PrState::Draft);
            if registered {
                register(&mut ws, "session", 9);
            }
            let url = pr(9, git::PrState::Open).url;

            assert!(apply_tracked_pr(
                &mut ws.data.projects,
                &url,
                &pr(9, git::PrState::Open)
            ));
            assert_eq!(tracked_of(&ws, "session")[0].state, git::PrState::Open);
            assert!(
                !apply_tracked_pr(&mut ws.data.projects, &url, &pr(9, git::PrState::Open)),
                "the same state again is not a change"
            );

            assert!(apply_tracked_pr(
                &mut ws.data.projects,
                &url,
                &pr(9, git::PrState::Merged)
            ));
            let tracked = tracked_of(&ws, "session");
            assert_eq!(tracked.len(), 1, "kept whether registered or not");
            assert_eq!(tracked[0].state, git::PrState::Merged);
            assert!(
                tracked_pr_polls(&ws).is_empty(),
                "a merged PR is not polled again"
            );
        }
    }

    #[test]
    fn tracked_prs_are_polled_once_each_and_only_on_cadence() {
        let mut ws = linked_workspace();
        track(&mut ws, "session", 9, git::PrState::Open);
        track(&mut ws, "repo", 9, git::PrState::Open);
        let tracked = tracked_pr_polls(&ws);
        assert_eq!(tracked.len(), 1, "two sessions, one PR, one request");

        let schedule = GithubPollSchedule::default();
        let polls = select_tracked_pr_polls(&tracked, &schedule, 1, true, &HashSet::new());
        assert_eq!(polls.len(), 1);
        assert_eq!(polls[0].tracked_pr, Some(9));
        assert_eq!(polls[0].path, "/p/okena");
        assert!(polls[0].want_pr && !polls[0].want_ci);

        assert!(select_tracked_pr_polls(&tracked, &schedule, 1, false, &HashSet::new()).is_empty());
        assert!(
            select_tracked_pr_polls(
                &tracked,
                &schedule,
                1,
                true,
                &ids(&[tracked[0].key.as_str()])
            )
            .is_empty()
        );
    }

    #[test]
    fn a_rate_limited_tracked_pr_parks_polling_and_changes_nothing() {
        let ws = Mutex::new(linked_workspace());
        let tick = watch::Sender::new(0);
        let mut schedule = GithubPollSchedule::default();

        apply_tracked_pr_result(
            &format!("{TRACKED_PR_KEY_PREFIX}https://github.com/o/r/pull/9"),
            PrFetch::RateLimited,
            5,
            &mut schedule,
            &mut HashMap::new(),
            &ws,
            &tick,
        );

        assert!(schedule.is_rate_limited(6));
        assert_eq!(*tick.borrow(), 0);
    }

    #[test]
    fn a_lookup_that_never_went_out_does_not_lift_the_rate_limit() {
        let ws = Mutex::new(linked_workspace());
        let tick = watch::Sender::new(0);
        let mut schedule = GithubPollSchedule::default();
        schedule.note_rate_limited(0);

        apply_tracked_pr_result(
            &format!("{TRACKED_PR_KEY_PREFIX}https://github.com/o/r/pull/9"),
            PrFetch::Failed,
            5,
            &mut schedule,
            &mut HashMap::new(),
            &ws,
            &tick,
        );

        assert!(schedule.is_rate_limited(5));
    }

    #[test]
    fn a_tracked_pr_is_dropped_only_when_github_keeps_saying_it_is_gone() {
        let ws = Mutex::new(linked_workspace());
        track(&mut ws.lock(), "session", 9, git::PrState::Open);
        let key = format!("{TRACKED_PR_KEY_PREFIX}{}", pr(9, git::PrState::Open).url);
        let tick = watch::Sender::new(0);
        let mut schedule = GithubPollSchedule::default();
        let mut failures = HashMap::new();
        let mut apply = |fetch: PrFetch| {
            apply_tracked_pr_result(&key, fetch, 5, &mut schedule, &mut failures, &ws, &tick)
        };

        // Offline, or no token, for however long: a real PR is never dropped.
        for _ in 0..TRACKED_PR_LOOKUP_ATTEMPTS * 3 {
            apply(PrFetch::Failed);
        }
        assert_eq!(tracked_of(&ws.lock(), "session").len(), 1);

        for _ in 1..TRACKED_PR_LOOKUP_ATTEMPTS {
            apply(PrFetch::Fetched(None));
        }
        // An answer starts the count again.
        apply(PrFetch::Fetched(Some(pr(9, git::PrState::Open))));
        for _ in 1..TRACKED_PR_LOOKUP_ATTEMPTS {
            apply(PrFetch::Fetched(None));
        }
        // No answer in between neither counts nor resets.
        apply(PrFetch::Failed);
        assert_eq!(tracked_of(&ws.lock(), "session").len(), 1);

        apply(PrFetch::Fetched(None));
        assert!(tracked_of(&ws.lock(), "session").is_empty());
    }

    #[test]
    fn a_linked_worktree_whose_pr_finished_is_still_watched_slowly() {
        // PR #1 merged; a follow-up PR on the same branch must still show up.
        let merged = HashMap::from([("hidden".to_string(), Some(pr(9, git::PrState::Merged)))]);
        let mut schedule = GithubPollSchedule::default();
        schedule.record_pr("hidden", 1);
        let select = |visible: &HashSet<String>, cycle: u64| {
            select_github_polls(
                &projects(),
                visible,
                &ids(&["hidden"]),
                &schedule,
                &merged,
                cycle,
                true,
                &HashSet::new(),
                false,
            )
        };

        assert!(
            select(&HashSet::new(), 13).iter().all(|poll| !poll.want_pr),
            "not on the normal cadence"
        );
        let polls = select(&HashSet::new(), 1 + FINISHED_LINKED_PR_EVERY_N_CYCLES);
        assert_eq!(polls.len(), 1);
        assert!(polls[0].want_pr);
        assert_eq!(
            select(&ids(&["hidden"]), 13).len(),
            1,
            "on screen it is polled like any project"
        );
    }

    #[test]
    fn a_hidden_linked_worktree_whose_pr_finished_requests_no_checks() {
        let mut schedule = GithubPollSchedule::default();
        schedule.record_pr("hidden", 1);
        schedule.record_ci("hidden", 1, false, None);
        let select = |pr_infos: &HashMap<String, Option<git::PrInfo>>,
                      visible: &HashSet<String>,
                      cycle: u64| {
            select_github_polls(
                &projects(),
                visible,
                &ids(&["hidden"]),
                &schedule,
                pr_infos,
                cycle,
                true,
                &HashSet::new(),
                false,
            )
        };
        let slow_cycle = 1 + FINISHED_LINKED_PR_EVERY_N_CYCLES;

        for state in [git::PrState::Merged, git::PrState::Closed] {
            let finished = HashMap::from([("hidden".to_string(), Some(pr(9, state)))]);
            assert!(
                select(&finished, &HashSet::new(), 13).is_empty(),
                "checks are due, but not for a finished PR"
            );
            let polls = select(&finished, &HashSet::new(), slow_cycle);
            assert_eq!(polls.len(), 1);
            assert!(polls[0].want_pr && !polls[0].want_ci, "only the slow PR poll");
            assert!(
                select(&finished, &ids(&["hidden"]), 13)[0].want_ci,
                "on screen its checks are polled as before"
            );
        }

        // The slow PR poll finds a new open PR: checks resume on the next pass.
        let reopened = HashMap::from([("hidden".to_string(), Some(pr(10, git::PrState::Open)))]);
        let polls = select(&reopened, &HashSet::new(), slow_cycle + 1);
        assert_eq!(polls.len(), 1);
        assert!(polls[0].want_ci);
    }

    #[test]
    fn a_registration_fetches_a_hidden_worktrees_pr_but_not_its_checks() {
        let mut schedule = GithubPollSchedule::default();
        schedule.record_pr("hidden", 1);
        schedule.record_ci("hidden", 1, false, None);
        schedule.force_pr("hidden");

        // Even while another pass runs.
        let polls = select_github_polls(
            &projects(),
            &HashSet::new(),
            &ids(&["hidden"]),
            &schedule,
            &HashMap::new(),
            2,
            false,
            &HashSet::new(),
            true,
        );
        assert_eq!(polls.len(), 1);
        assert_eq!(polls[0].id, "hidden");
        assert!(polls[0].want_pr && !polls[0].want_ci);
    }

    #[test]
    fn a_branch_lookup_is_retried_only_a_cadence_later() {
        let ws = linked_workspace();
        let (key, pending) = removed_lookup(&ws);
        let mut schedule = GithubPollSchedule::default();
        schedule.force_pr(&key);
        let select = |schedule: &GithubPollSchedule, cycle: u64, cadence_due: bool| {
            select_removed_branch_polls(&pending, schedule, cycle, cadence_due, &HashSet::new())
        };

        assert_eq!(
            select(&schedule, 3, false).len(),
            1,
            "the first goes out at once"
        );
        schedule.pr_dispatched(&key, 3);
        assert!(
            select(&schedule, 3, false).is_empty() && select(&schedule, 4, true).is_empty(),
            "a wake right after a failure does not retry"
        );
        assert_eq!(select(&schedule, 15, true).len(), 1);
    }

    #[test]
    fn registering_an_asset_marks_its_session_for_a_pr_fetch() {
        let mut acc = TriggerAccumulator::default();
        acc.record(GitPollTrigger::linked_worktrees("session".to_string()));
        assert!(acc.session_asset_ids.contains("session"));
        assert!(
            acc.local_status_ids().is_empty(),
            "the session itself is not a checkout"
        );
        acc.clear();
        assert!(acc.session_asset_ids.is_empty());
    }

    #[test]
    fn a_worktree_removed_before_its_pr_was_seen_gets_a_branch_lookup() {
        let ws = linked_workspace();
        let links = session_links(&ws);
        let active = ids(&["repo", "session", "stray"]);

        let unseen = unseen_removed_worktrees(&links, &HashMap::new(), &active);
        assert_eq!(unseen.len(), 1);
        assert_eq!(unseen[0].0, format!("{REMOVED_BRANCH_KEY_PREFIX}wt"));

        let known = HashMap::from([(
            "wt".to_string(),
            (links["wt"].clone(), pr(9, git::PrState::Open)),
        )]);
        assert!(
            unseen_removed_worktrees(&links, &known, &active).is_empty(),
            "a PR already seen is handed over without a lookup"
        );

        let (key, pending) = removed_lookup(&ws);
        let polls = select_removed_branch_polls(
            &pending,
            &GithubPollSchedule::default(),
            1,
            true,
            &HashSet::new(),
        );
        assert_eq!(polls.len(), 1);
        assert_eq!(polls[0].removed_branch.as_deref(), Some("feat/x"));
        assert_eq!(polls[0].path, "/p/okena");
        assert!(polls[0].want_pr && !polls[0].want_ci);
        assert!(
            select_removed_branch_polls(
                &pending,
                &GithubPollSchedule::default(),
                1,
                true,
                &ids(&[key.as_str()]),
            )
            .is_empty()
        );
    }

    fn removed_lookup(ws: &Workspace) -> (String, HashMap<String, RemovedLookup>) {
        let key = format!("{REMOVED_BRANCH_KEY_PREFIX}wt");
        let link = session_links(ws)["wt"].clone();
        (
            key.clone(),
            HashMap::from([(key, RemovedLookup { link, attempts: 0 })]),
        )
    }

    #[test]
    fn a_branch_lookup_that_finds_the_pr_hands_it_to_the_session() {
        let ws = Mutex::new(linked_workspace());
        let (key, mut pending) = removed_lookup(&ws.lock());
        let tick = watch::Sender::new(0);
        let mut schedule = GithubPollSchedule::default();

        apply_removed_branch_result(
            &key,
            PrFetch::Fetched(Some(pr(9, git::PrState::Open))),
            5,
            &mut schedule,
            &mut pending,
            &ws,
            &tick,
        );

        assert!(pending.is_empty());
        assert_eq!(tracked_of(&ws.lock(), "session").len(), 1);
        assert_eq!(*tick.borrow(), 1, "the session change is saved");
    }

    #[test]
    fn a_branch_lookup_without_an_answer_is_tried_a_few_times() {
        let ws = Mutex::new(linked_workspace());
        let (key, mut pending) = removed_lookup(&ws.lock());
        let tick = watch::Sender::new(0);
        let mut schedule = GithubPollSchedule::default();

        for _ in 1..REMOVED_BRANCH_LOOKUP_ATTEMPTS {
            apply_removed_branch_result(
                &key,
                PrFetch::Failed,
                5,
                &mut schedule,
                &mut pending,
                &ws,
                &tick,
            );
            assert!(pending.contains_key(&key));
        }
        apply_removed_branch_result(
            &key,
            PrFetch::Failed,
            5,
            &mut schedule,
            &mut pending,
            &ws,
            &tick,
        );
        assert!(pending.is_empty());
        assert!(tracked_of(&ws.lock(), "session").is_empty());
    }

    #[test]
    fn only_pull_request_links_are_read_as_prs() {
        assert_eq!(
            parse_pr_link("https://github.com/n1rna/okena/pull/24/"),
            Some(PrLink {
                host: "github.com".into(),
                owner: "n1rna".into(),
                repo: "okena".into(),
                number: 24
            })
        );
        assert_eq!(
            parse_pr_link("https://github.com/n1rna/okena/pull/24/files#diff").map(|l| l.number),
            Some(24)
        );
        assert!(parse_pr_link("https://github.com/n1rna/okena/issues/24").is_none());
        assert!(parse_pr_link("https://linear.app/qblok/issue/QBL-374").is_none());
        assert!(parse_pr_link("not a link").is_none());
    }

    #[test]
    fn pr_links_keep_their_host_and_match_only_a_checkout_on_it() {
        let repo = |host: &str| git::repository::GithubRepo {
            host: host.into(),
            owner: "Team".into(),
            name: "App".into(),
        };
        let enterprise = parse_pr_link("https://Acme.GHE.com/team/app/pull/7").expect("a PR link");
        assert_eq!(
            enterprise,
            PrLink {
                host: "acme.ghe.com".into(),
                owner: "team".into(),
                repo: "app".into(),
                number: 7
            }
        );
        let dotcom = parse_pr_link("https://github.com/team/app/pull/7").expect("a PR link");
        let server = parse_pr_link("https://github.acme.corp/team/app/pull/7").expect("a PR link");
        assert_eq!(server.host, "github.acme.corp");

        // The same `owner/name` on two hosts are two repositories.
        assert!(enterprise.names(&repo("acme.ghe.com")));
        assert!(!enterprise.names(&repo("github.com")));
        assert!(dotcom.names(&repo("github.com")));
        assert!(!dotcom.names(&repo("acme.ghe.com")));
        assert!(server.names(&repo("github.acme.corp")));
        assert!(!server.names(&repo("github.com")));
    }

    fn merged_worktree_found() -> RegisteredFound {
        RegisteredFound {
            project: "okena".into(),
            repo_path: "/p/okena".into(),
            head_branch: Some("feat/x".into()),
        }
    }

    #[test]
    fn a_pr_registered_after_its_worktree_went_is_looked_up_and_kept() {
        // Case A: the PR merged, the worktree was removed with nothing
        // registered, and the agent registers the link at wrap-up.
        let mut ws = linked_workspace();
        ws.data.projects.retain(|p| p.id != "wt");
        register(&mut ws, "session", 9);
        let links = session_links(&ws);

        let unknown = unknown_registered_prs(&ws, &ids(&["session"]), &HashMap::new(), &links);
        assert_eq!(unknown.len(), 1);
        let lookup = unknown.into_iter().next().unwrap();
        assert_eq!(lookup.target.link.number, 9);
        assert_eq!(lookup.target.link.owner, "o");
        assert_eq!(lookup.target.link.repo, "r");
        assert!(
            lookup
                .target
                .candidates
                .iter()
                .any(|(_, path)| path == "/p/okena"),
            "the repo checkout is a candidate, the session and worktrees are not"
        );
        assert!(
            !lookup
                .target
                .candidates
                .iter()
                .any(|(_, path)| path == "/p" || path == "/p/stray")
        );

        let key = registered_key(&lookup);
        let mut pending = HashMap::from([(key.clone(), lookup)]);
        let ws = Mutex::new(ws);
        let tick = watch::Sender::new(0);
        apply_registered_pr_result(
            &key,
            PrFetch::Fetched(Some(pr(9, git::PrState::Merged))),
            Some(merged_worktree_found()),
            5,
            &mut GithubPollSchedule::default(),
            &mut pending,
            &links,
            &ws,
            &tick,
        );

        assert!(pending.is_empty());
        assert_eq!(tracked_of(&ws.lock(), "session").len(), 1);
        assert!(tracked_of(&ws.lock(), "session")[0].is_finished());
        // So the registration shows as one card marked Merged, not a row with
        // no state.
        let agent = ws.lock().project("session").unwrap().agent.clone().unwrap();
        let rows = okena_core::session_assets::derive_session_assets(
            &agent.assets,
            &[],
            &agent.tracked_prs,
            &agent.pushed_branches,
        );
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(
            rows[0].pr.as_ref().map(|p| (p.number, p.state.clone())),
            Some((9, git::PrState::Merged))
        );
        assert!(rows[0].registered);

        // Known now: registering again looks nothing up.
        let ws = ws.lock();
        assert!(
            unknown_registered_prs(&ws, &ids(&["session"]), &HashMap::new(), &links).is_empty()
        );
    }

    #[test]
    fn a_registered_pr_a_live_worktree_is_on_is_left_to_that_worktree() {
        let mut ws = linked_workspace();
        register(&mut ws, "session", 9);
        let links = session_links(&ws);

        // Its PR already fetched: nothing to look up.
        let live = HashMap::from([("wt".to_string(), Some(pr(9, git::PrState::Open)))]);
        assert!(unknown_registered_prs(&ws, &ids(&["session"]), &live, &links).is_empty());

        // Not fetched yet: looked up once, and the answer is on the live
        // worktree's branch, so nothing is recorded.
        let mut pending: HashMap<String, RegisteredLookup> =
            unknown_registered_prs(&ws, &ids(&["session"]), &HashMap::new(), &links)
                .into_iter()
                .map(|lookup| (registered_key(&lookup), lookup))
                .collect();
        let key = pending.keys().next().cloned().expect("a lookup");
        let ws = Mutex::new(ws);
        apply_registered_pr_result(
            &key,
            PrFetch::Fetched(Some(pr(9, git::PrState::Open))),
            Some(merged_worktree_found()),
            5,
            &mut GithubPollSchedule::default(),
            &mut pending,
            &links,
            &ws,
            &watch::Sender::new(0),
        );
        assert!(pending.is_empty());
        assert!(tracked_of(&ws.lock(), "session").is_empty());
    }
}
