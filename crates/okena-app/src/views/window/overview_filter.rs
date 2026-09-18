//! Narrowing the Projects and Agents overviews from the bar above the grid.
//!
//! Kept apart from the view and free of GPUI so the rules — what a search
//! matches, and how chips combine with it — are tested directly. The same two
//! rules as the Tasks filter (`harness::task_filter`):
//!
//! - **Within a chip group, OR.** Picking a second state widens.
//! - **Across groups, AND** — and with the search text. Picking a second group
//!   narrows.
//!
//! The filter only ever narrows the list the grid was already going to show:
//! folder filter, hidden projects and focus are applied before it, by
//! `Workspace::visible_projects`, and are none of its business.

use crate::workspace::state::{AgentRole, ProjectData};
use okena_core::agent_activity::AgentActivity;

/// The states a chip can be picked for, in the order they are offered.
///
/// The order a card's colour escalates in rather than alphabetical, so the
/// chips read like the lifecycle they are.
const STATE_ORDER: [AgentActivity; 8] = [
    AgentActivity::Working,
    AgentActivity::Waiting,
    AgentActivity::NeedsInput,
    AgentActivity::ReadyForReview,
    AgentActivity::Blocked,
    AgentActivity::Done,
    AgentActivity::Stopped,
    AgentActivity::Unknown,
];

/// The roles a chip can be picked for, in the order they are offered.
const ROLE_ORDER: [AgentRole; 6] = [
    AgentRole::Implement,
    AgentRole::Task,
    AgentRole::Spec,
    AgentRole::Knowledge,
    AgentRole::Scan,
    AgentRole::Custom,
];

/// What an agent session is, for the chips: what its card says it is doing,
/// and what it was started for. `None` on the Projects overview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AgentFacts {
    pub state: AgentActivity,
    pub role: Option<AgentRole>,
}

/// One project as the filter sees it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Candidate<'a> {
    pub project: &'a ProjectData,
    /// The checked-out branch from the daemon's git poll, when it is known.
    pub branch: Option<&'a str>,
    pub agent: Option<AgentFacts>,
}

/// What an overview is currently narrowed to. `Default` is "everything".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OverviewFilter {
    /// Text in the search box, as typed.
    search: String,
    /// Picked states. A `Vec` rather than a set: there are at most eight, and
    /// `AgentRole` has no ordering or hash to key a set by.
    states: Vec<AgentActivity>,
    roles: Vec<AgentRole>,
}

impl OverviewFilter {
    /// Whether anything is narrowing the grid, so "N of M" and Clear show.
    pub fn is_active(&self) -> bool {
        self.searching() || !self.states.is_empty() || !self.roles.is_empty()
    }

    fn searching(&self) -> bool {
        !self.search.trim().is_empty()
    }

    pub fn set_search(&mut self, text: &str) {
        self.search = text.to_string();
    }

    /// Empty the text and the chips alike: there is one Clear.
    pub fn clear(&mut self) {
        self.search.clear();
        self.states.clear();
        self.roles.clear();
    }

    pub fn state_selected(&self, state: AgentActivity) -> bool {
        self.states.contains(&state)
    }

    pub fn role_selected(&self, role: AgentRole) -> bool {
        self.roles.contains(&role)
    }

    pub fn toggle_state(&mut self, state: AgentActivity) {
        toggle(&mut self.states, state);
    }

    pub fn toggle_role(&mut self, role: AgentRole) {
        toggle(&mut self.roles, role);
    }

    /// Whether `c` survives the filter.
    ///
    /// A project with no agent facts fails any picked chip: a chip asks about
    /// agents, and a plain project is not one.
    pub fn matches(&self, c: &Candidate) -> bool {
        if !self.states.is_empty() && !c.agent.is_some_and(|a| self.states.contains(&a.state)) {
            return false;
        }
        if !self.roles.is_empty()
            && !c
                .agent
                .and_then(|a| a.role)
                .is_some_and(|r| self.roles.contains(&r))
        {
            return false;
        }
        let needle = self.search.trim().to_lowercase();
        needle.is_empty() || haystack(c).any(|s| s.to_lowercase().contains(&needle))
    }

    /// Drop picked chips no candidate has any more, so a finished agent does
    /// not leave a filter that matches nothing. The text is left alone: the
    /// person typing it is looking at the result.
    pub fn prune(&mut self, facets: &Facets) {
        self.states.retain(|s| facets.states.contains(s));
        self.roles.retain(|r| facets.roles.contains(r));
    }
}

fn toggle<T: PartialEq>(set: &mut Vec<T>, value: T) {
    if let Some(i) = set.iter().position(|v| *v == value) {
        set.remove(i);
    } else {
        set.push(value);
    }
}

/// Every string a search looks in: name, path, branch, and the task the
/// project was started for — its key and title, and the keys of any others.
fn haystack<'a>(c: &Candidate<'a>) -> impl Iterator<Item = &'a str> {
    let p = c.project;
    // The stored worktree branch is deprecated and empty on anything written
    // since, but an old row may still carry it.
    let stored_branch = p
        .worktree_info
        .as_ref()
        .map(|w| w.branch_name.as_str())
        .filter(|b| !b.is_empty());
    let task = p
        .task_ref
        .iter()
        .flat_map(|t| [t.display_key.as_str(), t.title.as_str()]);
    let also = p.also_tasks.iter().map(|t| t.display_key.as_str());
    [p.name.as_str(), p.path.as_str()]
        .into_iter()
        .chain(stored_branch)
        .chain(c.branch)
        .chain(task)
        .chain(also)
}

/// The chip values on offer: only those some candidate actually has.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Facets {
    pub states: Vec<AgentActivity>,
    pub roles: Vec<AgentRole>,
}

/// Gather the chips to offer from `candidates`, in a fixed order.
pub(crate) fn collect_facets(candidates: &[Candidate]) -> Facets {
    let agents = || candidates.iter().filter_map(|c| c.agent);
    Facets {
        states: STATE_ORDER
            .into_iter()
            .filter(|s| agents().any(|a| a.state == *s))
            .collect(),
        roles: ROLE_ORDER
            .into_iter()
            .filter(|r| agents().any(|a| a.role == Some(*r)))
            .collect(),
    }
}

/// The ids of the candidates that survive, in their original order. Each is
/// judged on its own: a worktree does not bring in its parent, nor a parent
/// its worktrees.
pub(crate) fn apply(filter: &OverviewFilter, candidates: &[Candidate]) -> Vec<String> {
    candidates
        .iter()
        .filter(|c| filter.matches(c))
        .map(|c| c.project.id.clone())
        .collect()
}

/// A chip's word for a state, as the card shows it.
pub(crate) fn state_label(state: AgentActivity) -> &'static str {
    match state {
        AgentActivity::Working => "Working",
        AgentActivity::Waiting => "Waiting",
        AgentActivity::NeedsInput => "Needs input",
        AgentActivity::ReadyForReview => "Ready for review",
        AgentActivity::Blocked => "Blocked",
        AgentActivity::Done => "Done",
        AgentActivity::Stopped => "Stopped",
        AgentActivity::Unknown => "Needs attention",
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentFacts, Candidate, Facets, OverviewFilter, apply, collect_facets};
    use crate::workspace::state::{AgentRole, ProjectData};
    use okena_core::agent_activity::AgentActivity;

    fn project(json: serde_json::Value) -> ProjectData {
        serde_json::from_value(json).expect("a minimal project deserializes")
    }

    fn plain(id: &str, name: &str, path: &str) -> ProjectData {
        project(serde_json::json!({ "id": id, "name": name, "path": path }))
    }

    fn task(key: &str, title: &str) -> serde_json::Value {
        serde_json::json!({
            "id": { "provider": "linear", "external_id": key },
            "display_key": key,
            "title": title,
            "url": "",
        })
    }

    fn cand(p: &ProjectData) -> Candidate<'_> {
        Candidate {
            project: p,
            branch: None,
            agent: None,
        }
    }

    fn agent(p: &ProjectData, state: AgentActivity, role: AgentRole) -> Candidate<'_> {
        Candidate {
            project: p,
            branch: None,
            agent: Some(AgentFacts {
                state,
                role: Some(role),
            }),
        }
    }

    fn searching(text: &str) -> OverviewFilter {
        let mut f = OverviewFilter::default();
        f.set_search(text);
        f
    }

    #[test]
    fn empty_filter_matches_everything_and_is_not_active() {
        let p = plain("a", "okena", "/src/okena");
        let f = OverviewFilter::default();
        assert!(f.matches(&cand(&p)));
        assert!(!f.is_active());
        // Whitespace alone is not a search.
        let f = searching("   ");
        assert!(f.matches(&cand(&p)));
        assert!(!f.is_active());
    }

    #[test]
    fn search_matches_name_and_path() {
        let p = plain("a", "Frontend", "/home/me/work/web-app");
        assert!(searching("front").matches(&cand(&p)));
        assert!(searching("web-app").matches(&cand(&p)));
        assert!(!searching("backend").matches(&cand(&p)));
    }

    #[test]
    fn search_is_case_insensitive_and_trimmed() {
        let p = plain("a", "Okena", "/x");
        assert!(searching("  oKeNa \t").matches(&cand(&p)));
        assert!(searching("OKE").matches(&cand(&p)));
    }

    #[test]
    fn search_matches_the_polled_branch() {
        let p = plain("a", "wt", "/x/wt");
        let c = Candidate {
            branch: Some("feat/qbl-420-search"),
            ..cand(&p)
        };
        assert!(searching("qbl-420").matches(&c));
        assert!(!searching("qbl-420").matches(&cand(&p)));
    }

    #[test]
    fn search_matches_a_stored_worktree_branch() {
        let p = project(serde_json::json!({
            "id": "w", "name": "wt", "path": "/x",
            "worktree_info": { "parent_project_id": "a", "branch_name": "fix/crash" },
        }));
        assert!(searching("fix/cr").matches(&cand(&p)));
    }

    #[test]
    fn search_matches_task_key_title_and_other_tasks_keys() {
        let p = project(serde_json::json!({
            "id": "s", "name": "session", "path": "/x",
            "task_ref": task("QBL-397", "Tasks filter bar"),
            "also_tasks": [task("QBL-398", "Something unrelated")],
        }));
        assert!(searching("qbl-397").matches(&cand(&p)));
        assert!(searching("filter bar").matches(&cand(&p)));
        assert!(searching("QBL-398").matches(&cand(&p)));
        // Only the other tasks' keys are searched, not their titles.
        assert!(!searching("unrelated").matches(&cand(&p)));
    }

    #[test]
    fn chips_in_one_group_are_ored() {
        let a = plain("a", "a", "/a");
        let b = plain("b", "b", "/b");
        let c = plain("c", "c", "/c");
        let cands = [
            agent(&a, AgentActivity::NeedsInput, AgentRole::Implement),
            agent(&b, AgentActivity::Blocked, AgentRole::Implement),
            agent(&c, AgentActivity::Working, AgentRole::Implement),
        ];
        let mut f = OverviewFilter::default();
        f.toggle_state(AgentActivity::NeedsInput);
        f.toggle_state(AgentActivity::Blocked);
        assert_eq!(apply(&f, &cands), ["a", "b"]);
    }

    #[test]
    fn groups_and_search_are_anded() {
        let a = plain("a", "okena-a", "/a");
        let b = plain("b", "okena-b", "/b");
        let c = plain("c", "other", "/c");
        let d = plain("d", "okena-d", "/d");
        let cands = [
            agent(&a, AgentActivity::NeedsInput, AgentRole::Implement),
            agent(&b, AgentActivity::NeedsInput, AgentRole::Spec),
            agent(&c, AgentActivity::NeedsInput, AgentRole::Implement),
            agent(&d, AgentActivity::Working, AgentRole::Implement),
        ];
        let mut f = OverviewFilter::default();
        f.toggle_state(AgentActivity::NeedsInput);
        assert_eq!(apply(&f, &cands), ["a", "b", "c"]);
        f.set_search("okena");
        assert_eq!(apply(&f, &cands), ["a", "b"]);
        f.toggle_role(AgentRole::Implement);
        assert_eq!(apply(&f, &cands), ["a"]);
        // Toggling a chip twice takes it off again.
        f.toggle_role(AgentRole::Implement);
        assert_eq!(apply(&f, &cands), ["a", "b"]);
    }

    #[test]
    fn a_chip_excludes_projects_that_are_not_agents() {
        let p = plain("p", "okena", "/p");
        let mut f = OverviewFilter::default();
        f.toggle_role(AgentRole::Task);
        assert!(!f.matches(&cand(&p)));
    }

    #[test]
    fn worktrees_match_on_their_own_and_order_is_kept() {
        let parent = plain("parent", "okena", "/src/okena");
        let wt1 = project(serde_json::json!({
            "id": "wt1", "name": "wt1", "path": "/wt/one",
            "worktree_info": { "parent_project_id": "parent" },
        }));
        let other = plain("other", "zed", "/src/zed");
        let wt2 = project(serde_json::json!({
            "id": "wt2", "name": "wt2", "path": "/wt/two",
            "worktree_info": { "parent_project_id": "parent" },
        }));
        let cands = [
            cand(&parent),
            Candidate {
                branch: Some("feat/search"),
                ..cand(&wt1)
            },
            cand(&other),
            Candidate {
                branch: Some("feat/search-2"),
                ..cand(&wt2)
            },
        ];
        // A worktree matching does not bring in its parent.
        assert_eq!(apply(&searching("feat/search"), &cands), ["wt1", "wt2"]);
        // A parent matching does not bring in its worktrees.
        assert_eq!(apply(&searching("/src/okena"), &cands), ["parent"]);
    }

    #[test]
    fn only_present_values_are_offered_in_a_fixed_order() {
        let a = plain("a", "a", "/a");
        let b = plain("b", "b", "/b");
        let c = plain("c", "c", "/c");
        let cands = [
            agent(&a, AgentActivity::Stopped, AgentRole::Scan),
            agent(&b, AgentActivity::Working, AgentRole::Implement),
            cand(&c),
        ];
        let facets = collect_facets(&cands);
        assert_eq!(
            facets.states,
            [AgentActivity::Working, AgentActivity::Stopped]
        );
        assert_eq!(facets.roles, [AgentRole::Implement, AgentRole::Scan]);
        assert_eq!(collect_facets(&[cand(&c)]), Facets::default());
    }

    #[test]
    fn prune_drops_values_no_candidate_has_and_keeps_the_text() {
        let a = plain("a", "a", "/a");
        let mut f = searching("a");
        f.toggle_state(AgentActivity::NeedsInput);
        f.toggle_state(AgentActivity::Working);
        f.toggle_role(AgentRole::Spec);
        let facets = collect_facets(&[agent(&a, AgentActivity::Working, AgentRole::Implement)]);
        f.prune(&facets);
        assert!(f.state_selected(AgentActivity::Working));
        assert!(!f.state_selected(AgentActivity::NeedsInput));
        assert!(!f.role_selected(AgentRole::Spec));
        assert!(f.is_active());
        f.clear();
        assert!(!f.is_active());
    }
}
