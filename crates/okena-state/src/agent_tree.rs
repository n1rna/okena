//! Which agent session sits under which.
//!
//! An epic's agent and its stories' agents are related whether or not okena
//! started them together: the relation belongs to the tickets, and okena is
//! only reading it back. That matters because the two ways a hierarchy appears
//! look nothing alike — one agent breaking an epic down and filing children,
//! versus somebody starting work on three stories over an afternoon — and a
//! grouping that only understood the first would be wrong most of the time.
//!
//! So this takes sessions that each know their own task and their task's
//! parent, and returns them in reading order with a depth on each. Nothing
//! here knows about okena's UI, its projects, or how a session was launched.
//!
//! Kept apart from the sidebar because it is the foundation for more than
//! drawing an indent: handing a parent's context down to its children, rolling
//! a subtree's status up to the epic, and stopping a tree at once all need the
//! same answer to "what is under this".

use std::collections::{HashMap, HashSet};

/// One agent session, as far as the hierarchy is concerned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentNode {
    /// The session's project id — how a caller gets back to the real thing.
    pub id: String,
    /// Provider id of the task this session is on, if any.
    pub task_id: Option<String>,
    /// Provider id of that task's parent, if it has one.
    pub parent_task_id: Option<String>,
}

/// A session placed in the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacedAgent {
    pub id: String,
    /// 0 for a session with no running parent, 1 for its children, and so on.
    pub depth: usize,
    /// Whether anything sits under it.
    pub has_children: bool,
}

/// Arrange `nodes` so every session follows its parent, indented.
///
/// A session is a child only when the agent on its task's parent is *also*
/// running: an orphan is shown at the top level rather than hidden or
/// indented under nothing, because the alternative is work that has quietly
/// left the list.
///
/// Incoming order is preserved among siblings and among roots, so whatever the
/// caller sorted by — activity, name — survives inside each level.
///
/// Cycles cannot come from a well-formed provider, but a task that is somehow
/// its own ancestor must not hang the sidebar, so a node already placed is
/// never placed again.
pub fn arrange(nodes: &[AgentNode]) -> Vec<PlacedAgent> {
    // Which session is on which task. First wins: two sessions on one task is
    // legitimate (a second agent on the same story), and the later ones are
    // siblings rather than children of the first.
    let mut session_for_task: HashMap<&str, &str> = HashMap::new();
    for node in nodes {
        if let Some(task) = node.task_id.as_deref() {
            session_for_task.entry(task).or_insert(&node.id);
        }
    }

    // A node's parent session, when its task's parent has one running and it
    // is not the node itself.
    let parent_of: HashMap<&str, &str> = nodes
        .iter()
        .filter_map(|node| {
            let parent_task = node.parent_task_id.as_deref()?;
            let parent = *session_for_task.get(parent_task)?;
            (parent != node.id).then_some((node.id.as_str(), parent))
        })
        .collect();

    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for node in nodes {
        if let Some(parent) = parent_of.get(node.id.as_str()) {
            children.entry(parent).or_default().push(&node.id);
        }
    }

    let mut out = Vec::with_capacity(nodes.len());
    let mut placed: HashSet<&str> = HashSet::new();
    for node in nodes {
        // Roots only: a child is emitted by its parent, below.
        if parent_of.contains_key(node.id.as_str()) {
            continue;
        }
        place(&node.id, 0, &children, &mut placed, &mut out);
    }
    // Anything left is in a cycle. It is still work someone is doing, so it
    // goes at the top level rather than disappearing.
    for node in nodes {
        if !placed.contains(node.id.as_str()) {
            place(&node.id, 0, &children, &mut placed, &mut out);
        }
    }
    out
}

fn place<'a>(
    id: &'a str,
    depth: usize,
    children: &HashMap<&'a str, Vec<&'a str>>,
    placed: &mut HashSet<&'a str>,
    out: &mut Vec<PlacedAgent>,
) {
    if !placed.insert(id) {
        return;
    }
    let kids = children.get(id).map(Vec::as_slice).unwrap_or_default();
    out.push(PlacedAgent {
        id: id.to_string(),
        depth,
        has_children: !kids.is_empty(),
    });
    for child in kids {
        place(child, depth + 1, children, placed, out);
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentNode, arrange};

    fn node(id: &str, task: Option<&str>, parent: Option<&str>) -> AgentNode {
        AgentNode {
            id: id.into(),
            task_id: task.map(str::to_string),
            parent_task_id: parent.map(str::to_string),
        }
    }

    fn shape(nodes: &[AgentNode]) -> Vec<(String, usize)> {
        arrange(nodes)
            .into_iter()
            .map(|p| (p.id, p.depth))
            .collect()
    }

    #[test]
    fn sessions_with_no_tasks_stay_a_flat_list_in_order() {
        let nodes = [node("a", None, None), node("b", None, None)];
        assert_eq!(shape(&nodes), [("a".into(), 0), ("b".into(), 0)]);
    }

    #[test]
    fn a_story_agent_sits_under_its_epics_agent() {
        let nodes = [
            node("epic", Some("E"), None),
            node("s1", Some("S1"), Some("E")),
            node("s2", Some("S2"), Some("E")),
        ];
        assert_eq!(
            shape(&nodes),
            [("epic".into(), 0), ("s1".into(), 1), ("s2".into(), 1)]
        );
    }

    #[test]
    fn a_child_started_first_still_ends_up_under_its_parent() {
        // The order agents were started in says nothing about the hierarchy —
        // this is the case a grouping built from "who launched what" gets
        // wrong.
        let nodes = [
            node("s1", Some("S1"), Some("E")),
            node("epic", Some("E"), None),
        ];
        assert_eq!(shape(&nodes), [("epic".into(), 0), ("s1".into(), 1)]);
    }

    #[test]
    fn an_agent_whose_parent_is_not_running_stays_at_the_top() {
        // Indenting under nothing, or hiding it, would both lose the fact that
        // somebody is working on it.
        let nodes = [node("s1", Some("S1"), Some("E"))];
        assert_eq!(shape(&nodes), [("s1".into(), 0)]);
    }

    #[test]
    fn three_levels_nest() {
        let nodes = [
            node("epic", Some("E"), None),
            node("feature", Some("F"), Some("E")),
            node("story", Some("S"), Some("F")),
        ];
        assert_eq!(
            shape(&nodes),
            [
                ("epic".into(), 0),
                ("feature".into(), 1),
                ("story".into(), 2)
            ]
        );
    }

    #[test]
    fn a_subtree_stays_together_when_another_root_is_between_them() {
        let nodes = [
            node("epic", Some("E"), None),
            node("loose", None, None),
            node("s1", Some("S1"), Some("E")),
        ];
        assert_eq!(
            shape(&nodes),
            [("epic".into(), 0), ("s1".into(), 1), ("loose".into(), 0)]
        );
    }

    #[test]
    fn siblings_keep_the_order_they_arrived_in() {
        // So whatever the caller sorted by survives inside each level.
        let nodes = [
            node("epic", Some("E"), None),
            node("b", Some("B"), Some("E")),
            node("a", Some("A"), Some("E")),
        ];
        assert_eq!(
            shape(&nodes),
            [("epic".into(), 0), ("b".into(), 1), ("a".into(), 1)]
        );
    }

    #[test]
    fn a_second_agent_on_one_task_is_a_sibling_not_a_child() {
        // Two agents on the same story are both doing that story. Making the
        // later one a child of the earlier would invent a hierarchy the
        // tickets do not have.
        let nodes = [
            node("first", Some("S"), Some("E")),
            node("second", Some("S"), Some("E")),
            node("epic", Some("E"), None),
        ];
        assert_eq!(
            shape(&nodes),
            [
                ("epic".into(), 0),
                ("first".into(), 1),
                ("second".into(), 1)
            ]
        );
    }

    #[test]
    fn has_children_marks_only_the_parents() {
        let nodes = [
            node("epic", Some("E"), None),
            node("s1", Some("S1"), Some("E")),
        ];
        let placed = arrange(&nodes);
        assert!(placed[0].has_children);
        assert!(!placed[1].has_children);
    }

    #[test]
    fn a_cycle_terminates_and_keeps_every_session() {
        // Impossible from a well-formed provider, but it must not hang the
        // sidebar, and the work must not vanish from it either.
        let nodes = [
            node("a", Some("A"), Some("B")),
            node("b", Some("B"), Some("A")),
        ];
        let placed = arrange(&nodes);
        assert_eq!(placed.len(), 2, "{placed:?}");
    }

    #[test]
    fn a_task_that_is_its_own_parent_is_not_its_own_child() {
        let nodes = [node("a", Some("A"), Some("A"))];
        assert_eq!(shape(&nodes), [("a".into(), 0)]);
    }

    #[test]
    fn every_session_is_placed_exactly_once() {
        let nodes = [
            node("epic", Some("E"), None),
            node("s1", Some("S1"), Some("E")),
            node("s2", Some("S2"), Some("E")),
            node("orphan", Some("O"), Some("gone")),
            node("loose", None, None),
        ];
        let placed = arrange(&nodes);
        assert_eq!(placed.len(), nodes.len());
        let mut ids: Vec<&str> = placed.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["epic", "loose", "orphan", "s1", "s2"]);
    }
}
