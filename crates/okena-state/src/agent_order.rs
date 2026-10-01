//! The order live agent sessions are listed in, and what a drop does to it.
//!
//! Two tiers. Pinned agents come first, in the order the user arranged them,
//! and stay there whichever sort is chosen: a pin is an anchor, so it must not
//! move when an agent does something. Everything else follows in the chosen
//! sort — most recently active, or by name.
//!
//! The unit that is pinned and arranged is the *group*: a top-level agent
//! together with the sub-agents `agent_tree` puts under it. A sub-agent has no
//! place of its own — it goes where its parent goes — so its own `pinned` flag
//! is not read while it has a parent running.
//!
//! The arranged order is not a list of its own. It is the agents' positions in
//! `project_order`, which every project already has a place in and which
//! `MoveProject` already rearranges, so pinning an agent somewhere is a move
//! and a pin, with nothing new to persist.
//!
//! One definition for both lists that show agents — the Agents list and the
//! agent rows under a worktree in the Projects list (`agent_links`) — so they
//! cannot disagree about the order.

use crate::agent_tree::{self, AgentNode};
use crate::{AgentSortMode, ProjectData};
use std::collections::HashMap;

/// A live session in its place in the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderedAgent {
    pub id: String,
    /// 0 for a top-level agent, 1 for its sub-agents, and so on.
    pub depth: usize,
    /// Whether its group is in the pinned tier: the top-level agent's own
    /// pin, which its sub-agents share.
    pub pinned: bool,
}

/// A session's name as the lists show it: without the kind suffix older
/// sessions carry, which the badge already says.
fn listed_name(p: &ProjectData) -> &str {
    p.agent_role()
        .and_then(|role| role.legacy_name_suffix())
        .and_then(|suffix| p.name.strip_suffix(suffix))
        .unwrap_or(&p.name)
}

/// Put the live `sessions` in display order: pinned groups first, in their
/// `project_order` order, then the rest sorted by `mode`.
///
/// `sessions` is every live session being listed together; a session is a
/// sub-agent only of another one in it.
pub fn order_live(
    sessions: &[&ProjectData],
    project_order: &[String],
    mode: AgentSortMode,
) -> Vec<OrderedAgent> {
    let mut sorted: Vec<&ProjectData> = sessions.to_vec();
    match mode {
        // Most recent first. A session that has never run anything has no
        // activity stamp and sorts last rather than first, which is where a
        // just-created-but-idle session belongs.
        AgentSortMode::Activity => sorted.sort_by(|a, b| {
            b.last_activity_at
                .cmp(&a.last_activity_at)
                .then_with(|| listed_name(a).cmp(listed_name(b)))
        }),
        AgentSortMode::Name => sorted.sort_by_key(|p| listed_name(p).to_lowercase()),
    }

    // Who started whom, then the tickets' own hierarchy, applied after
    // sorting so the chosen order survives inside each level.
    let nodes: Vec<AgentNode> = sorted
        .iter()
        .map(|p| AgentNode {
            id: p.id.clone(),
            task_id: p.task_ref.as_ref().map(|t| t.id.external_id.clone()),
            parent_task_id: p.task_ref.as_ref().and_then(|t| t.parent_id.clone()),
            started_by: p.started_by.clone(),
        })
        .collect();
    let placed = agent_tree::arrange(&nodes);

    let pinned: HashMap<&str, bool> = sorted.iter().map(|p| (p.id.as_str(), p.pinned)).collect();
    let rank = |id: &str| {
        project_order
            .iter()
            .position(|o| o == id)
            .unwrap_or(usize::MAX)
    };

    // Split into groups, each a top-level agent and what follows it.
    let mut groups: Vec<Vec<OrderedAgent>> = Vec::new();
    for agent in placed {
        let row = OrderedAgent {
            id: agent.id,
            depth: agent.depth,
            pinned: false,
        };
        match groups.last_mut() {
            Some(group) if row.depth > 0 => group.push(row),
            _ => groups.push(vec![row]),
        }
    }
    let (mut top, rest): (Vec<_>, Vec<_>) = groups
        .into_iter()
        .partition(|group| pinned.get(group[0].id.as_str()).copied().unwrap_or(false));
    // Stable, so pinned agents with no place in `project_order` keep the
    // sorted order among themselves, after the ones that have one.
    top.sort_by_key(|group| rank(&group[0].id));
    for row in top.iter_mut().flatten() {
        row.pinned = true;
    }
    top.into_iter().chain(rest).flatten().collect()
}

/// Where in the Agents list a dragged agent was let go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentDrop {
    /// On the pinned agent with this id: it lands just above it.
    BeforePinned(String),
    /// Below the last pinned agent, still in the pinned tier. Also what the
    /// menu's "Pin to top" means: a newly pinned agent joins at the end, so
    /// agents sit in the order they were pinned.
    EndOfPinned,
    /// Among the agents that are not pinned.
    Unpinned,
}

/// What a drop comes to, in the actions that already exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DropPlan {
    /// `MoveProject` to this index of `project_order`, when its place among
    /// the pinned agents changes.
    pub move_to: Option<usize>,
    /// `ToggleProjectPinned`, when it changes tier.
    pub toggle_pinned: bool,
}

/// What dropping the agent `dragged` at `drop` does, or `None` when it does
/// nothing.
///
/// `roots` is the top-level agents in display order, each with whether it is
/// pinned. An agent that is not among them is a sub-agent, and cannot be
/// placed apart from its parent.
///
/// `project_order` must be the order `MoveProject` will index into: the
/// entries of the daemon that owns `dragged`, in its order.
pub fn plan_drop(
    dragged: &str,
    drop: &AgentDrop,
    roots: &[(String, bool)],
    project_order: &[String],
) -> Option<DropPlan> {
    let was_pinned = roots.iter().find(|(id, _)| id == dragged)?.1;
    let pinned: Vec<&str> = roots
        .iter()
        .filter(|(_, pinned)| *pinned)
        .map(|(id, _)| id.as_str())
        .collect();
    // `MoveProject` takes the agent out before putting it back, so the index
    // counts the entries without it.
    let index_of = |id: &str| {
        project_order
            .iter()
            .filter(|o| o.as_str() != dragged)
            .position(|o| o == id)
    };

    let move_to = match drop {
        AgentDrop::Unpinned => {
            return was_pinned.then_some(DropPlan {
                move_to: None,
                toggle_pinned: true,
            });
        }
        AgentDrop::BeforePinned(target) => {
            let at = pinned.iter().position(|id| *id == target.as_str())?;
            // On itself, or on the agent it already sits above.
            if target == dragged || (at > 0 && pinned[at - 1] == dragged) {
                return None;
            }
            index_of(target)
        }
        AgentDrop::EndOfPinned => match pinned.iter().copied().rfind(|id| *id != dragged) {
            // Already the last one.
            Some(_) if pinned.last() == Some(&dragged) => return None,
            Some(last) => index_of(last).map(|i| i + 1),
            // Nothing else is pinned: there is no order to take a place in.
            None => None,
        },
    };
    let plan = DropPlan {
        move_to,
        toggle_pinned: !was_pinned,
    };
    (plan.move_to.is_some() || plan.toggle_pinned).then_some(plan)
}

#[cfg(test)]
mod tests {
    use super::{AgentDrop, DropPlan, order_live, plan_drop};
    use crate::{AgentSortMode, ProjectData};

    /// A free-form session, last active at `at`.
    fn session(id: &str, at: u64) -> ProjectData {
        let mut p: ProjectData = serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "path": "/projects", "custom_session": "goal",
        }))
        .unwrap();
        p.last_activity_at = Some(at);
        p
    }

    /// A session on the task `task`, whose parent task is `parent`.
    fn on_task(id: &str, at: u64, task: &str, parent: Option<&str>) -> ProjectData {
        let mut p: ProjectData = serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "path": "/projects",
            "task_ref": {
                "id": { "provider": "linear", "external_id": task },
                "display_key": task, "title": "t", "url": "http://x",
                "parent_id": parent,
            },
        }))
        .unwrap();
        p.last_activity_at = Some(at);
        p
    }

    fn pinned(mut p: ProjectData) -> ProjectData {
        p.pinned = true;
        p
    }

    fn order(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    /// `id` for an unpinned row, `*id` for a pinned one, indented by depth.
    fn shape(sessions: &[ProjectData], project_order: &[&str], mode: AgentSortMode) -> Vec<String> {
        let refs: Vec<&ProjectData> = sessions.iter().collect();
        order_live(&refs, &order(project_order), mode)
            .into_iter()
            .map(|a| {
                format!(
                    "{}{}{}",
                    "  ".repeat(a.depth),
                    if a.pinned { "*" } else { "" },
                    a.id
                )
            })
            .collect()
    }

    #[test]
    fn agents_a_coordinator_started_follow_it_whatever_their_tickets_say() {
        // Three tasks picked together, none a child of another. The
        // coordinator is named after the first; the tickets list all loose.
        let started = |id: &str, at: u64, task: &str| {
            let mut p = on_task(id, at, task, None);
            p.started_by = Some("coord".into());
            p
        };
        let sessions = [
            started("b", 40, "B"),
            on_task("other", 30, "Z", None),
            started("a", 20, "A"),
            on_task("coord", 10, "A", None),
        ];
        assert_eq!(
            shape(&sessions, &[], AgentSortMode::Activity),
            ["other", "coord", "  b", "  a"]
        );
        // And they go where the coordinator goes when it is pinned.
        let mut pinned_coord = sessions.clone();
        pinned_coord[3].pinned = true;
        assert_eq!(
            shape(&pinned_coord, &["coord"], AgentSortMode::Activity),
            ["*coord", "  *b", "  *a", "other"]
        );
    }

    #[test]
    fn with_nothing_pinned_the_sort_decides() {
        let sessions = [session("b", 30), session("a", 10), session("c", 20)];
        assert_eq!(
            shape(&sessions, &["a", "b", "c"], AgentSortMode::Activity),
            ["b", "c", "a"]
        );
        assert_eq!(
            shape(&sessions, &["a", "b", "c"], AgentSortMode::Name),
            ["a", "b", "c"]
        );
    }

    #[test]
    fn pinned_agents_come_first_in_their_arranged_order_in_both_sorts() {
        // `z` and `m` are pinned and arranged z-then-m, which neither sort
        // would produce: `m` is the more recent and the earlier name.
        let sessions = [
            session("a", 50),
            pinned(session("m", 40)),
            pinned(session("z", 10)),
            session("b", 60),
        ];
        let arranged = ["a", "z", "b", "m"];
        assert_eq!(
            shape(&sessions, &arranged, AgentSortMode::Activity),
            ["*z", "*m", "b", "a"]
        );
        assert_eq!(
            shape(&sessions, &arranged, AgentSortMode::Name),
            ["*z", "*m", "a", "b"]
        );
    }

    #[test]
    fn activity_never_reorders_the_pinned_tier() {
        let mut sessions = [pinned(session("first", 10)), pinned(session("second", 20))];
        let arranged = ["first", "second"];
        assert_eq!(
            shape(&sessions, &arranged, AgentSortMode::Activity),
            ["*first", "*second"]
        );
        sessions[1].last_activity_at = Some(999);
        assert_eq!(
            shape(&sessions, &arranged, AgentSortMode::Activity),
            ["*first", "*second"]
        );
    }

    #[test]
    fn sub_agents_go_where_their_parent_goes() {
        // The epic's agent is pinned; its two story agents come with it,
        // ahead of an unpinned agent more recent than any of them.
        let sessions = [
            session("solo", 90),
            pinned(on_task("epic", 10, "E", None)),
            on_task("story-1", 20, "S1", Some("E")),
            on_task("story-2", 30, "S2", Some("E")),
        ];
        assert_eq!(
            shape(
                &sessions,
                &["solo", "epic", "story-1", "story-2"],
                AgentSortMode::Activity
            ),
            ["*epic", "  *story-2", "  *story-1", "solo"]
        );
    }

    #[test]
    fn a_sub_agents_own_pin_does_not_lift_it_out_of_its_group() {
        // Pinned while it stood alone, perhaps; now its parent is running, so
        // it sits under the parent, which is not pinned.
        let sessions = [
            session("solo", 90),
            on_task("epic", 10, "E", None),
            pinned(on_task("story", 20, "S", Some("E"))),
        ];
        assert_eq!(
            shape(
                &sessions,
                &["story", "epic", "solo"],
                AgentSortMode::Activity
            ),
            ["solo", "epic", "  story"]
        );
    }

    #[test]
    fn a_pinned_agent_missing_from_the_order_goes_after_the_placed_ones() {
        let sessions = [pinned(session("lost", 99)), pinned(session("placed", 1))];
        assert_eq!(
            shape(&sessions, &["placed"], AgentSortMode::Activity),
            ["*placed", "*lost"]
        );
    }

    // ---- drops ----

    fn roots(rows: &[(&str, bool)]) -> Vec<(String, bool)> {
        rows.iter().map(|(id, p)| (id.to_string(), *p)).collect()
    }

    /// `p1`, `p2` pinned in that order; `u1`, `u2` not. Repos sit between
    /// them in `project_order`, as they do in a real workspace.
    fn list() -> (Vec<(String, bool)>, Vec<String>) {
        (
            roots(&[("p1", true), ("p2", true), ("u1", false), ("u2", false)]),
            order(&["repo-a", "u1", "p1", "repo-b", "p2", "u2"]),
        )
    }

    fn moved(dragged: &str, to: usize, project_order: &[String]) -> Vec<String> {
        let mut out: Vec<String> = project_order
            .iter()
            .filter(|o| *o != dragged)
            .cloned()
            .collect();
        out.insert(to.min(out.len()), dragged.to_string());
        out
    }

    #[test]
    fn dropping_an_unpinned_agent_on_a_pinned_one_pins_it_just_above() {
        let (roots, project_order) = list();
        let plan = plan_drop(
            "u2",
            &AgentDrop::BeforePinned("p2".into()),
            &roots,
            &project_order,
        )
        .unwrap();
        assert!(plan.toggle_pinned);
        // Applied as `MoveProject` applies it, it lands between p1 and p2.
        assert_eq!(
            moved("u2", plan.move_to.unwrap(), &project_order),
            order(&["repo-a", "u1", "p1", "repo-b", "u2", "p2"])
        );
    }

    #[test]
    fn dropping_an_agent_from_above_counts_the_index_without_it() {
        // u1 sits before p2 in `project_order`, so taking it out shifts p2.
        let (roots, project_order) = list();
        let plan = plan_drop(
            "u1",
            &AgentDrop::BeforePinned("p2".into()),
            &roots,
            &project_order,
        )
        .unwrap();
        assert_eq!(
            moved("u1", plan.move_to.unwrap(), &project_order),
            order(&["repo-a", "p1", "repo-b", "u1", "p2", "u2"])
        );
    }

    #[test]
    fn dropping_at_the_end_of_the_pinned_tier_pins_it_last() {
        let (roots, project_order) = list();
        let plan = plan_drop("u1", &AgentDrop::EndOfPinned, &roots, &project_order).unwrap();
        assert!(plan.toggle_pinned);
        assert_eq!(
            moved("u1", plan.move_to.unwrap(), &project_order),
            order(&["repo-a", "p1", "repo-b", "p2", "u1", "u2"])
        );
    }

    #[test]
    fn reordering_inside_the_pinned_tier_moves_without_unpinning() {
        let (roots, project_order) = list();
        let plan = plan_drop(
            "p2",
            &AgentDrop::BeforePinned("p1".into()),
            &roots,
            &project_order,
        )
        .unwrap();
        assert!(!plan.toggle_pinned);
        assert_eq!(
            moved("p2", plan.move_to.unwrap(), &project_order),
            order(&["repo-a", "u1", "p2", "p1", "repo-b", "u2"])
        );
        // And the first to the end.
        let plan = plan_drop("p1", &AgentDrop::EndOfPinned, &roots, &project_order).unwrap();
        assert!(!plan.toggle_pinned);
        assert_eq!(
            moved("p1", plan.move_to.unwrap(), &project_order),
            order(&["repo-a", "u1", "repo-b", "p2", "p1", "u2"])
        );
    }

    #[test]
    fn dropping_a_pinned_agent_among_the_unpinned_unpins_it_where_it_is() {
        let (roots, project_order) = list();
        assert_eq!(
            plan_drop("p1", &AgentDrop::Unpinned, &roots, &project_order),
            Some(DropPlan {
                move_to: None,
                toggle_pinned: true
            })
        );
    }

    #[test]
    fn a_drop_that_changes_nothing_does_nothing() {
        let (roots, project_order) = list();
        let nothing = |dragged: &str, drop: AgentDrop| {
            assert_eq!(
                plan_drop(dragged, &drop, &roots, &project_order),
                None,
                "{dragged} at {drop:?}"
            );
        };
        // An unpinned agent among the unpinned: the sort places it.
        nothing("u1", AgentDrop::Unpinned);
        // On itself, on the one it is already above, and last at the end.
        nothing("p1", AgentDrop::BeforePinned("p1".into()));
        nothing("p1", AgentDrop::BeforePinned("p2".into()));
        nothing("p2", AgentDrop::EndOfPinned);
        // On an agent that is not pinned, named as if it were.
        nothing("p1", AgentDrop::BeforePinned("u1".into()));
    }

    #[test]
    fn a_sub_agent_cannot_be_placed_on_its_own() {
        // Not among the top-level agents, so nothing it is dropped on moves it.
        let (roots, project_order) = list();
        for drop in [
            AgentDrop::BeforePinned("p1".into()),
            AgentDrop::EndOfPinned,
            AgentDrop::Unpinned,
        ] {
            assert_eq!(plan_drop("story", &drop, &roots, &project_order), None);
        }
    }

    #[test]
    fn the_first_pin_only_pins() {
        let roots = roots(&[("u1", false), ("u2", false)]);
        assert_eq!(
            plan_drop("u2", &AgentDrop::EndOfPinned, &roots, &order(&["u1", "u2"])),
            Some(DropPlan {
                move_to: None,
                toggle_pinned: true
            })
        );
        // The only pinned agent dropped at the end of its own tier.
        let roots = self::roots(&[("p", true), ("u", false)]);
        assert_eq!(
            plan_drop("p", &AgentDrop::EndOfPinned, &roots, &order(&["p", "u"])),
            None
        );
    }

    #[test]
    fn pinning_from_the_menu_keeps_the_order_pinned_in() {
        // Pin `late` (first in `project_order`) after `early` is already
        // pinned: it joins below `early`, not above it.
        let sessions = [pinned(session("early", 1)), session("late", 2)];
        let project_order = order(&["late", "repo", "early"]);
        let roots = roots(&[("early", true), ("late", false)]);
        let plan = plan_drop("late", &AgentDrop::EndOfPinned, &roots, &project_order).unwrap();
        let after = moved("late", plan.move_to.unwrap(), &project_order);
        let mut sessions = sessions.to_vec();
        sessions[1].pinned = plan.toggle_pinned;
        let refs: Vec<&ProjectData> = sessions.iter().collect();
        let ids: Vec<String> = order_live(&refs, &after, AgentSortMode::Activity)
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(ids, ["early", "late"]);
    }
}
