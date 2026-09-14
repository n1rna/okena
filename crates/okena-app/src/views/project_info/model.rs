//! What okena knows about one project, independent of how it is shown.
//!
//! The facts the harness Projects view used to gather into a lane per repo,
//! read here for a single project so they can sit beside its own terminal.

use crate::workspace::state::{ProjectData, Workspace};
use okena_core::api::{ApiGitStatus, CiStatus, PrState, RepoPullRequest};
use okena_core::project_map::{Interface, InterfaceKind, ProjectMap};
use std::collections::HashSet;

/// Git facts for one checkout: branch, diff, divergence, PR and pipeline.
#[derive(Clone, Debug, Default)]
pub struct GitFacts {
    pub branch: Option<String>,
    pub lines_added: usize,
    pub lines_removed: usize,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    pub pr: Option<(u32, PrState)>,
    pub ci: Option<(CiStatus, usize, usize, usize)>,
}

impl GitFacts {
    /// Read from the daemon snapshot that already feeds the column header, so
    /// the panel and the header chip cannot disagree.
    pub fn collect(ws: &Workspace, project_id: &str) -> Self {
        let Some(g) = ws
            .remote_snapshot(project_id)
            .and_then(|snap| snap.git_status.as_ref())
        else {
            return Self::default();
        };
        Self {
            branch: g.branch.clone(),
            lines_added: g.lines_added,
            lines_removed: g.lines_removed,
            ahead: g.ahead,
            behind: g.behind,
            pr: g.pr_info.as_ref().map(|pr| (pr.number, pr.state.clone())),
            ci: g
                .ci_checks
                .as_ref()
                .map(|c| (c.status.clone(), c.passed, c.failed, c.pending)),
        }
    }

    /// Whether anything has changed in the checkout — and so whether a diff is
    /// worth offering at all.
    pub fn has_changes(&self) -> bool {
        self.lines_added > 0 || self.lines_removed > 0
    }
}

/// What the project is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectInfoKind {
    Repo,
    /// A worktree, with the name of the repo it was created from while that is
    /// still open.
    Worktree {
        repo: Option<String>,
    },
}

/// Everything the panel shows about one project.
#[derive(Clone, Debug)]
pub struct ProjectInfo {
    pub project_id: String,
    pub path: String,
    pub kind: ProjectInfoKind,
    pub git: GitFacts,
    /// Worktrees open against a repo, by id. Always empty for a worktree.
    pub worktrees: Vec<String>,
    /// Agent sessions working this project's tasks, by id.
    pub sessions: Vec<String>,
    /// Every open pull request in the project's GitHub repository — a
    /// worktree's is its repository's. `None` when there is no list: no
    /// github.com remote, no token, or no poll answered yet.
    pub pull_requests: Option<Vec<RepoPullRequest>>,
}

impl ProjectInfo {
    /// Collect everything shown about `project_id`, or `None` if it is gone.
    pub fn collect(ws: &Workspace, project_id: &str) -> Option<Self> {
        let project = ws.project(project_id)?;
        let worktrees: Vec<&ProjectData> = project
            .worktree_ids
            .iter()
            .filter_map(|id| ws.project(id))
            .collect();
        let kind = match &project.worktree_info {
            Some(info) => ProjectInfoKind::Worktree {
                repo: ws
                    .project(&info.parent_project_id)
                    .map(|parent| parent.name.clone()),
            },
            None => ProjectInfoKind::Repo,
        };
        let tasks = task_ids(project, worktrees.iter().copied());

        Some(Self {
            project_id: project.id.clone(),
            path: project.path.clone(),
            kind,
            git: GitFacts::collect(ws, project_id),
            worktrees: worktrees.iter().map(|w| w.id.clone()).collect(),
            sessions: sessions_working(ws.projects(), &tasks),
            pull_requests: pull_requests_of(
                ws.remote_snapshot(project_id)
                    .and_then(|snap| snap.git_status.as_ref()),
            ),
        })
    }
}

/// The open pull requests a project shows, from the daemon snapshot that
/// feeds its header — so a local and a remote project read alike.
pub(super) fn pull_requests_of(git: Option<&ApiGitStatus>) -> Option<Vec<RepoPullRequest>> {
    git?.repo_pull_requests.clone()
}

/// What a PR row says under its title: its number, who opened it, and the
/// branch it merges from and into.
pub(super) fn pr_caption(pr: &RepoPullRequest) -> String {
    let mut parts = vec![format!("#{}", pr.pr.number)];
    parts.extend(pr.author.clone());
    let branches = match (pr.head.as_str(), pr.pr.base.as_deref()) {
        ("", None) => None,
        (head, None) => Some(head.to_string()),
        ("", Some(base)) => Some(format!("→ {base}")),
        (head, Some(base)) => Some(format!("{head} → {base}")),
    };
    parts.extend(branches);
    parts.join(" · ")
}

/// A map's interfaces grouped by type: types in [`InterfaceKind::all`] order,
/// each group in manifest order, empty types left out.
pub(super) fn group_interfaces(list: &[Interface]) -> Vec<(InterfaceKind, Vec<&Interface>)> {
    InterfaceKind::all()
        .into_iter()
        .filter_map(|kind| {
            let items: Vec<&Interface> = list.iter().filter(|i| i.kind == kind).collect();
            (!items.is_empty()).then_some((kind, items))
        })
        .collect()
}

/// The names of the areas `ids` names, in order: a label where the map has
/// one, the id itself otherwise.
pub(super) fn area_labels(map: &ProjectMap, ids: &[String]) -> String {
    ids.iter()
        .map(|id| map.area(id).map_or(id.as_str(), |a| a.label()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Tasks a project has work in flight for: its own, when one was started on the
/// project itself, and those of the worktrees open against it.
pub(super) fn task_ids<'a>(
    project: &'a ProjectData,
    worktrees: impl Iterator<Item = &'a ProjectData>,
) -> HashSet<String> {
    project
        .linked_tasks()
        .chain(worktrees.flat_map(|w| w.linked_tasks()))
        .map(|t| t.id.external_id.clone())
        .collect()
}

/// Agent sessions working any of `task_ids`, in workspace order.
///
/// Matched by task rather than by directory: a session is rooted above the
/// repos precisely so one agent can span several, so it has no path that would
/// place it under a project. A worktree carries its task too, but it is a
/// checkout, not a session, and is never listed as one.
pub(super) fn sessions_working(
    projects: &[ProjectData],
    task_ids: &HashSet<String>,
) -> Vec<String> {
    projects
        .iter()
        .filter(|p| p.worktree_info.is_none() && p.is_any_agent_session())
        .filter(|p| {
            p.linked_tasks()
                .any(|t| task_ids.contains(&t.id.external_id))
        })
        .map(|p| p.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{
        area_labels, group_interfaces, pr_caption, pull_requests_of, sessions_working, task_ids,
    };
    use okena_core::api::{ApiGitStatus, PrInfo, PrState, RepoPullRequest};
    use crate::workspace::state::ProjectData;
    use okena_core::project_map::{InterfaceKind, ProjectMap};
    use std::collections::HashSet;

    fn map(json: serde_json::Value) -> ProjectMap {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn interfaces_group_by_type_in_a_fixed_order_keeping_manifest_order() {
        let m = map(serde_json::json!({
            "version": 1,
            "project": { "name": "api", "description": "d" },
            "consumes": [
                { "type": "package", "name": "@acme/money" },
                { "type": "http", "name": "accounts/v1" },
                { "type": "package", "name": "@acme/log" },
            ],
        }));
        let groups: Vec<(InterfaceKind, Vec<&str>)> = group_interfaces(&m.consumes)
            .into_iter()
            .map(|(kind, items)| (kind, items.iter().map(|i| i.name.as_str()).collect()))
            .collect();
        assert_eq!(
            groups,
            [
                (InterfaceKind::Http, vec!["accounts/v1"]),
                (InterfaceKind::Package, vec!["@acme/money", "@acme/log"]),
            ]
        );
        assert!(group_interfaces(&m.exposes).is_empty());
    }

    #[test]
    fn areas_are_named_by_label_else_id() {
        let m = map(serde_json::json!({
            "version": 1,
            "project": { "name": "api", "description": "d" },
            "areas": [
                { "id": "billing", "name": "Billing", "description": "d", "paths": ["src"] },
                { "id": "http", "description": "d", "paths": ["src"] },
            ],
        }));
        let ids = [
            "billing".to_string(),
            "http".to_string(),
            "gone".to_string(),
        ];
        assert_eq!(area_labels(&m, &ids), "Billing, http, gone");
    }

    fn project(json: serde_json::Value) -> ProjectData {
        serde_json::from_value(json).unwrap()
    }

    fn task(external_id: &str) -> serde_json::Value {
        serde_json::json!({
            "id": { "provider": "linear", "external_id": external_id },
            "display_key": "QBL-1", "title": "t", "url": "http://x",
        })
    }

    fn task_session(id: &str, external_id: &str) -> ProjectData {
        project(serde_json::json!({
            "id": id, "name": "QBL-1 (agent)", "path": "/p", "task_ref": task(external_id),
        }))
    }

    fn worktree(id: &str, external_id: &str) -> ProjectData {
        project(serde_json::json!({
            "id": id, "name": "okena (QBL-1)", "path": "/p/wt",
            "worktree_info": {
                "parent_project_id": "repo",
                "main_repo_path": "/p/okena",
                "worktree_path": "/p/wt",
                "branch_name": "feat/x",
            },
            "task_ref": task(external_id),
        }))
    }

    fn ids(v: &[&str]) -> HashSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_repo_has_the_tasks_of_its_worktrees() {
        let repo = project(serde_json::json!({ "id": "repo", "name": "okena", "path": "/p" }));
        let worktrees = [worktree("wt1", "u1"), worktree("wt2", "u2")];
        assert_eq!(task_ids(&repo, worktrees.iter()), ids(&["u1", "u2"]));
    }

    #[test]
    fn a_task_started_on_the_repo_itself_counts_too() {
        let repo = project(serde_json::json!({
            "id": "repo", "name": "okena", "path": "/p", "task_ref": task("u3"),
        }));
        assert_eq!(task_ids(&repo, std::iter::empty()), ids(&["u3"]));
    }

    #[test]
    fn a_session_is_listed_under_the_project_whose_task_it_works() {
        let projects = [task_session("s1", "u1"), task_session("s2", "u9")];
        assert_eq!(sessions_working(&projects, &ids(&["u1", "u2"])), ["s1"]);
    }

    #[test]
    fn a_worktree_on_the_task_is_not_listed_as_a_session() {
        // It carries the same task as the session working it, and is the
        // checkout that session writes to — not a second agent.
        let projects = [worktree("wt1", "u1"), task_session("s1", "u1")];
        assert_eq!(sessions_working(&projects, &ids(&["u1"])), ["s1"]);
    }

    #[test]
    fn a_spec_session_belongs_to_no_project() {
        // It works the spec repository, which is not a project here.
        let spec = project(serde_json::json!({
            "id": "s1", "name": "add-login (spec)", "path": "/specs",
            "spec_change": "add-login",
        }));
        assert!(sessions_working(&[spec], &ids(&["u1"])).is_empty());
    }

    #[test]
    fn a_project_with_no_tasks_claims_no_sessions() {
        // Nothing but the task links a session to a project — a loose match
        // would list one agent under every repo.
        assert!(sessions_working(&[task_session("s1", "u1")], &ids(&[])).is_empty());
    }

    #[test]
    fn a_session_on_several_tasks_is_listed_for_each_of_them() {
        let mut session = task_session("s1", "u1");
        session.also_tasks = vec![serde_json::from_value(task("u2")).unwrap()];
        assert_eq!(sessions_working(&[session], &ids(&["u2"])), ["s1"]);
    }

    #[test]
    fn a_repo_has_every_task_its_worktrees_cover() {
        let repo = project(serde_json::json!({ "id": "repo", "name": "okena", "path": "/p" }));
        let mut wt = worktree("wt1", "u1");
        wt.also_tasks = vec![serde_json::from_value(task("u2")).unwrap()];
        assert_eq!(task_ids(&repo, [wt].iter()), ids(&["u1", "u2"]));
    }

    fn listed(author: Option<&str>, head: &str, base: Option<&str>) -> RepoPullRequest {
        RepoPullRequest {
            pr: PrInfo {
                url: "https://github.com/o/r/pull/12".into(),
                state: PrState::Open,
                number: 12,
                base: base.map(Into::into),
                readiness: None,
                readiness_unavailable: false,
            },
            title: "Someone's change".into(),
            author: author.map(Into::into),
            head: head.into(),
            ci: None,
        }
    }

    #[test]
    fn a_pr_row_names_its_author_and_where_it_merges() {
        assert_eq!(
            pr_caption(&listed(Some("octo"), "feat/x", Some("main"))),
            "#12 · octo · feat/x → main"
        );
        assert_eq!(
            pr_caption(&listed(None, "feat/x", None)),
            "#12 · feat/x",
            "a deleted author and an unknown base are left out"
        );
        assert_eq!(pr_caption(&listed(None, "", Some("main"))), "#12 · → main");
    }

    #[test]
    fn the_panel_shows_the_list_the_snapshot_carries_and_no_section_without_one() {
        assert_eq!(pull_requests_of(None), None, "no git status yet");
        let mut git = ApiGitStatus::default();
        assert_eq!(
            pull_requests_of(Some(&git)),
            None,
            "not on github.com, or no token"
        );
        git.repo_pull_requests = Some(Vec::new());
        assert_eq!(
            pull_requests_of(Some(&git)),
            Some(Vec::new()),
            "a section that says none are open"
        );
        git.repo_pull_requests = Some(vec![listed(Some("octo"), "feat/x", Some("main"))]);
        assert_eq!(pull_requests_of(Some(&git)).map(|l| l.len()), Some(1));
    }
}
