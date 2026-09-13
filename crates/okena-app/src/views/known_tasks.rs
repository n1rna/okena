//! What okena last heard about tasks from the provider, shared app-wide.
//!
//! The Tasks view is what talks to the provider, but a session's panel beside
//! its terminal lists the tasks that agent filed and needs their state too. The
//! view publishes every task it loads here and panels observe it, so a task
//! moved to Done on the provider reads as done on the next refresh without the
//! panel asking the provider itself.
//!
//! Created on first use rather than at startup, so a panel that opens before
//! the Tasks view has loaded anything still has something to observe.

use gpui::*;
use okena_core::harness::AgentAsset;
use okena_core::tasks::Task;
use std::collections::HashMap;

#[derive(Default)]
pub struct KnownTasks {
    /// The provider's name for each task's state, by provider id.
    states: HashMap<String, String>,
}

impl KnownTasks {
    pub fn state_of(&self, external_id: &str) -> Option<&str> {
        self.states.get(external_id).map(String::as_str)
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
/// Merges rather than replaces: the queue, each parent's children and the
/// tasks agents filed arrive separately, and a load of one must not forget the
/// others. Observers are notified only when a state actually changed.
pub fn remember<'a>(tasks: impl IntoIterator<Item = &'a Task>, cx: &mut App) {
    let loaded: Vec<(String, String)> = tasks
        .into_iter()
        .map(|t| (t.id.external_id.clone(), state_name(t)))
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

/// The team's own name for the state, or okena's when the provider gave none.
fn state_name(task: &Task) -> String {
    if task.state_name.trim().is_empty() {
        task.state.label().to_string()
    } else {
        task.state_name.clone()
    }
}

/// The muted line under an asset's title: its kind, then where it is — a
/// task's key and current state, the project it landed in, or its link.
pub fn asset_caption(asset: &AgentAsset, cx: &App) -> String {
    let kind = asset.kind.label();
    if let Some(task) = &asset.task {
        let state = cx
            .try_global::<GlobalKnownTasks>()
            .and_then(|g| g.0.read(cx).state_of(&task.id.external_id))
            .map(str::to_string);
        return task_caption(kind, &task.display_key, state.as_deref());
    }
    match (&asset.project, &asset.url) {
        (Some(p), _) => format!("{kind} · {p}"),
        (None, Some(url)) => format!("{kind} · {url}"),
        (None, None) => kind.to_string(),
    }
}

/// A task's caption. Without a known state it says nothing about one, rather
/// than guessing from what the task was when it was filed.
fn task_caption(kind: &str, key: &str, state: Option<&str>) -> String {
    match state {
        Some(state) => format!("{kind} · {key} · {state}"),
        None => format!("{kind} · {key}"),
    }
}

#[cfg(test)]
mod tests {
    use super::task_caption;

    #[test]
    fn a_task_row_names_its_key_and_only_a_known_state() {
        assert_eq!(
            task_caption("task", "QBL-9", Some("In Progress")),
            "task · QBL-9 · In Progress"
        );
        assert_eq!(task_caption("task", "#42", None), "task · #42");
    }
}
