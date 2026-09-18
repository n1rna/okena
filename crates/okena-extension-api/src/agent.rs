//! Launching okena agent sessions from an action.
//!
//! The extension writes the brief itself. An action declared with
//! [`AgentMode::Prefill`](crate::AgentMode::Prefill) opens okena's launcher
//! with these fields filled in; with [`AgentMode::Start`](crate::AgentMode::Start)
//! the session starts at once with the default agent.

use crate::wit::exports::okena::extension::guest as g;

pub use g::ContextKind;

/// Something from okena's context — a map entry, spec, doc, skill or agent —
/// handed to the session, owned by a project or a knowledge store.
#[derive(Clone, Debug)]
pub struct ContextRef {
    pub kind: ContextKind,
    pub project_id: Option<String>,
    pub store: Option<String>,
    pub locator: String,
}

impl ContextRef {
    /// `locator` is `area:<id>` and the like for a map entry, else a path
    /// relative to the project.
    pub fn in_project(kind: ContextKind, project_id: &str, locator: &str) -> Self {
        Self {
            kind,
            project_id: Some(project_id.into()),
            store: None,
            locator: locator.into(),
        }
    }

    /// `store` is the store's root key, e.g. `store:acme-eng`.
    pub fn in_store(kind: ContextKind, store: &str, locator: &str) -> Self {
        Self {
            kind,
            project_id: None,
            store: Some(store.into()),
            locator: locator.into(),
        }
    }

    fn into_wit(self) -> g::ContextRef {
        g::ContextRef {
            kind: self.kind,
            project_id: self.project_id,
            store: self.store,
            locator: self.locator,
        }
    }
}

/// An agent session for okena to launch.
#[derive(Clone, Debug)]
pub struct AgentLaunch {
    pub goal: String,
    pub name: Option<String>,
    pub root: Option<String>,
    pub project_ids: Vec<String>,
    pub context: Vec<ContextRef>,
    pub item: Option<String>,
    pub item_label: Option<String>,
}

impl AgentLaunch {
    /// `goal` is the brief the agent starts with.
    pub fn new(goal: impl Into<String>) -> Self {
        Self {
            goal: goal.into(),
            name: None,
            root: None,
            project_ids: Vec::new(),
            context: Vec::new(),
            item: None,
            item_label: None,
        }
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// The directory the agent works in.
    pub fn root(mut self, root: impl Into<String>) -> Self {
        self.root = Some(root.into());
        self
    }

    pub fn project(mut self, project_id: impl Into<String>) -> Self {
        self.project_ids.push(project_id.into());
        self
    }

    pub fn context(mut self, context: ContextRef) -> Self {
        self.context.push(context);
        self
    }

    /// The row or item the session is about. Its agent badge follows the
    /// session, and the session panel names it.
    pub fn item(mut self, id: impl Into<String>, label: impl Into<String>) -> Self {
        self.item = Some(id.into());
        self.item_label = Some(label.into());
        self
    }

    pub(crate) fn into_wit(self) -> g::AgentLaunch {
        g::AgentLaunch {
            goal: self.goal,
            name: self.name,
            root: self.root,
            project_ids: self.project_ids,
            context: self.context.into_iter().map(ContextRef::into_wit).collect(),
            item: self.item,
            item_label: self.item_label,
        }
    }
}
