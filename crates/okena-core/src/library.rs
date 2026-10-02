//! Library wire types: knowledge, specs and freeform markdown as one set of
//! typed origins (QBL-440).
//!
//! Knowledge stores and OpenSpec roots are the same thing to work with — a
//! folder of markdown, usually a git repository, that you browse, edit,
//! search, commit, push and hand to agents — so they are one list. What
//! differs is what an origin's *type* promises about the files inside it:
//!
//! - **`knowledge`** follows ADR-0003's layout (`docs/`, `skills/`,
//!   `agents/`, `templates/`). Knowledge origins layer: the top one holding a
//!   template, a partial or a skill wins, in the saved order, with okena's own
//!   `okena-defaults` last. Only this type has overrides.
//! - **`spec`** is an OpenSpec root, in the CLI's own format and registry
//!   ([`crate::specs`]). Library wraps it and changes nothing on disk, so the
//!   `openspec` CLI reads what okena writes and the other way round.
//! - **`freeform`** is any folder of markdown: workflows, decisions, notes.
//!   It is listed, edited, searched and attached as launch context, and it
//!   promises nothing else — no layout, no layering, no overrides.
//!
//! An origin is named by its **key**: the type, then the key discovery gave
//! the root — `knowledge:store:eng`, `spec:path:/work/app`,
//! `freeform:path:/notes`. The type is in the key because a knowledge store
//! and an OpenSpec store may share an id, and one key has to name one folder.
//! The daemon only accepts keys it discovered itself, so a key cannot name an
//! arbitrary directory.
//!
//! Reading and writing live in the daemon (`okena-knowledge`,
//! `okena-openspec`, and the freeform walk beside them). These are only the
//! shapes that cross the wire.

use crate::diagnostic::{Diagnostic, Severity};
use crate::doc_search::KnowledgeFacet;
use crate::knowledge::{
    KnowledgeCounts, KnowledgePointer, KnowledgeRoot, KnowledgeRootKind, KnowledgeStores,
    KnowledgeTree,
};
use crate::specs::{
    SpecPointer, SpecReference, SpecRoot, SpecRootKind, SpecStores, SpecTree, path_root_key,
};
use crate::store_git::StoreGitStatus;
use serde::{Deserialize, Serialize};

/// What an origin's files are, which decides what okena does with them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginType {
    Knowledge,
    Spec,
    Freeform,
}

impl OriginType {
    /// Every type, in the order the origin list groups them: the layers
    /// first, since their order is the one that matters.
    pub const fn all() -> [OriginType; 3] {
        [OriginType::Knowledge, OriginType::Spec, OriginType::Freeform]
    }

    /// The type as keys, settings and the wire spell it.
    pub const fn slug(self) -> &'static str {
        match self {
            OriginType::Knowledge => "knowledge",
            OriginType::Spec => "spec",
            OriginType::Freeform => "freeform",
        }
    }

    pub fn from_slug(slug: &str) -> Option<OriginType> {
        OriginType::all().into_iter().find(|t| t.slug() == slug)
    }

    pub const fn label(self) -> &'static str {
        match self {
            OriginType::Knowledge => "Knowledge",
            OriginType::Spec => "Spec",
            OriginType::Freeform => "Freeform",
        }
    }

    /// One line on what the type is for, shown where an origin is added.
    pub const fn blurb(self) -> &'static str {
        match self {
            OriginType::Knowledge => {
                "Docs, skills, agents and templates in okena's layout. Knowledge origins layer, so one can override another."
            }
            OriginType::Spec => {
                "An OpenSpec root: capabilities and changes, in the format the openspec CLI reads."
            }
            OriginType::Freeform => {
                "Any folder of markdown — workflows, decisions, notes. No layout and no overrides."
            }
        }
    }

    /// Whether origins of this type layer over one another, which is what
    /// overrides and the saved order are about.
    pub const fn layers(self) -> bool {
        matches!(self, OriginType::Knowledge)
    }
}

/// The Library key of the root discovery keyed `inner`.
pub fn origin_key(origin_type: OriginType, inner: &str) -> String {
    format!("{}:{inner}", origin_type.slug())
}

/// A Library key's type and the key discovery gave the root, or `None` for a
/// string that is not a Library key.
pub fn split_key(key: &str) -> Option<(OriginType, &str)> {
    let (slug, inner) = key.split_once(':')?;
    Some((OriginType::from_slug(slug)?, inner))
}

/// A key as the Library spells it, from one that may predate typed keys.
///
/// Sessions started before QBL-440 recorded `store:eng` on a purpose whose
/// kind already said which section it belonged to; `origin_type` is that
/// kind. A key that already carries a type is left alone.
pub fn upgrade_key(origin_type: OriginType, key: &str) -> String {
    match split_key(key) {
        Some(_) => key.to_string(),
        None => origin_key(origin_type, key),
    }
}

/// Rewrite what knowledge discovery found to carry Library keys.
///
/// The daemon does this once, where it discovers, so everything after —
/// resolving the key a client sent back, a tree's `root_key`, a search hit, a
/// session's recorded root — speaks one kind of key. The saved layering order
/// is applied before this and stays in the keys discovery gave.
pub fn key_knowledge_stores(stores: &mut KnowledgeStores) {
    for root in &mut stores.roots {
        root.key = upgrade_key(OriginType::Knowledge, &root.key);
    }
    for pointer in &mut stores.pointers {
        if let Some(key) = pointer.root_key.as_mut() {
            *key = upgrade_key(OriginType::Knowledge, key);
        }
    }
}

/// Rewrite what OpenSpec discovery found to carry Library keys. See
/// [`key_knowledge_stores`].
pub fn key_spec_stores(stores: &mut SpecStores) {
    for root in &mut stores.roots {
        root.key = upgrade_key(OriginType::Spec, &root.key);
    }
    for pointer in &mut stores.pointers {
        if let Some(key) = pointer.root_key.as_mut() {
            *key = upgrade_key(OriginType::Spec, key);
        }
    }
}

/// Where an origin was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginKind {
    /// A checkout in a registry: okena's knowledge registry, or OpenSpec's
    /// machine store registry.
    Store,
    /// Found in an okena project: its knowledge folders, or its `openspec/`
    /// tree. The project's own git owns it.
    Project,
    /// A folder named in the space's settings.
    Folder,
}

impl OriginKind {
    pub const fn label(self) -> &'static str {
        match self {
            OriginKind::Store => "store",
            OriginKind::Project => "project",
            OriginKind::Folder => "folder",
        }
    }
}

impl From<KnowledgeRootKind> for OriginKind {
    fn from(kind: KnowledgeRootKind) -> Self {
        match kind {
            KnowledgeRootKind::Store => OriginKind::Store,
            KnowledgeRootKind::Project => OriginKind::Project,
        }
    }
}

impl From<SpecRootKind> for OriginKind {
    fn from(kind: SpecRootKind) -> Self {
        match kind {
            SpecRootKind::Store => OriginKind::Store,
            SpecRootKind::Project => OriginKind::Project,
            SpecRootKind::Folder => OriginKind::Folder,
        }
    }
}

/// One origin.
///
/// One shape for all three types. The fields only one type has are optional
/// rather than a per-type payload, so a client lists every origin with the
/// same code and reads the extras where it draws them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryOrigin {
    /// The origin's Library key — see the module docs.
    pub key: String,
    #[serde(rename = "type")]
    pub origin_type: OriginType,
    pub kind: OriginKind,
    /// Store name or id, project name, or folder name.
    pub name: String,
    pub path: String,
    /// The store id, when the origin is a store checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Canonical clone source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// Usable. Problems that don't stop reading are warnings in `status` on a
    /// healthy origin.
    pub healthy: bool,
    /// okena's own `okena-defaults` knowledge store: read-only, rewritten on
    /// every start, and always the last layer.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub builtin: bool,
    /// Spec only: this store is OpenSpec's machine-wide `defaultStore`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_default: bool,
    /// Sync state and changed files, when the origin is the top of a git
    /// checkout that is its own to commit — never a project's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<StoreGitStatus>,
    /// Knowledge only: how many entries of each kind it holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counts: Option<KnowledgeCounts>,
    /// Freeform only: how many documents it lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documents: Option<u32>,
    /// Spec only: the workflow schema from `openspec/config.yaml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Spec only: the stores its config declares under `references:`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<SpecReference>,
    /// okena projects that follow or point at this origin.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub used_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<Diagnostic>,
}

impl LibraryOrigin {
    /// The key discovery gave this origin, without its type: what the saved
    /// knowledge order and a session's recorded root are written in.
    pub fn inner_key(&self) -> &str {
        split_key(&self.key).map_or(self.key.as_str(), |(_, inner)| inner)
    }

    /// Whether anything in `status`, or in a reference, is worth a look.
    pub fn warned(&self) -> bool {
        self.status.iter().any(|d| d.severity == Severity::Warning)
            || self.references.iter().any(|r| !r.status.is_empty())
    }

    /// Whether files here may be changed. okena's own store may not: it is
    /// rewritten to match the build on every start.
    pub fn writable(&self) -> bool {
        self.healthy && !self.builtin
    }

    /// Whether this origin has a place in the saved layering order, and so a
    /// handle to drag it by. Knowledge origins only, and never okena's own,
    /// which is always last. An unhealthy one keeps its place: its checkout
    /// may well come back.
    pub fn takes_part_in_order(&self) -> bool {
        self.origin_type.layers() && !self.builtin
    }

    /// Whether this origin can hold an override of one of okena's defaults.
    pub fn takes_overrides(&self) -> bool {
        self.origin_type.layers() && self.writable()
    }

    /// A knowledge root as an origin. `root.key` is the key discovery gave
    /// it, or the Library key the daemon already made of it.
    pub fn from_knowledge(root: KnowledgeRoot) -> Self {
        Self {
            key: upgrade_key(OriginType::Knowledge, &root.key),
            origin_type: OriginType::Knowledge,
            kind: root.kind.into(),
            name: root.name,
            path: root.path,
            store_id: root.store_id,
            description: root.description,
            remote: root.remote,
            healthy: root.healthy,
            builtin: root.builtin,
            is_default: false,
            git: root.git,
            counts: Some(root.counts),
            documents: None,
            schema: None,
            references: Vec::new(),
            used_by: root.used_by,
            status: root.status,
        }
    }

    /// An OpenSpec root as an origin. `root.key` is the key discovery gave it,
    /// or the Library key the daemon already made of it.
    pub fn from_spec(root: SpecRoot) -> Self {
        Self {
            key: upgrade_key(OriginType::Spec, &root.key),
            origin_type: OriginType::Spec,
            kind: root.kind.into(),
            name: root.name,
            path: root.path,
            store_id: root.store_id,
            description: None,
            remote: root.remote,
            healthy: root.healthy,
            builtin: false,
            is_default: root.is_default,
            git: root.git,
            counts: None,
            documents: None,
            schema: root.schema,
            references: root.references,
            used_by: root.used_by,
            status: root.status,
        }
    }

    /// A folder of markdown as an origin. `path` is absolute.
    pub fn freeform(name: impl Into<String>, path: impl Into<String>) -> Self {
        let path = path.into();
        Self {
            key: origin_key(OriginType::Freeform, &path_root_key(&path)),
            origin_type: OriginType::Freeform,
            kind: OriginKind::Folder,
            name: name.into(),
            path,
            store_id: None,
            description: None,
            remote: None,
            healthy: true,
            builtin: false,
            is_default: false,
            git: None,
            counts: None,
            documents: None,
            schema: None,
            references: Vec::new(),
            used_by: Vec::new(),
            status: Vec::new(),
        }
    }
}

/// An okena project that follows a knowledge store (`.okena/knowledge.yaml`)
/// or points at an OpenSpec store (`store:` in `openspec/config.yaml`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryPointer {
    #[serde(rename = "type")]
    pub origin_type: OriginType,
    pub project: String,
    pub path: String,
    pub store_id: String,
    /// The origin the pointer resolves to, when the store is here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_key: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<Diagnostic>,
}

impl LibraryPointer {
    fn from_knowledge(p: KnowledgePointer) -> Self {
        Self {
            origin_type: OriginType::Knowledge,
            project: p.project,
            path: p.path,
            store_id: p.store_id,
            root_key: p.root_key.map(|k| upgrade_key(OriginType::Knowledge, &k)),
            status: p.status,
        }
    }

    fn from_spec(p: SpecPointer) -> Self {
        Self {
            origin_type: OriginType::Spec,
            project: p.project,
            path: p.path,
            store_id: p.store_id,
            root_key: p.root_key.map(|k| upgrade_key(OriginType::Spec, &k)),
            status: p.status,
        }
    }
}

/// Every origin the active space reads, of every type.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryOrigins {
    /// Knowledge origins first, in the order they layer in (`okena-defaults`
    /// last), then spec origins, then freeform ones.
    #[serde(default)]
    pub origins: Vec<LibraryOrigin>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pointers: Vec<LibraryPointer>,
    /// Problems not tied to one origin: an unreadable registry, a stale
    /// `defaultStore`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<Diagnostic>,
    /// okena's knowledge registry file, whether or not it exists yet.
    #[serde(default)]
    pub knowledge_registry_path: String,
    /// OpenSpec's store registry file, whether or not it exists yet.
    #[serde(default)]
    pub spec_registry_path: String,
    /// OpenSpec's global config file, which holds `defaultStore`.
    #[serde(default)]
    pub spec_config_path: String,
    /// OpenSpec's machine-wide `defaultStore`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_default_store: Option<String>,
}

impl LibraryOrigins {
    /// One list from what each type's discovery found.
    pub fn assemble(
        knowledge: KnowledgeStores,
        specs: SpecStores,
        freeform: Vec<LibraryOrigin>,
    ) -> Self {
        let origins = knowledge
            .roots
            .into_iter()
            .map(LibraryOrigin::from_knowledge)
            .chain(specs.roots.into_iter().map(LibraryOrigin::from_spec))
            .chain(freeform)
            .collect();
        let pointers = knowledge
            .pointers
            .into_iter()
            .map(LibraryPointer::from_knowledge)
            .chain(specs.pointers.into_iter().map(LibraryPointer::from_spec))
            .collect();
        Self {
            origins,
            pointers,
            status: knowledge.status.into_iter().chain(specs.status).collect(),
            knowledge_registry_path: knowledge.registry_path,
            spec_registry_path: specs.registry_path,
            spec_config_path: specs.config_path,
            spec_default_store: specs.default_store,
        }
    }

    pub fn origin(&self, key: &str) -> Option<&LibraryOrigin> {
        self.origins.iter().find(|o| o.key == key)
    }

    pub fn of_type(&self, origin_type: OriginType) -> impl Iterator<Item = &LibraryOrigin> {
        self.origins
            .iter()
            .filter(move |o| o.origin_type == origin_type)
    }

    /// The origin to open when nobody has picked one.
    ///
    /// A healthy store you can write in, because shared material is what the
    /// page is for and okena's own defaults are a reference, not a place to
    /// work; then anything healthy; then whatever exists, so its problems are
    /// on screen.
    pub fn default_origin(&self) -> Option<&LibraryOrigin> {
        self.origins
            .iter()
            .find(|o| o.kind == OriginKind::Store && o.writable())
            .or_else(|| self.origins.iter().find(|o| o.writable()))
            .or_else(|| self.origins.iter().find(|o| o.healthy))
            .or_else(|| self.origins.first())
    }

    /// Put the knowledge origins in the saved layering `order` — inner keys,
    /// top first — leaving every other origin where it is.
    ///
    /// What a client does right after a drag, so the list moves before the
    /// daemon has been asked again.
    pub fn arrange_layers(&mut self, order: &[String]) {
        let mut layers: Vec<LibraryOrigin> = self
            .origins
            .iter()
            .filter(|o| o.origin_type.layers())
            .cloned()
            .collect();
        crate::knowledge_order::apply(&mut layers, order);
        let mut next = layers.into_iter();
        for origin in &mut self.origins {
            if origin.origin_type.layers()
                && let Some(layer) = next.next()
            {
                *origin = layer;
            }
        }
    }
}

/// One document in a freeform origin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreeformDoc {
    /// Path relative to the origin, e.g. `workflows/release.md`.
    pub path: String,
    /// What to show: the first `#` heading, else the file name without its
    /// extension.
    pub title: String,
}

/// Every markdown file in a freeform origin.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreeformTree {
    /// The origin's Library key.
    #[serde(default)]
    pub root_key: String,
    /// Absolute path of the origin, for display.
    #[serde(default)]
    pub root: String,
    /// Sorted by path.
    #[serde(default)]
    pub documents: Vec<FreeformDoc>,
    /// Problems with the tree as a whole, e.g. the document cap was reached.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<Diagnostic>,
}

/// What one origin holds, in its type's own shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LibraryTree {
    Knowledge(KnowledgeTree),
    Spec(SpecTree),
    Freeform(FreeformTree),
}

impl LibraryTree {
    pub fn origin_type(&self) -> OriginType {
        match self {
            LibraryTree::Knowledge(_) => OriginType::Knowledge,
            LibraryTree::Spec(_) => OriginType::Spec,
            LibraryTree::Freeform(_) => OriginType::Freeform,
        }
    }

    /// The Library key of the origin this tree was read from.
    pub fn root_key(&self) -> &str {
        match self {
            LibraryTree::Knowledge(t) => &t.root_key,
            LibraryTree::Spec(t) => &t.root_key,
            LibraryTree::Freeform(t) => &t.root_key,
        }
    }

    /// Whether the tree still lists `path`: an entry or one of a skill's
    /// files, a spec document anywhere including the archive, a freeform
    /// document.
    ///
    /// Used after a refresh to drop a selection whose file has gone, so a
    /// deleted document doesn't stay on screen looking current.
    pub fn contains(&self, path: &str) -> bool {
        match self {
            LibraryTree::Knowledge(t) => t
                .entries
                .iter()
                .any(|e| e.path == path || e.files.iter().any(|f| f == path)),
            LibraryTree::Spec(t) => {
                let in_change = |c: &crate::specs::SpecChange| {
                    c.artifacts
                        .iter()
                        .chain(c.specs.iter())
                        .any(|d| d.path == path)
                };
                t.specs.iter().any(|d| d.path == path)
                    || t.changes.iter().any(in_change)
                    || t.archived.iter().any(in_change)
            }
            LibraryTree::Freeform(t) => t.documents.iter().any(|d| d.path == path),
        }
    }
}

/// A document read from an origin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryDocument {
    pub root_key: String,
    /// Relative to the origin, as sent.
    pub path: String,
    pub content: String,
    /// What `LibraryWrite` must be handed back to replace this file.
    #[serde(default)]
    pub revision: String,
}

/// One file a `LibrarySearch` found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryHit {
    pub root_key: String,
    #[serde(rename = "type")]
    pub origin_type: OriginType,
    /// Relative to the origin, as `LibraryTree` gives it.
    pub path: String,
    /// What to show: an entry's title, a capability id, a change's name and
    /// its file, a document's title.
    pub label: String,
    /// What a knowledge file is. `None` outside knowledge origins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facet: Option<KnowledgeFacet>,
}

/// What a `LibrarySearch` found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibrarySearchResult {
    /// In origin order, then as each tree lists its files.
    #[serde(default)]
    pub hits: Vec<LibraryHit>,
    /// How many files every healthy origin holds, for "N of M".
    #[serde(default)]
    pub total: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::{KnowledgeEntry, KnowledgeKind};
    use crate::specs::{SpecChange, SpecDoc};

    fn knowledge_root(key: &str, kind: KnowledgeRootKind) -> KnowledgeRoot {
        KnowledgeRoot {
            key: key.into(),
            kind,
            name: key.into(),
            path: format!("/k/{key}"),
            store_id: None,
            description: None,
            remote: None,
            healthy: true,
            builtin: false,
            git: None,
            counts: KnowledgeCounts::default(),
            used_by: Vec::new(),
            status: Vec::new(),
        }
    }

    fn spec_root(key: &str, kind: SpecRootKind) -> SpecRoot {
        SpecRoot {
            key: key.into(),
            kind,
            name: key.into(),
            path: format!("/s/{key}"),
            store_id: None,
            remote: None,
            schema: None,
            healthy: true,
            is_default: false,
            git: None,
            references: Vec::new(),
            used_by: Vec::new(),
            status: Vec::new(),
        }
    }

    fn keys(origins: &LibraryOrigins) -> Vec<&str> {
        origins.origins.iter().map(|o| o.key.as_str()).collect()
    }

    #[test]
    fn a_key_carries_its_type_so_two_stores_with_one_id_are_two_origins() {
        // A knowledge store and an OpenSpec store may both be called `eng`.
        let knowledge = origin_key(OriginType::Knowledge, "store:eng");
        let spec = origin_key(OriginType::Spec, "store:eng");
        assert_eq!(knowledge, "knowledge:store:eng");
        assert_ne!(knowledge, spec);
        assert_eq!(split_key(&spec), Some((OriginType::Spec, "store:eng")));
        // A path keeps its colons and slashes.
        assert_eq!(
            split_key("freeform:path:/notes/a:b"),
            Some((OriginType::Freeform, "path:/notes/a:b"))
        );
    }

    #[test]
    fn a_key_from_before_typed_keys_is_not_a_library_key_until_upgraded() {
        assert_eq!(split_key("store:eng"), None);
        assert_eq!(split_key("path:/repo"), None);
        assert_eq!(
            upgrade_key(OriginType::Knowledge, "store:eng"),
            "knowledge:store:eng"
        );
        // Upgrading twice changes nothing, whatever type is asked for.
        assert_eq!(
            upgrade_key(OriginType::Spec, "knowledge:store:eng"),
            "knowledge:store:eng"
        );
    }

    #[test]
    fn every_type_has_a_distinct_slug_that_reads_back() {
        let mut seen = std::collections::HashSet::new();
        for t in OriginType::all() {
            assert!(seen.insert(t.slug()));
            assert_eq!(OriginType::from_slug(t.slug()), Some(t));
            assert!(!t.label().is_empty() && !t.blurb().is_empty());
        }
        assert_eq!(OriginType::from_slug("specs"), None);
    }

    #[test]
    fn only_knowledge_origins_layer() {
        assert!(OriginType::Knowledge.layers());
        assert!(!OriginType::Spec.layers());
        assert!(!OriginType::Freeform.layers());
    }

    #[test]
    fn the_list_is_knowledge_then_specs_then_freeform_with_typed_keys() {
        let knowledge = KnowledgeStores {
            registry_path: "/cfg/knowledge/stores.yaml".into(),
            roots: vec![knowledge_root("store:eng", KnowledgeRootKind::Store)],
            pointers: vec![KnowledgePointer {
                project: "web".into(),
                path: "/web".into(),
                store_id: "eng".into(),
                root_key: Some("store:eng".into()),
                status: Vec::new(),
            }],
            status: Vec::new(),
        };
        let specs = SpecStores {
            registry_path: "/data/openspec/stores/registry.yaml".into(),
            config_path: "/cfg/openspec/config.json".into(),
            default_store: Some("eng".into()),
            roots: vec![spec_root("store:eng", SpecRootKind::Store)],
            pointers: Vec::new(),
            status: Vec::new(),
        };
        let all = LibraryOrigins::assemble(
            knowledge,
            specs,
            vec![LibraryOrigin::freeform("notes", "/notes")],
        );
        assert_eq!(
            keys(&all),
            [
                "knowledge:store:eng",
                "spec:store:eng",
                "freeform:path:/notes"
            ]
        );
        assert_eq!(all.pointers[0].root_key.as_deref(), Some("knowledge:store:eng"));
        assert_eq!(all.spec_default_store.as_deref(), Some("eng"));
        assert_eq!(all.origin("spec:store:eng").map(|o| o.inner_key()), Some("store:eng"));
        assert_eq!(all.of_type(OriginType::Freeform).count(), 1);
    }

    #[test]
    fn stores_keyed_by_the_daemon_are_not_keyed_twice_when_listed() {
        let mut knowledge = KnowledgeStores {
            roots: vec![knowledge_root("store:eng", KnowledgeRootKind::Store)],
            pointers: vec![KnowledgePointer {
                project: "web".into(),
                path: "/web".into(),
                store_id: "eng".into(),
                root_key: Some("store:eng".into()),
                status: Vec::new(),
            }],
            ..Default::default()
        };
        key_knowledge_stores(&mut knowledge);
        assert_eq!(knowledge.roots[0].key, "knowledge:store:eng");
        assert!(knowledge.root("knowledge:store:eng").is_some());
        let mut specs = SpecStores {
            roots: vec![spec_root("path:/work/app", SpecRootKind::Project)],
            ..Default::default()
        };
        key_spec_stores(&mut specs);
        let all = LibraryOrigins::assemble(knowledge, specs, Vec::new());
        assert_eq!(keys(&all), ["knowledge:store:eng", "spec:path:/work/app"]);
        assert_eq!(all.pointers[0].root_key.as_deref(), Some("knowledge:store:eng"));
    }

    #[test]
    fn only_a_writable_knowledge_origin_takes_an_override() {
        let knowledge =
            LibraryOrigin::from_knowledge(knowledge_root("store:eng", KnowledgeRootKind::Store));
        assert!(knowledge.takes_overrides());

        let defaults = LibraryOrigin::from_knowledge(KnowledgeRoot {
            builtin: true,
            ..knowledge_root("store:okena-defaults", KnowledgeRootKind::Store)
        });
        assert!(!defaults.writable() && !defaults.takes_overrides());

        assert!(!LibraryOrigin::from_spec(spec_root("store:plans", SpecRootKind::Store)).takes_overrides());
        let freeform = LibraryOrigin::freeform("notes", "/notes");
        assert!(freeform.writable() && !freeform.takes_overrides());
    }

    #[test]
    fn the_default_origin_is_a_store_you_can_write_in() {
        let mut all = LibraryOrigins {
            origins: vec![
                LibraryOrigin::from_knowledge(KnowledgeRoot {
                    builtin: true,
                    ..knowledge_root("store:okena-defaults", KnowledgeRootKind::Store)
                }),
                LibraryOrigin::from_knowledge(knowledge_root("path:/p", KnowledgeRootKind::Project)),
                LibraryOrigin::from_spec(spec_root("store:plans", SpecRootKind::Store)),
            ],
            ..Default::default()
        };
        assert_eq!(all.default_origin().map(|o| o.key.as_str()), Some("spec:store:plans"));
        // No store of your own: a project's, before okena's defaults.
        all.origins.pop();
        assert_eq!(all.default_origin().map(|o| o.key.as_str()), Some("knowledge:path:/p"));
        // Only okena's own: still something to open.
        all.origins.pop();
        assert_eq!(
            all.default_origin().map(|o| o.key.as_str()),
            Some("knowledge:store:okena-defaults")
        );
    }

    #[test]
    fn arranging_the_layers_moves_knowledge_origins_and_nothing_else() {
        let mut all = LibraryOrigins {
            origins: vec![
                LibraryOrigin::from_knowledge(knowledge_root("store:a", KnowledgeRootKind::Store)),
                LibraryOrigin::from_knowledge(knowledge_root("store:b", KnowledgeRootKind::Store)),
                LibraryOrigin::from_knowledge(KnowledgeRoot {
                    builtin: true,
                    ..knowledge_root("store:okena-defaults", KnowledgeRootKind::Store)
                }),
                LibraryOrigin::from_spec(spec_root("store:plans", SpecRootKind::Store)),
                LibraryOrigin::freeform("notes", "/notes"),
            ],
            ..Default::default()
        };
        // The order is written in inner keys, as it always was.
        all.arrange_layers(&["store:b".to_string(), "store:a".to_string()]);
        assert_eq!(
            keys(&all),
            [
                "knowledge:store:b",
                "knowledge:store:a",
                "knowledge:store:okena-defaults",
                "spec:store:plans",
                "freeform:path:/notes",
            ]
        );
    }

    #[test]
    fn a_tree_says_its_type_on_the_wire_and_finds_its_own_files() {
        let knowledge = LibraryTree::Knowledge(KnowledgeTree {
            root_key: "knowledge:store:eng".into(),
            entries: vec![KnowledgeEntry {
                kind: KnowledgeKind::Skill,
                path: "skills/release/SKILL.md".into(),
                name: "release".into(),
                title: "Release".into(),
                description: None,
                tags: Vec::new(),
                files: vec!["skills/release/run.sh".into()],
                flows: Vec::new(),
                variables: Vec::new(),
                models: Default::default(),
                status: Vec::new(),
            }],
            ..Default::default()
        });
        let json = serde_json::to_value(&knowledge).expect("encode");
        assert_eq!(json["type"], "knowledge");
        assert_eq!(json["root_key"], "knowledge:store:eng");
        assert_eq!(
            serde_json::from_value::<LibraryTree>(json).expect("decode"),
            knowledge
        );
        assert!(knowledge.contains("skills/release/SKILL.md"));
        assert!(knowledge.contains("skills/release/run.sh"), "a skill's own files count");
        assert!(!knowledge.contains("docs/gone.md"));

        let doc = |path: &str| SpecDoc {
            path: path.into(),
            name: path.into(),
        };
        let spec = LibraryTree::Spec(SpecTree {
            root_key: "spec:store:plans".into(),
            initialized: true,
            specs: vec![doc("openspec/specs/auth/spec.md")],
            archived: vec![SpecChange {
                name: "old".into(),
                path: "openspec/changes/archive/old".into(),
                artifacts: vec![doc("openspec/changes/archive/old/proposal.md")],
                specs: Vec::new(),
                archived: true,
                schema: None,
                created: None,
            }],
            ..Default::default()
        });
        assert_eq!(spec.origin_type(), OriginType::Spec);
        assert!(spec.contains("openspec/specs/auth/spec.md"));
        // An archived document stays selected while you read it.
        assert!(spec.contains("openspec/changes/archive/old/proposal.md"));
        assert!(!spec.contains("openspec/changes/new/design.md"));

        let freeform = LibraryTree::Freeform(FreeformTree {
            root_key: "freeform:path:/notes".into(),
            root: "/notes".into(),
            documents: vec![FreeformDoc {
                path: "workflows/release.md".into(),
                title: "Release".into(),
            }],
            status: Vec::new(),
        });
        assert_eq!(serde_json::to_value(&freeform).expect("encode")["type"], "freeform");
        assert!(freeform.contains("workflows/release.md"));
        assert_eq!(freeform.root_key(), "freeform:path:/notes");
    }

    #[test]
    fn an_origin_writes_only_what_its_type_has() {
        let json =
            serde_json::to_string(&LibraryOrigin::freeform("notes", "/notes")).expect("encode");
        for absent in ["counts", "schema", "references", "builtin", "is_default", "git"] {
            assert!(!json.contains(absent), "{absent} in {json}");
        }
        assert!(json.contains(r#""type":"freeform""#), "got {json}");
        let back: LibraryOrigin = serde_json::from_str(&json).expect("decode");
        assert_eq!(back.kind, OriginKind::Folder);
    }
}
