//! Which agents' usage the bar shows, kept in the extension's settings as
//! `{ "agents": ["claude", "codex"] }`.

use gpui::{App, BorrowAppContext};
use okena_extensions::ExtensionSettingsStore;

/// The extension's id, and its settings namespace.
pub const EXTENSION_ID: &str = "usage";

/// A coding agent whose usage can be shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Agent {
    Claude,
    Codex,
    Copilot,
}

impl Agent {
    /// In the order the settings list and the bar show them.
    pub const ALL: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::Copilot];

    pub const fn slug(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Copilot => "copilot",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude",
            Agent::Codex => "Codex",
            Agent::Copilot => "Copilot",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Agent::Claude => "Session and weekly limits, from your Claude Code login",
            Agent::Codex => "5-hour and weekly limits, from your Codex login",
            Agent::Copilot => "Premium requests this month, from your GitHub login (gh)",
        }
    }

    pub const fn icon(self) -> &'static str {
        match self {
            Agent::Claude => "icons/agent-claude.svg",
            Agent::Codex => "icons/agent-codex.svg",
            Agent::Copilot => "icons/agent-copilot.svg",
        }
    }
}

/// Shown before anything has been chosen.
const DEFAULT: [Agent; 1] = [Agent::Claude];

fn settings(cx: &App) -> Option<serde_json::Value> {
    cx.try_global::<ExtensionSettingsStore>()
        .and_then(|store| store.get(EXTENSION_ID, cx))
}

/// The chosen agents, in the bar's order.
pub fn selected(cx: &App) -> Vec<Agent> {
    match settings(cx).as_ref().and_then(|s| s["agents"].as_array()) {
        Some(slugs) => {
            let slugs: Vec<&str> = slugs.iter().filter_map(|s| s.as_str()).collect();
            Agent::ALL
                .into_iter()
                .filter(|a| slugs.contains(&a.slug()))
                .collect()
        }
        None => DEFAULT.to_vec(),
    }
}

/// Show or hide `agent`'s usage on the bar.
pub fn set_selected(agent: Agent, on: bool, cx: &mut App) {
    let mut chosen = selected(cx);
    chosen.retain(|a| *a != agent);
    if on {
        chosen.push(agent);
    }
    let slugs: Vec<&str> = Agent::ALL
        .into_iter()
        .filter(|a| chosen.contains(a))
        .map(Agent::slug)
        .collect();
    let mut value = settings(cx)
        .filter(|v| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    value["agents"] = serde_json::json!(slugs);
    ExtensionSettingsStore::update(EXTENSION_ID, value, cx);
    // The store does not announce writes; tell whoever observes it.
    cx.update_global::<ExtensionSettingsStore, _>(|_, _| {});
}
