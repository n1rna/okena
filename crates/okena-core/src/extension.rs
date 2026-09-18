//! Extensions installed from git and run as WASM components in the daemon:
//! what clients see of them.
//!
//! The daemon's extension host turns an extension's manifest and the
//! declarative view it returns into these types, and they reach every client
//! through the state snapshot. Clients draw them natively; an extension never
//! hands okena a view of its own.
//!
//! Every enum a newer daemon might extend keeps an `Unknown` fallback, so an
//! older client skips what it cannot draw instead of failing the snapshot.

use crate::context::ContextRef;
use serde::{Deserialize, Serialize};

/// How text, a badge or a stat is coloured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    #[default]
    Neutral,
    Info,
    Success,
    Warning,
    Danger,
    #[serde(other)]
    Unknown,
}

// ─── The view ───────────────────────────────────────────────────────────────

/// What an extension draws: a flat list of nodes, children by index.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtView {
    pub nodes: Vec<ExtNode>,
    pub root: u32,
}

impl ExtView {
    /// The node at `index`, if the extension pointed at one that exists.
    pub fn node(&self, index: u32) -> Option<&ExtNode> {
        self.nodes.get(index as usize)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextStyle {
    #[default]
    Body,
    Heading,
    Muted,
    Code,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtBadge {
    pub label: String,
    #[serde(default)]
    pub tone: Tone,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnKind {
    #[default]
    Text,
    Number,
    Badge,
    Date,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtColumn {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub kind: ColumnKind,
    #[serde(default)]
    pub groupable: bool,
    #[serde(default)]
    pub sortable: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtCell {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<Tone>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtField {
    pub label: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<Tone>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtRow {
    /// Stable across refreshes: actions and agent sessions are keyed by it.
    pub id: String,
    pub cells: Vec<ExtCell>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detail: Vec<ExtField>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtSort {
    pub column: String,
    #[serde(default)]
    pub descending: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtTable {
    pub id: String,
    pub columns: Vec<ExtColumn>,
    pub rows: Vec<ExtRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<ExtSort>,
    #[serde(default)]
    pub row_actions: Vec<String>,
    #[serde(default)]
    pub bulk_actions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty_text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtDetail {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub fields: Vec<ExtField>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtTreeItem {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<ExtBadge>,
    /// Indices into the tree's `items`.
    #[serde(default)]
    pub children: Vec<u32>,
    #[serde(default)]
    pub expanded: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detail: Vec<ExtField>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtTree {
    pub id: String,
    pub items: Vec<ExtTreeItem>,
    /// Indices into `items`.
    pub roots: Vec<u32>,
    #[serde(default)]
    pub show_detail: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty_text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtPoint {
    pub label: String,
    pub value: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtSeries {
    pub label: String,
    pub points: Vec<ExtPoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtChart {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub series: Vec<ExtSeries>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtStat {
    pub label: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<Tone>,
}

/// One component. `Unknown` is a component from a newer interface version
/// that this client cannot draw.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExtNode {
    Text {
        text: String,
        #[serde(default)]
        style: TextStyle,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tone: Option<Tone>,
    },
    Badge {
        badge: ExtBadge,
    },
    Badges {
        badges: Vec<ExtBadge>,
    },
    Table {
        table: ExtTable,
    },
    Detail {
        detail: ExtDetail,
    },
    Tree {
        tree: ExtTree,
    },
    BarChart {
        chart: ExtChart,
    },
    LineChart {
        chart: ExtChart,
    },
    Stats {
        stats: Vec<ExtStat>,
    },
    /// View-level action buttons, by action id.
    Actions {
        actions: Vec<String>,
    },
    Loading {
        text: String,
    },
    Empty {
        text: String,
    },
    Error {
        text: String,
    },
    Stack {
        children: Vec<u32>,
    },
    Columns {
        children: Vec<u32>,
    },
    Section {
        title: String,
        children: Vec<u32>,
        #[serde(default)]
        collapsible: bool,
        #[serde(default)]
        collapsed: bool,
    },
    #[serde(other)]
    Unknown,
}

/// The status bar widget: a short label that opens the extension's view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtStatus {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<Tone>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<String>,
}

// ─── Actions and queries ────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    #[default]
    Text,
    Multiline,
    Number,
    Toggle,
    Select,
    #[serde(other)]
    Unknown,
}

/// A field of an action's form, or a query's parameter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtInput {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub kind: InputKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

/// How an agent action launches its session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    /// okena's agent launcher opens filled in; the user starts it.
    Prefill,
    /// The session starts at once with the default agent.
    Start,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtActionDef {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub destructive: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<ExtInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentMode>,
    #[serde(default)]
    pub agent_callable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtQueryDef {
    pub id: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<ExtInput>,
}

/// Who asked for an action.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Invoker {
    #[default]
    User,
    Agent,
}

/// An agent session an action asks for, built by the extension.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtAgentLaunch {
    pub goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default)]
    pub project_ids: Vec<String>,
    #[serde(default)]
    pub context: Vec<ContextRef>,
    /// The row or item the session is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_label: Option<String>,
}

/// What running an action returned. `agent` with `mode: Prefill` is for the
/// client to open its launcher with; with `Start` the daemon already started
/// the session and `session_project_id` names it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtActionOutcome {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<Tone>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<ExtAgentLaunch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_mode: Option<AgentMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_project_id: Option<String>,
    #[serde(default)]
    pub refresh: bool,
}

// ─── Manifest-level facts ───────────────────────────────────────────────────

/// What an extension may do, as declared in its manifest and approved by the
/// user at install.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtPermissions {
    /// Program names it may run.
    #[serde(default)]
    pub commands: Vec<String>,
    /// Paths it may read, and everything under them. `~` is the home
    /// directory and `{config.<key>}` a path from its configuration.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Starting agent sessions without the user pressing Start.
    #[serde(default)]
    pub start_agents: bool,
}

impl ExtPermissions {
    /// What `self` asks for that `approved` did not grant.
    pub fn added_since(&self, approved: &ExtPermissions) -> ExtPermissions {
        ExtPermissions {
            commands: self
                .commands
                .iter()
                .filter(|c| !approved.commands.contains(c))
                .cloned()
                .collect(),
            paths: self
                .paths
                .iter()
                .filter(|p| !approved.paths.contains(p))
                .cloned()
                .collect(),
            start_agents: self.start_agents && !approved.start_agents,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty() && self.paths.is_empty() && !self.start_agents
    }
}

/// A tool the extension needs installed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtRequiredTool {
    pub name: String,
    /// The version check, e.g. `["aws", "--version"]`.
    pub check: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_version: Option<String>,
    #[serde(default)]
    pub install_hint: String,
}

/// The outcome of checking one required tool.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtToolStatus {
    pub name: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Why it failed: not found, too old, or the check failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    #[serde(default)]
    pub install_hint: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFieldKind {
    #[default]
    String,
    Path,
    Number,
    Bool,
    Select,
    #[serde(other)]
    Unknown,
}

/// One field of an extension's configuration form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtConfigField {
    pub key: String,
    pub label: String,
    /// `type` in `extension.toml`.
    #[serde(default, alias = "type")]
    pub kind: ConfigFieldKind,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

/// Where an installed extension came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExtSource {
    /// Cloned from `url` at `git_ref` (a branch, tag or commit; the remote's
    /// default branch when absent), from the folder `path` in the repo.
    Git {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        git_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        /// The commit it was installed from.
        #[serde(default)]
        commit: String,
    },
    /// A folder on the daemon's machine, for developing an extension.
    /// Reloading rebuilds and reloads it in place.
    Local { path: String },
}

/// A newer commit of the ref an extension was installed from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtUpdate {
    pub commit: String,
    #[serde(default)]
    pub version: String,
    /// What the new version asks for beyond what was approved. Updating
    /// needs it approved first.
    #[serde(default)]
    pub added_permissions: ExtPermissions,
}

/// Where an extension's lifecycle stands.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExtRunState {
    /// Installed but switched off.
    #[default]
    Disabled,
    /// Loading or checking its tools.
    Starting,
    /// A required tool is missing or too old; nothing runs until a re-check passes.
    MissingTools,
    /// A required configuration field is empty.
    NeedsConfig { missing: Vec<String> },
    Ready,
    /// It failed to load, or trapped.
    Failed { message: String },
    #[serde(other)]
    Unknown,
}

/// A host call the extension made and the host refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtRefusal {
    pub at_ms: u64,
    pub message: String,
}

/// An action waiting for the user to confirm it, because it is destructive
/// and an agent asked for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtPendingConfirmation {
    pub id: String,
    pub action: String,
    pub action_label: String,
    #[serde(default)]
    pub items: Vec<String>,
    /// The agent session that asked, by project id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_project_id: Option<String>,
    pub requested_at_ms: u64,
}

/// An installed extension, as every client sees it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiExtension {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub enabled: bool,
    pub source: ExtSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<ExtUpdate>,
    /// What the user approved.
    #[serde(default)]
    pub permissions: ExtPermissions,
    #[serde(default)]
    pub requires: Vec<ExtRequiredTool>,
    /// The last dependency check, one entry per required tool.
    #[serde(default)]
    pub tools: Vec<ExtToolStatus>,
    #[serde(default)]
    pub config_schema: Vec<ExtConfigField>,
    /// The view's title in the navigation; `None` when it has no view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_title: Option<String>,
    #[serde(default)]
    pub refresh_interval_secs: u64,
    #[serde(default)]
    pub state: ExtRunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<ExtView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ExtStatus>,
    #[serde(default)]
    pub actions: Vec<ExtActionDef>,
    #[serde(default)]
    pub queries: Vec<ExtQueryDef>,
    #[serde(default)]
    pub refreshing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refreshed_at_ms: Option<u64>,
    /// The last refresh's error; the previous view stays up beside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_error: Option<String>,
    /// The latest refusals, newest last.
    #[serde(default)]
    pub refusals: Vec<ExtRefusal>,
    #[serde(default)]
    pub pending_confirmations: Vec<ExtPendingConfirmation>,
}

impl ApiExtension {
    pub fn action(&self, id: &str) -> Option<&ExtActionDef> {
        self.actions.iter().find(|a| a.id == id)
    }
}

/// What a client shows before an install or an update: the manifest's
/// facts, for the user to approve.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtInstallPreview {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub source: ExtSource,
    pub permissions: ExtPermissions,
    /// For an update: what it asks for beyond what was approved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_permissions: Option<ExtPermissions>,
    pub requires: Vec<ExtRequiredTool>,
    /// A prebuilt `extension.wasm` sits next to the manifest; otherwise it
    /// is built from source.
    pub prebuilt: bool,
    /// Why building from source would fail here (no cargo, no wasm target).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_problem: Option<String>,
    /// Already installed; approving replaces it.
    #[serde(default)]
    pub installed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_from_a_newer_interface_is_unknown_not_an_error() {
        let view: ExtView = serde_json::from_str(
            r#"{"nodes":[{"kind":"hologram","depth":3},{"kind":"text","text":"hi"}],"root":1}"#,
        )
        .expect("parses");
        assert_eq!(view.nodes[0], ExtNode::Unknown);
        assert!(matches!(&view.nodes[1], ExtNode::Text { text, .. } if text == "hi"));
    }

    #[test]
    fn added_permissions_are_only_what_was_not_approved() {
        let approved = ExtPermissions {
            commands: vec!["jq".into()],
            paths: vec!["~/data".into()],
            start_agents: false,
        };
        let wanted = ExtPermissions {
            commands: vec!["jq".into(), "aws".into()],
            paths: vec!["~/data".into()],
            start_agents: true,
        };
        let added = wanted.added_since(&approved);
        assert_eq!(added.commands, vec!["aws".to_string()]);
        assert!(added.paths.is_empty());
        assert!(added.start_agents);
        assert!(approved.added_since(&approved).is_empty());
    }
}
