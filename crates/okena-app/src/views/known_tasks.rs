//! What okena last heard about tasks from their provider, shared app-wide.
//!
//! Whoever loads a task publishes its state here: the Tasks view for the queue
//! and fetched children, a session panel for the tasks its agent filed. Panels
//! observe it, so a state learned by one view shows everywhere the task does.
//!
//! Keyed by provider and id together. A Linear UUID and an Azure DevOps work
//! item number live in different id spaces, and nothing stops two providers
//! handing out the same string.
//!
//! Created on first use rather than at startup, so a panel that opens before
//! anything has loaded still has something to observe.

use gpui::*;
use okena_core::session_assets::SessionAsset;
use okena_core::tasks::{Task, TaskId, TaskRef};
use std::collections::HashMap;

#[derive(Default)]
pub struct KnownTasks {
    /// The provider's name for each task's state.
    states: HashMap<TaskId, String>,
}

impl KnownTasks {
    pub fn state_of(&self, id: &TaskId) -> Option<&str> {
        self.states.get(id).map(String::as_str)
    }
}

pub struct GlobalKnownTasks(pub Entity<KnownTasks>);

impl Global for GlobalKnownTasks {}

/// The shared entity, created on first use.
pub fn entity(cx: &mut App) -> Entity<KnownTasks> {
    if let Some(global) = cx.try_global::<GlobalKnownTasks>() {
        return global.0.clone();
    }
    let known = cx.new(|_| KnownTasks::default());
    cx.set_global(GlobalKnownTasks(known.clone()));
    known
}

/// Remember the state of tasks just loaded.
///
/// Merges rather than replaces: tasks arrive from several places, and a load
/// of one set must not forget the others. Observers are notified only when a
/// state actually changed.
pub fn remember<'a>(tasks: impl IntoIterator<Item = &'a Task>, cx: &mut App) {
    let loaded: Vec<(TaskId, String)> = tasks
        .into_iter()
        .map(|t| (t.id.clone(), state_name(t)))
        .collect();
    if loaded.is_empty() {
        return;
    }
    entity(cx).update(cx, |known, cx| {
        let mut changed = false;
        for (id, state) in loaded {
            if known.states.get(&id) != Some(&state) {
                known.states.insert(id, state);
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    });
}

/// Where `task` last stood, when okena has heard. `None` rather than a guess
/// from what the task was when it was filed.
pub fn task_state(task: Option<&TaskRef>, cx: &App) -> Option<String> {
    let task = task?;
    cx.try_global::<GlobalKnownTasks>()
        .and_then(|g| g.0.read(cx).state_of(&task.id).map(str::to_string))
}

/// The team's own name for the state, or okena's when the provider gave none.
fn state_name(task: &Task) -> String {
    if task.state_name.trim().is_empty() {
        task.state.label().to_string()
    } else {
        task.state_name.clone()
    }
}

/// How long a fetched state is trusted before a panel showing it asks again.
///
/// A task moves on the provider without telling okena, so a panel left open
/// has to look again; a minute keeps it current without polling a provider
/// harder than anyone reading the row would notice.
pub const STATE_STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(60);

/// Tasks among `rows` that are due a fetch, grouped by provider.
///
/// `requested` is when each task was last asked for. A task asked for within
/// `stale_after` of `now` is left out — its answer is fresh, or still on its
/// way — and anything older is asked for again. One batch per provider.
pub fn tasks_to_fetch<'a>(
    rows: impl IntoIterator<Item = &'a SessionAsset>,
    requested: &HashMap<TaskId, std::time::Instant>,
    now: std::time::Instant,
    stale_after: std::time::Duration,
) -> Vec<(String, Vec<String>)> {
    let mut by_provider: Vec<(String, Vec<String>)> = Vec::new();
    for task in rows.into_iter().filter_map(|r| r.task.as_ref()) {
        if requested
            .get(&task.id)
            .is_some_and(|at| now.saturating_duration_since(*at) < stale_after)
        {
            continue;
        }
        let ids = match by_provider
            .iter_mut()
            .position(|(p, _)| *p == task.id.provider)
        {
            Some(i) => &mut by_provider[i].1,
            None => {
                by_provider.push((task.id.provider.clone(), Vec::new()));
                let last = by_provider.len() - 1;
                &mut by_provider[last].1
            }
        };
        if !ids.contains(&task.id.external_id) {
            ids.push(task.id.external_id.clone());
        }
    }
    by_provider
}

#[cfg(test)]
mod tests {
    use super::tasks_to_fetch;
    use okena_core::harness::AgentAssetKind;
    use okena_core::session_assets::SessionAsset;
    use okena_core::tasks::{TaskId, TaskRef};
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    const WINDOW: Duration = Duration::from_secs(60);

    fn task(provider: &str, id: &str) -> SessionAsset {
        SessionAsset {
            kind: AgentAssetKind::Task,
            title: id.into(),
            task: Some(TaskRef {
                id: TaskId::new(provider, id),
                display_key: id.into(),
                title: id.into(),
                url: String::new(),
                parent_id: None,
                parent_key: None,
            }),
            registered: true,
            ..SessionAsset::default()
        }
    }

    #[test]
    fn fetches_are_one_batch_per_provider_without_repeats_or_requests_in_flight() {
        let rows = [
            task("linear", "a"),
            task("azure_devops", "7"),
            task("linear", "b"),
            task("linear", "a"),
            task("linear", "c"),
            SessionAsset::default(),
        ];
        let now = Instant::now();
        let in_flight = HashMap::from([(TaskId::new("linear", "c"), now)]);
        assert_eq!(
            tasks_to_fetch(&rows, &in_flight, now, WINDOW),
            vec![
                ("linear".to_string(), vec!["a".to_string(), "b".to_string()]),
                ("azure_devops".to_string(), vec!["7".to_string()]),
            ]
        );
    }

    #[test]
    fn a_stale_state_is_asked_for_again_and_a_fresh_one_is_not() {
        // A task that moved to Done while its panel stayed open has to show
        // it without the panel being reopened.
        let rows = [task("linear", "fresh"), task("linear", "stale")];
        let start = Instant::now();
        let now = start + Duration::from_secs(90);
        let requested = HashMap::from([
            (
                TaskId::new("linear", "fresh"),
                start + Duration::from_secs(60),
            ),
            (TaskId::new("linear", "stale"), start),
        ]);
        assert_eq!(
            tasks_to_fetch(&rows, &requested, now, WINDOW),
            vec![("linear".to_string(), vec!["stale".to_string()])]
        );
        // Right at the edge of the window it is due.
        let only_stale = HashMap::from([(TaskId::new("linear", "stale"), start)]);
        assert_eq!(
            tasks_to_fetch(&rows[1..], &only_stale, start + WINDOW, WINDOW),
            vec![("linear".to_string(), vec!["stale".to_string()])]
        );
    }
}
