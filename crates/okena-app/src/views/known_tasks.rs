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

/// `TaskGetMany` as it goes over the wire, and so as a daemon that does not
/// know it names it in its refusal.
const TASK_GET_MANY: &str = "task_get_many";

/// Whether `error` is a daemon refusing `TaskGetMany` because it predates it.
///
/// Such a daemon cannot parse the request at all, so the answer is serde's
/// message about the unknown tag rather than anything the action returned.
pub fn is_batch_unknown(error: &str) -> bool {
    error.contains(&format!("unknown variant `{TASK_GET_MANY}`"))
}

/// Connections whose daemon does not know `TaskGetMany`, each with the
/// connection generation it was learned on.
///
/// App-wide, so every panel on that connection stops asking and the refusal
/// is logged once. A reconnect may bring a newer daemon, so the fact lapses
/// when the connection's generation moves on.
#[derive(Default)]
pub struct BatchUnsupported {
    by_connection: HashMap<String, u64>,
}

impl Global for BatchUnsupported {}

impl BatchUnsupported {
    /// Whether the batch is known to be refused on this connection as it is now.
    pub fn applies(&self, connection_id: &str, generation: u64) -> bool {
        self.by_connection.get(connection_id) == Some(&generation)
    }

    /// Remember a refusal. True when it is news for this connection generation.
    pub fn record(&mut self, connection_id: &str, generation: u64) -> bool {
        self.by_connection
            .insert(connection_id.to_string(), generation)
            != Some(generation)
    }
}

pub fn batch_unsupported(connection_id: &str, generation: u64, cx: &App) -> bool {
    cx.try_global::<BatchUnsupported>()
        .is_some_and(|b| b.applies(connection_id, generation))
}

pub fn record_batch_unsupported(connection_id: &str, generation: u64, cx: &mut App) -> bool {
    cx.default_global::<BatchUnsupported>()
        .record(connection_id, generation)
}

#[cfg(test)]
mod batch_tests {
    use super::{BatchUnsupported, is_batch_unknown};

    /// A daemon from before `TaskGetMany`: it knows reading one task, not many.
    #[derive(serde::Deserialize, Debug)]
    #[serde(tag = "action", rename_all = "snake_case")]
    #[allow(dead_code)]
    enum OldDaemonAction {
        TaskGet { provider: String },
    }

    #[test]
    fn an_old_daemons_refusal_of_the_batch_is_recognised() {
        let request = serde_json::to_value(okena_core::api::ActionRequest::TaskGetMany {
            provider: "linear".into(),
            task_external_ids: vec!["a".into()],
        })
        .unwrap();
        let refusal = serde_json::from_value::<OldDaemonAction>(request).unwrap_err();
        // What reaches the panel: the status line and axum's rejection text.
        let answer = format!(
            "Server returned 422 Unprocessable Entity: Failed to deserialize the JSON body into the target type: {refusal}"
        );
        assert!(is_batch_unknown(&answer), "{answer}");

        // Any other failure is still just a failure, logged every time.
        assert!(!is_batch_unknown("HTTP request failed: connection refused"));
        assert!(!is_batch_unknown(
            "unknown variant `task_get`, expected one of"
        ));
        assert!(!is_batch_unknown("Linear API error: not authorized"));
    }

    #[test]
    fn a_refused_batch_stops_on_its_connection_until_it_reconnects() {
        let mut refused = BatchUnsupported::default();
        assert!(!refused.applies("remote-1", 1));

        assert!(refused.record("remote-1", 1), "the first refusal is news");
        assert!(refused.applies("remote-1", 1));
        assert!(
            !refused.record("remote-1", 1),
            "a repeat is not logged again"
        );
        assert!(!refused.applies("local", 1), "other connections still ask");

        // A reconnect may bring a newer daemon: ask it again.
        assert!(!refused.applies("remote-1", 2));
        assert!(
            refused.record("remote-1", 2),
            "a refusal after reconnecting is news"
        );
    }
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
