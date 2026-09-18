//! Which repos and worktrees an agent session works in.
//!
//! A session is a project of its own, rooted above the repos, so nothing in
//! its path says where its work lands. It is tied to repos two ways: a task
//! session owns every worktree that works on one of its tasks, and a session
//! with no worktree of its own — a coordinator over picked tasks — records
//! the repos it was given in `repo_ids`. The projects a session was merely
//! handed context from (`context_projects`) are not a link.
//!
//! One definition for every place that asks, so the session panel's WORKTREES
//! list and the sidebar's tree cannot disagree about where an agent works.

use crate::{AgentSortMode, ProjectData};
use okena_core::tasks::TaskRef;
use std::collections::HashMap;

/// Whether `candidate` works on any of the session's tasks, and isn't the
/// session itself.
///
/// Every task on both sides counts: a session started on several picked tasks
/// owns the worktrees of its second and later tasks too. Matched on the
/// provider's own task id rather than the display key, which changes when an
/// issue moves team and would silently drop the worktrees.
pub fn is_related<'a>(
    session_id: &str,
    session_tasks: &[String],
    candidate_id: &str,
    mut candidate_tasks: impl Iterator<Item = &'a TaskRef>,
) -> bool {
    candidate_id != session_id && candidate_tasks.any(|t| session_tasks.contains(&t.id.external_id))
}

/// Where each agent session sits among the repos, by session project id.
///
/// Every list is in display order: live sessions first, in the order asked
/// for, then closed ones, most recently closed first. A session appears under
/// every worktree it relates to, and under every repo it was given when it
/// relates to none.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SessionPlacement {
    by_worktree: HashMap<String, Vec<String>>,
    by_repo: HashMap<String, Vec<String>>,
}

impl SessionPlacement {
    /// Sessions working in the worktree `worktree_id`.
    pub fn for_worktree(&self, worktree_id: &str) -> &[String] {
        self.by_worktree
            .get(worktree_id)
            .map_or(&[], |v| v.as_slice())
    }

    /// Sessions sitting directly under the repo `repo_id`: ones given it in
    /// `repo_ids` that have no worktree of their own.
    pub fn for_repo(&self, repo_id: &str) -> &[String] {
        self.by_repo.get(repo_id).map_or(&[], |v| v.as_slice())
    }

    /// Whether `session_id` is placed under anything.
    pub fn contains(&self, session_id: &str) -> bool {
        self.by_worktree
            .values()
            .chain(self.by_repo.values())
            .any(|ids| ids.iter().any(|id| id == session_id))
    }

    /// Whether no session is placed anywhere.
    pub fn is_empty(&self) -> bool {
        self.by_worktree.is_empty() && self.by_repo.is_empty()
    }
}

/// Place every agent session in `projects` under the worktrees and repos it
/// works in. Sessions linked to neither are left out.
///
/// `live_order` orders the live sessions the way the Agents list does, so a
/// worktree's agents read in the same order in both lists.
pub fn place_sessions(projects: &[ProjectData], live_order: AgentSortMode) -> SessionPlacement {
    let mut sessions: Vec<&ProjectData> = projects
        .iter()
        .filter(|p| p.worktree_info.is_none() && p.agent_role().is_some())
        .collect();
    sessions.sort_by(|a, b| match (a.closed_at, b.closed_at) {
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(x), Some(y)) => y.cmp(&x).then_with(|| a.name.cmp(&b.name)),
        (None, None) => match live_order {
            // A session that has never run anything sorts last rather than
            // first, as it does in the Agents list.
            AgentSortMode::Activity => b
                .last_activity_at
                .cmp(&a.last_activity_at)
                .then_with(|| a.name.cmp(&b.name)),
            AgentSortMode::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        },
    });

    let worktrees: Vec<&ProjectData> = projects
        .iter()
        .filter(|p| p.worktree_info.is_some())
        .collect();

    let mut placement = SessionPlacement::default();
    for session in sessions {
        let tasks: Vec<String> = session
            .linked_tasks()
            .map(|t| t.id.external_id.clone())
            .collect();
        let mut related = false;
        if !tasks.is_empty() {
            for wt in &worktrees {
                if is_related(&session.id, &tasks, &wt.id, wt.linked_tasks()) {
                    related = true;
                    placement
                        .by_worktree
                        .entry(wt.id.clone())
                        .or_default()
                        .push(session.id.clone());
                }
            }
        }
        // Only a session with no worktree sits directly under a repo: one
        // with a worktree is already shown where its work is.
        if !related {
            for repo_id in &session.repo_ids {
                let under = placement.by_repo.entry(repo_id.clone()).or_default();
                if !under.contains(&session.id) {
                    under.push(session.id.clone());
                }
            }
        }
    }
    placement
}

#[cfg(test)]
mod tests {
    use super::{is_related, place_sessions};
    use crate::{AgentSortMode, ProjectData};
    use okena_core::tasks::{TaskId, TaskRef};

    fn task(external: &str, key: &str) -> TaskRef {
        TaskRef {
            id: TaskId::new("linear", external),
            display_key: key.to_string(),
            title: "Title".to_string(),
            url: "http://x".to_string(),
            parent_id: None,
            parent_key: None,
        }
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn task_json(external: &str) -> serde_json::Value {
        serde_json::json!({
            "id": { "provider": "linear", "external_id": external },
            "display_key": external.to_uppercase(), "title": "t", "url": "http://x",
        })
    }

    fn repo(id: &str) -> ProjectData {
        serde_json::from_value(serde_json::json!({ "id": id, "name": id, "path": "/r" })).unwrap()
    }

    fn worktree(id: &str, parent: &str, tasks: &[&str]) -> ProjectData {
        let mut p: ProjectData = serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "path": "/r/wt",
            "worktree_info": {
                "parent_project_id": parent, "color_override": null,
                "main_repo_path": "/r", "worktree_path": "/r/wt", "branch_name": id,
            },
        }))
        .unwrap();
        let mut refs = tasks
            .iter()
            .map(|t| serde_json::from_value(task_json(t)).unwrap());
        p.task_ref = refs.next();
        p.also_tasks = refs.collect();
        p
    }

    /// A task session on `tasks` (its own first, then the others picked
    /// with it).
    fn session(id: &str, tasks: &[&str]) -> ProjectData {
        let mut p: ProjectData = serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "path": "/projects",
        }))
        .unwrap();
        let mut refs = tasks
            .iter()
            .map(|t| serde_json::from_value(task_json(t)).unwrap());
        p.task_ref = refs.next();
        p.also_tasks = refs.collect();
        p
    }

    fn custom(id: &str) -> ProjectData {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "path": "/projects", "custom_session": "goal",
        }))
        .unwrap()
    }

    #[test]
    fn a_worktree_on_the_same_task_is_related() {
        assert!(is_related(
            "s1",
            &ids(&["u1"]),
            "wt1",
            [task("u1", "QBL-1")].iter()
        ));
    }

    #[test]
    fn a_worktree_on_a_sessions_second_task_is_related() {
        // A session started on two picked tasks: the worktree Start work made
        // for the second one is as much this session's as the first's.
        assert!(is_related(
            "s1",
            &ids(&["u1", "u2"]),
            "wt2",
            [task("u2", "QBL-2")].iter()
        ));
        // And a worktree covering several tasks matches on any of them.
        assert!(is_related(
            "s1",
            &ids(&["u2"]),
            "wt1",
            [task("u1", "QBL-1"), task("u2", "QBL-2")].iter()
        ));
    }

    #[test]
    fn the_session_is_not_related_to_itself() {
        // It carries the same task as its worktrees, so it would otherwise
        // list itself as one of its own checkouts.
        assert!(!is_related(
            "s1",
            &ids(&["u1"]),
            "s1",
            [task("u1", "QBL-1")].iter()
        ));
    }

    #[test]
    fn a_different_task_is_not_related() {
        assert!(!is_related(
            "s1",
            &ids(&["u1", "u2"]),
            "wt1",
            [task("u9", "QBL-9")].iter()
        ));
    }

    #[test]
    fn an_unlinked_project_is_not_related() {
        assert!(!is_related("s1", &ids(&["u1"]), "p1", std::iter::empty()));
    }

    #[test]
    fn matching_is_by_provider_id_not_display_key() {
        // The display key changes when an issue moves team; the provider id
        // does not. Matching on the key would silently drop the worktrees.
        let mut moved = task("u1", "NEW-7");
        moved.title = "Moved".to_string();
        assert!(is_related("s1", &ids(&["u1"]), "wt1", [moved].iter()));
    }

    #[test]
    fn a_session_sits_under_the_worktree_on_its_task() {
        let projects = vec![
            repo("okena"),
            worktree("wt1", "okena", &["u1"]),
            session("s1", &["u1"]),
        ];
        let placed = place_sessions(&projects, AgentSortMode::Activity);
        assert_eq!(placed.for_worktree("wt1"), ["s1"]);
        // Not also under the repo: it is shown where its work is.
        assert!(placed.for_repo("okena").is_empty());
    }

    #[test]
    fn a_session_sits_under_the_worktree_of_its_second_task() {
        let projects = vec![
            worktree("wt2", "okena", &["u2"]),
            session("s1", &["u1", "u2"]),
        ];
        let placed = place_sessions(&projects, AgentSortMode::Activity);
        assert_eq!(placed.for_worktree("wt2"), ["s1"]);
    }

    #[test]
    fn a_session_with_worktrees_in_two_repos_sits_under_both() {
        let projects = vec![
            worktree("wt-a", "repo-a", &["u1"]),
            worktree("wt-b", "repo-b", &["u2"]),
            session("s1", &["u1", "u2"]),
        ];
        let placed = place_sessions(&projects, AgentSortMode::Activity);
        assert_eq!(placed.for_worktree("wt-a"), ["s1"]);
        assert_eq!(placed.for_worktree("wt-b"), ["s1"]);
    }

    #[test]
    fn a_worktree_shared_by_two_sessions_shows_both() {
        let mut older = session("s-old", &["u1"]);
        older.last_activity_at = Some(10);
        let mut newer = session("s-new", &["u1"]);
        newer.last_activity_at = Some(20);
        let projects = vec![worktree("wt1", "okena", &["u1"]), older, newer];
        let placed = place_sessions(&projects, AgentSortMode::Activity);
        assert_eq!(placed.for_worktree("wt1"), ["s-new", "s-old"]);
    }

    #[test]
    fn a_coordinator_with_no_worktree_sits_under_each_repo_it_was_given() {
        let mut coordinator = custom("coord");
        coordinator.repo_ids = ids(&["repo-a", "repo-b"]);
        let projects = vec![repo("repo-a"), repo("repo-b"), coordinator];
        let placed = place_sessions(&projects, AgentSortMode::Activity);
        assert_eq!(placed.for_repo("repo-a"), ["coord"]);
        assert_eq!(placed.for_repo("repo-b"), ["coord"]);
    }

    #[test]
    fn a_session_with_a_worktree_is_not_repeated_under_its_repo_ids() {
        let mut s = session("s1", &["u1"]);
        s.repo_ids = ids(&["okena"]);
        let projects = vec![worktree("wt1", "okena", &["u1"]), s];
        let placed = place_sessions(&projects, AgentSortMode::Activity);
        assert_eq!(placed.for_worktree("wt1"), ["s1"]);
        assert!(placed.for_repo("okena").is_empty());
    }

    #[test]
    fn context_projects_are_not_a_link() {
        let mut s = custom("spec");
        s.context_projects = ids(&["okena"]);
        let projects = vec![repo("okena"), s];
        let placed = place_sessions(&projects, AgentSortMode::Activity);
        assert!(placed.for_repo("okena").is_empty());
        assert!(
            placed.is_empty(),
            "a session with neither link is not placed"
        );
    }

    #[test]
    fn a_worktree_carrying_a_task_with_no_session_has_no_agents() {
        let projects = vec![
            worktree("wt1", "okena", &["u1"]),
            session("s1", &["u9"]),
            repo("okena"),
        ];
        let placed = place_sessions(&projects, AgentSortMode::Activity);
        assert!(placed.for_worktree("wt1").is_empty());
        assert!(placed.is_empty());
    }

    #[test]
    fn closed_sessions_follow_live_ones_newest_closed_first() {
        let mut closed_old = session("closed-old", &["u1"]);
        closed_old.closed_at = Some(100);
        let mut closed_new = session("closed-new", &["u1"]);
        closed_new.closed_at = Some(300);
        let mut live_idle = session("live-idle", &["u1"]);
        live_idle.last_activity_at = None;
        let mut live_busy = session("live-busy", &["u1"]);
        live_busy.last_activity_at = Some(50);
        let projects = vec![
            closed_old,
            live_idle,
            worktree("wt1", "okena", &["u1"]),
            closed_new,
            live_busy,
        ];
        let placed = place_sessions(&projects, AgentSortMode::Activity);
        assert_eq!(
            placed.for_worktree("wt1"),
            ["live-busy", "live-idle", "closed-new", "closed-old"]
        );
        let by_name = place_sessions(&projects, AgentSortMode::Name);
        assert_eq!(
            by_name.for_worktree("wt1"),
            ["live-busy", "live-idle", "closed-new", "closed-old"]
        );
    }
}
