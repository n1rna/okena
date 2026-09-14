//! The task list as a tree: which ancestors to load so every listed task can
//! sit under its parents, even when those parents are not in your queue.

use std::collections::HashSet;

use okena_core::tasks::Task;

use super::tasks_view::{TaskRow, order_rows};

/// A section's rows: its own tasks nested under their parents, with any
/// ancestor that is not one of them shown as a context row.
///
/// `real` are the section's tasks, in order; their count is the section's
/// count, and only they are ever real rows. `lookup` finds any other task okena
/// has loaded — the rest of your queue, whatever section or filter it falls
/// in, and the ancestors loaded for it.
///
/// Each context ancestor enters just before the first of its descendants, so
/// the tree lands where that task would have been.
pub(super) fn section_rows<'a>(
    real: Vec<Task>,
    lookup: impl Fn(&str) -> Option<&'a Task>,
    collapsed: &HashSet<String>,
) -> Vec<TaskRow> {
    let real_ids: HashSet<String> = real.iter().map(|t| t.id.external_id.clone()).collect();
    let mut context: HashSet<String> = HashSet::new();
    let mut tasks: Vec<Task> = Vec::with_capacity(real.len());

    for task in real {
        // Nearest first; stops at the section's own tasks, at a context row
        // already added for a sibling, at a parent nobody loaded, and on a
        // cycle.
        let mut chain: Vec<Task> = Vec::new();
        let mut seen: HashSet<String> = HashSet::from([task.id.external_id.clone()]);
        let mut next = task.parent_id.clone();
        while let Some(parent_id) = next {
            if real_ids.contains(&parent_id)
                || context.contains(&parent_id)
                || !seen.insert(parent_id.clone())
            {
                break;
            }
            let Some(parent) = lookup(&parent_id) else {
                break;
            };
            next = parent.parent_id.clone();
            chain.push(parent.clone());
        }
        for ancestor in chain.into_iter().rev() {
            context.insert(ancestor.id.external_id.clone());
            tasks.push(ancestor);
        }
        tasks.push(task);
    }

    order_rows(tasks, &context, collapsed)
}

/// Load the ancestors of `listed` that are not listed themselves, all the way
/// to the top of each chain.
///
/// One `fetch` per level, never one per task: every parent a level is missing
/// goes in the same batch. An id is asked for once, so a parent the provider
/// does not return — deleted, or out of reach — ends that chain rather than
/// being asked for again, and a parent cycle ends when it comes back round.
///
/// Returns what was loaded, top levels last, and the error that stopped the
/// walk if one did: the levels loaded before it still hold the tree together
/// that far.
pub(super) fn walk_ancestors<E>(
    listed: &[Task],
    mut fetch: impl FnMut(Vec<String>) -> Result<Vec<Task>, E>,
) -> (Vec<Task>, Option<E>) {
    let mut known: HashSet<String> = listed.iter().map(|t| t.id.external_id.clone()).collect();
    let mut asked: HashSet<String> = HashSet::new();
    let mut found: Vec<Task> = Vec::new();

    let mut level = next_level(listed, &known, &asked);
    while !level.is_empty() {
        asked.extend(level.iter().cloned());
        let fresh: Vec<Task> = match fetch(level) {
            Ok(tasks) => tasks
                .into_iter()
                .filter(|t| known.insert(t.id.external_id.clone()))
                .collect(),
            Err(e) => return (found, Some(e)),
        };
        level = next_level(&fresh, &known, &asked);
        found.extend(fresh);
    }
    (found, None)
}

/// Parents of `tasks` that nobody has loaded or asked for yet, each once, in
/// the order they are first named.
fn next_level(tasks: &[Task], known: &HashSet<String>, asked: &HashSet<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for parent in tasks.iter().filter_map(|t| t.parent_id.as_ref()) {
        if !known.contains(parent) && !asked.contains(parent) && !out.contains(parent) {
            out.push(parent.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::walk_ancestors;
    use okena_core::tasks::{Task, TaskId, TaskKind, TaskState};
    use std::collections::HashMap;

    fn task(id: &str, parent: Option<&str>) -> Task {
        Task {
            id: TaskId::new("linear", id),
            display_key: id.to_uppercase(),
            title: format!("Task {id}"),
            description: None,
            state: TaskState::Todo,
            state_name: "Todo".into(),
            url: String::new(),
            branch_name: String::new(),
            updated_at: String::new(),
            kind: TaskKind::Task,
            parent_id: parent.map(str::to_string),
            parent_key: parent.map(str::to_uppercase),
            labels: Vec::new(),
            groups: Vec::new(),
        }
    }

    /// A provider holding `tasks`, recording every batch it is asked for.
    fn provider(tasks: &[Task]) -> (HashMap<String, Task>, std::cell::RefCell<Vec<Vec<String>>>) {
        let by_id = tasks
            .iter()
            .map(|t| (t.id.external_id.clone(), t.clone()))
            .collect();
        (by_id, std::cell::RefCell::new(Vec::new()))
    }

    fn ids(tasks: &[Task]) -> Vec<&str> {
        tasks.iter().map(|t| t.id.external_id.as_str()).collect()
    }

    #[test]
    fn the_whole_chain_loads_one_level_per_batch() {
        let (remote, asked) = provider(&[task("feature", Some("epic")), task("epic", None)]);
        let listed = [task("story", Some("feature"))];

        let (found, error) = walk_ancestors::<()>(&listed, |level| {
            asked.borrow_mut().push(level.clone());
            Ok(level
                .iter()
                .filter_map(|id| remote.get(id).cloned())
                .collect())
        });

        assert!(error.is_none());
        assert_eq!(ids(&found), ["feature", "epic"]);
        assert_eq!(*asked.borrow(), [vec!["feature"], vec!["epic"]]);
    }

    #[test]
    fn a_level_is_one_batch_however_many_tasks_need_it() {
        let (remote, asked) = provider(&[
            task("f1", Some("epic")),
            task("f2", Some("epic")),
            task("epic", None),
        ]);
        let listed = [
            task("s1", Some("f1")),
            task("s2", Some("f1")),
            task("s3", Some("f2")),
        ];

        let (found, _) = walk_ancestors::<()>(&listed, |level| {
            asked.borrow_mut().push(level.clone());
            Ok(level
                .iter()
                .filter_map(|id| remote.get(id).cloned())
                .collect())
        });

        assert_eq!(ids(&found), ["f1", "f2", "epic"]);
        assert_eq!(*asked.borrow(), [vec!["f1", "f2"], vec!["epic"]]);
    }

    #[test]
    fn a_parent_already_listed_is_not_loaded() {
        let listed = [
            task("story", Some("feature")),
            task("feature", Some("epic")),
            task("epic", None),
        ];
        let (found, error) = walk_ancestors::<()>(&listed, |level| {
            panic!("nothing is missing, yet {level:?} was asked for")
        });
        assert!(found.is_empty());
        assert!(error.is_none());
    }

    #[test]
    fn only_what_is_missing_is_loaded_above_a_listed_parent() {
        let (remote, asked) = provider(&[task("epic", None)]);
        let listed = [
            task("story", Some("feature")),
            task("feature", Some("epic")),
        ];

        let (found, _) = walk_ancestors::<()>(&listed, |level| {
            asked.borrow_mut().push(level.clone());
            Ok(level
                .iter()
                .filter_map(|id| remote.get(id).cloned())
                .collect())
        });

        assert_eq!(ids(&found), ["epic"]);
        assert_eq!(*asked.borrow(), [vec!["epic"]]);
    }

    #[test]
    fn a_parent_cycle_ends_the_walk() {
        let (remote, asked) = provider(&[task("a", Some("b")), task("b", Some("a"))]);
        let listed = [task("story", Some("a"))];

        let (found, _) = walk_ancestors::<()>(&listed, |level| {
            asked.borrow_mut().push(level.clone());
            Ok(level
                .iter()
                .filter_map(|id| remote.get(id).cloned())
                .collect())
        });

        assert_eq!(ids(&found), ["a", "b"]);
        assert_eq!(*asked.borrow(), [vec!["a"], vec!["b"]]);
    }

    #[test]
    fn a_cycle_back_into_the_listed_tasks_ends_the_walk() {
        let (remote, asked) = provider(&[task("feature", Some("story"))]);
        let listed = [task("story", Some("feature"))];

        let (found, _) = walk_ancestors::<()>(&listed, |level| {
            asked.borrow_mut().push(level.clone());
            Ok(level
                .iter()
                .filter_map(|id| remote.get(id).cloned())
                .collect())
        });

        assert_eq!(ids(&found), ["feature"]);
        assert_eq!(asked.borrow().len(), 1);
    }

    #[test]
    fn a_parent_the_provider_does_not_return_is_not_asked_for_again() {
        let asked = std::cell::RefCell::new(Vec::new());
        let listed = [task("s1", Some("gone")), task("s2", Some("gone"))];

        let (found, error) = walk_ancestors::<()>(&listed, |level| {
            asked.borrow_mut().push(level);
            Ok(Vec::new())
        });

        assert!(found.is_empty());
        assert!(error.is_none());
        assert_eq!(*asked.borrow(), [vec!["gone"]]);
    }

    #[test]
    fn a_failed_level_keeps_the_levels_loaded_before_it() {
        let listed = [task("story", Some("feature"))];
        let mut calls = 0;

        let (found, error) = walk_ancestors(&listed, |level| {
            calls += 1;
            match level.as_slice() {
                [id] if id == "feature" => Ok(vec![task("feature", Some("epic"))]),
                _ => Err("provider is down"),
            }
        });

        assert_eq!(ids(&found), ["feature"]);
        assert_eq!(error, Some("provider is down"));
        assert_eq!(calls, 2);
    }

    // ── Section rows ─────────────────────────────────────────────────────────

    use super::section_rows;
    use std::collections::HashSet;

    /// (id, depth, context) per row, in display order.
    fn rows(real: Vec<Task>, pool: &[Task], collapsed: &[&str]) -> Vec<(String, usize, bool)> {
        let collapsed: HashSet<String> = collapsed.iter().map(|s| s.to_string()).collect();
        section_rows(
            real,
            |id| pool.iter().find(|t| t.id.external_id == id),
            &collapsed,
        )
        .into_iter()
        .map(|r| (r.task.id.external_id, r.depth, r.context))
        .collect()
    }

    fn row(id: &str, depth: usize, context: bool) -> (String, usize, bool) {
        (id.to_string(), depth, context)
    }

    #[test]
    fn a_section_holding_the_whole_chain_nests_three_deep_with_no_context() {
        let real = vec![
            task("story", Some("feature")),
            task("epic", None),
            task("feature", Some("epic")),
        ];
        assert_eq!(
            rows(real, &[], &[]),
            [
                row("epic", 0, false),
                row("feature", 1, false),
                row("story", 2, false)
            ]
        );
    }

    #[test]
    fn ancestors_outside_the_section_are_context_rows_above_it() {
        let pool = [task("feature", Some("epic")), task("epic", None)];
        let real = vec![task("other", None), task("story", Some("feature"))];

        let got = rows(real, &pool, &[]);
        assert_eq!(
            got,
            [
                row("other", 0, false),
                row("epic", 0, true),
                row("feature", 1, true),
                row("story", 2, false),
            ]
        );
        // The section counts what is its own.
        assert_eq!(got.iter().filter(|(_, _, context)| !context).count(), 2);
    }

    #[test]
    fn a_running_childs_parent_is_context_in_active_and_real_in_tasks() {
        // The feature is in your queue with no agent; the story has one.
        let feature = task("feature", Some("epic"));
        let epic = task("epic", None);
        let story = task("story", Some("feature"));
        let queue = [feature.clone(), epic.clone(), story.clone()];

        let active = rows(vec![story], &queue, &[]);
        assert_eq!(
            active,
            [
                row("epic", 0, true),
                row("feature", 1, true),
                row("story", 2, false)
            ]
        );

        let rest = rows(vec![feature, epic], &queue, &[]);
        assert_eq!(rest, [row("epic", 0, false), row("feature", 1, false)]);
    }

    #[test]
    fn a_filtered_out_ancestor_keeps_the_match_nested_under_it() {
        // Filtering to the story's iteration: the feature is in your queue,
        // in another iteration, so it did not match.
        let queue = [
            task("feature", None),
            task("story", Some("feature")),
            task("sibling", Some("feature")),
        ];
        let matched = vec![task("story", Some("feature"))];

        assert_eq!(
            rows(matched, &queue, &[]),
            [row("feature", 0, true), row("story", 1, false)]
        );
    }

    #[test]
    fn a_real_parent_between_context_rows_stays_real() {
        let pool = [task("epic", None)];
        let real = vec![
            task("story", Some("feature")),
            task("feature", Some("epic")),
        ];

        assert_eq!(
            rows(real, &pool, &[]),
            [
                row("epic", 0, true),
                row("feature", 1, false),
                row("story", 2, false)
            ]
        );
    }

    #[test]
    fn siblings_share_one_context_parent() {
        let pool = [task("feature", None)];
        let real = vec![task("s1", Some("feature")), task("s2", Some("feature"))];

        assert_eq!(
            rows(real, &pool, &[]),
            [
                row("feature", 0, true),
                row("s1", 1, false),
                row("s2", 1, false)
            ]
        );
    }

    #[test]
    fn a_folded_context_row_hides_its_subtree_and_can_still_unfold() {
        let pool = [task("feature", Some("epic")), task("epic", None)];
        let real = vec![task("story", Some("feature"))];
        let collapsed: HashSet<String> = HashSet::from(["feature".to_string()]);

        let got = section_rows(
            real,
            |id| pool.iter().find(|t| t.id.external_id == id),
            &collapsed,
        );
        let shown: Vec<_> = got
            .iter()
            .map(|r| (r.task.id.external_id.as_str(), r.context, r.has_children))
            .collect();
        assert_eq!(shown, [("epic", true, true), ("feature", true, true)]);
    }

    #[test]
    fn an_unloaded_parent_leaves_the_task_a_root() {
        let real = vec![task("story", Some("not-loaded"))];
        assert_eq!(rows(real, &[], &[]), [row("story", 0, false)]);
    }

    #[test]
    fn a_parent_cycle_among_loaded_tasks_does_not_loop() {
        let pool = [task("a", Some("b")), task("b", Some("a"))];
        let real = vec![task("story", Some("a"))];

        let got = rows(real, &pool, &[]);
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(got.iter().filter(|(_, _, context)| !context).count(), 1);
    }
}
