//! Launch context wire types.
//!
//! What a user can hand an agent beside its goal: an entry of a project's map,
//! a spec document or change, a knowledge doc, a skill or a subagent. The
//! daemon indexes them (`okena-context`), a launcher searches that index and
//! sends back the refs it picked, and the daemon resolves each ref again before
//! it reaches a brief — a client's path is never trusted.
//!
//! A ref names an item by what owns it and where it sits in its owner, not by
//! an absolute path: the same ref resolves on the daemon that indexed it, which
//! for a remote daemon is not the machine the client runs on.

use serde::{Deserialize, Serialize};

/// What an item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextKind {
    /// An area, concept, interface, pipeline or infrastructure entry of a
    /// valid `project-map.yaml`.
    MapEntry,
    /// An OpenSpec spec document or change folder.
    Spec,
    /// A doc under a knowledge root's `docs/`.
    Doc,
    /// A skill: `skills/**/SKILL.md`.
    Skill,
    /// A subagent under `agents/`.
    Agent,
}

impl ContextKind {
    /// Every kind, in the order a brief lists them.
    pub const fn all() -> [ContextKind; 5] {
        [
            ContextKind::MapEntry,
            ContextKind::Spec,
            ContextKind::Doc,
            ContextKind::Skill,
            ContextKind::Agent,
        ]
    }

    /// What to call it in a results row or a brief.
    pub const fn label(self) -> &'static str {
        match self {
            ContextKind::MapEntry => "Map entry",
            ContextKind::Spec => "Spec",
            ContextKind::Doc => "Knowledge doc",
            ContextKind::Skill => "Skill",
            ContextKind::Agent => "Agent",
        }
    }

    /// Delivered as something the agent loads, not a path it reads.
    pub const fn is_installable(self) -> bool {
        matches!(self, ContextKind::Skill | ContextKind::Agent)
    }
}

/// Where an item comes from.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "owner", rename_all = "snake_case")]
pub enum ContextOwner {
    /// A workspace project: its map, its own knowledge root, its spec root.
    Project { project_id: String },
    /// A registered or followed store, by root key (`store:<id>`, `path:<abs>`).
    Store { root_key: String },
}

impl ContextOwner {
    pub fn project(id: impl Into<String>) -> Self {
        ContextOwner::Project {
            project_id: id.into(),
        }
    }

    pub fn store(key: impl Into<String>) -> Self {
        ContextOwner::Store {
            root_key: key.into(),
        }
    }

    pub fn project_id(&self) -> Option<&str> {
        match self {
            ContextOwner::Project { project_id } => Some(project_id),
            ContextOwner::Store { .. } => None,
        }
    }
}

/// An item picked for a launch, as the client sends it back.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ContextRef {
    pub kind: ContextKind,
    #[serde(flatten)]
    pub owner: ContextOwner,
    /// Where it sits in its owner: `area:<id>` (and `concept:`, `exposes:`,
    /// `consumes:`, `ci:`, `infrastructure:`) for a map entry, else the path
    /// relative to the root it was found in.
    pub locator: String,
}

/// One search result, and what a resolved ref becomes in a brief.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextItem {
    #[serde(rename = "ref")]
    pub reference: ContextRef,
    pub title: String,
    /// One line. Empty when the item has none.
    #[serde(default)]
    pub description: String,
    /// The owning project's or store's name.
    pub owner_name: String,
    /// Absolute path on the daemon's machine: the entry's doc, or its
    /// `project-map.yaml`; a spec file or change folder; a `SKILL.md`.
    pub path: String,
    /// A map entry's own id, e.g. `area:okena-core`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_id: Option<String>,
    /// From a chosen project or a store one follows, so ranked first.
    #[serde(default)]
    pub chosen: bool,
}

/// A chosen project with no map yet, surfaced so the user can scan it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnmappedProject {
    pub project_id: String,
    pub name: String,
}

/// Reply to `ContextSearch`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextSearchResult {
    /// Best first.
    pub items: Vec<ContextItem>,
    /// Chosen projects that have never been scanned, in the order chosen.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unmapped: Vec<UnmappedProject>,
}

/// Reply to `ContextRead`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextDocument {
    pub path: String,
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launch_from_an_older_client_carries_no_context() {
        use crate::api::ActionRequest;
        let old = r#"{"action":"knowledge_draft","request":"write the CI notes"}"#;
        let ActionRequest::KnowledgeDraft { context, .. } =
            serde_json::from_str::<ActionRequest>(old).unwrap()
        else {
            panic!("not a knowledge draft");
        };
        assert!(context.is_empty());
        // And an empty one is not sent, so an older daemon still accepts it.
        let body = serde_json::to_string(&ActionRequest::KnowledgeDraft {
            root: None,
            request: "x".into(),
            agent_command: None,
            model: None,
            context: Vec::new(),
        })
        .unwrap();
        assert!(!body.contains("context"), "{body}");
        assert!(!body.contains("model"), "{body}");
    }

    #[test]
    fn a_ref_is_flat_on_the_wire() {
        let r = ContextRef {
            kind: ContextKind::MapEntry,
            owner: ContextOwner::project("p1"),
            locator: "area:core".into(),
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "kind": "map_entry",
                "owner": "project",
                "project_id": "p1",
                "locator": "area:core",
            })
        );
        assert_eq!(serde_json::from_value::<ContextRef>(json).unwrap(), r);

        let store = ContextRef {
            kind: ContextKind::Skill,
            owner: ContextOwner::store("store:acme"),
            locator: "skills/review/SKILL.md".into(),
        };
        let text = serde_json::to_string(&store).unwrap();
        assert_eq!(serde_json::from_str::<ContextRef>(&text).unwrap(), store);
    }
}
