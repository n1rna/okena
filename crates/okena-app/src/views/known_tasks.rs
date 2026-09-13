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
use okena_core::harness::AgentAsset;
use okena_core::tasks::{Task, TaskId};
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
            .and_then(|g| g.0.read(cx).state_of(&task.id))
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

/// Tasks named by `assets`, grouped by provider, leaving out any in `skip`.
///
/// What a session panel asks its provider about: one batch per provider, and
/// never a task whose request is already on its way.
pub fn tasks_to_fetch<'a>(
    assets: impl IntoIterator<Item = &'a AgentAsset>,
    skip: &std::collections::HashSet<TaskId>,
) -> Vec<(String, Vec<String>)> {
    let mut by_provider: Vec<(String, Vec<String>)> = Vec::new();
    for task in assets.into_iter().filter_map(|a| a.task.as_ref()) {
        if skip.contains(&task.id) {
            continue;
        }
        let ids = match by_provider.iter_mut().find(|(p, _)| *p == task.id.provider) {
            Some((_, ids)) => ids,
            None => {
                by_provider.push((task.id.provider.clone(), Vec::new()));
                &mut by_provider.last_mut().expect("just pushed").1
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
    use super::{task_caption, tasks_to_fetch};
    use okena_core::harness::{AgentAsset, AgentAssetKind};
    use okena_core::tasks::{TaskId, TaskRef};
    use std::collections::HashSet;

    #[test]
    fn a_task_row_names_its_key_and_only_a_known_state() {
        assert_eq!(
            task_caption("task", "QBL-9", Some("In Progress")),
            "task · QBL-9 · In Progress"
        );
        assert_eq!(task_caption("task", "#42", None), "task · #42");
    }

    fn task(provider: &str, id: &str) -> AgentAsset {
        AgentAsset {
            kind: AgentAssetKind::Task,
            title: id.into(),
            url: None,
            project: None,
            created_at: 0,
            task: Some(TaskRef {
                id: TaskId::new(provider, id),
                display_key: id.into(),
                title: id.into(),
                url: String::new(),
                parent_id: None,
                parent_key: None,
            }),
        }
    }

    #[test]
    fn fetches_are_one_batch_per_provider_without_repeats_or_requests_in_flight() {
        let assets = [
            task("linear", "a"),
            task("azure_devops", "7"),
            task("linear", "b"),
            task("linear", "a"),
            task("linear", "c"),
        ];
        let in_flight = HashSet::from([TaskId::new("linear", "c")]);
        assert_eq!(
            tasks_to_fetch(&assets, &in_flight),
            vec![
                ("linear".to_string(), vec!["a".to_string(), "b".to_string()]),
                ("azure_devops".to_string(), vec!["7".to_string()]),
            ]
        );
    }
}
