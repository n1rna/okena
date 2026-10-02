//! Engineering-harness Library actions (QBL-440).
//!
//! One set of actions over three types of origin. This module is the part
//! that is the same for all of them: find out which type an action is about —
//! from the key it names, or the type it says it is adding — and hand it to
//! that type's own code:
//!
//! - `knowledge.rs` — knowledge stores and projects' knowledge folders, over
//!   `okena-knowledge` (ADR-0003). The only type that layers.
//! - `specs.rs` — OpenSpec roots, over `okena-openspec`, in the CLI's own
//!   format and registry.
//! - `freeform.rs` — any folder of markdown.
//!
//! The listing and the search are the two actions that are about every type
//! at once, and are put together here.
//!
//! None of these touches the workspace, and most may run git, so the daemon
//! runs them off its lock with what discovery needs copied out by
//! [`library_sources`]. The two that start an agent session — drafting and
//! refining — do change the workspace and are dispatched from `mod.rs`.

use super::{ActionResult, freeform, knowledge, specs};
use crate::workspace::persistence::AppSettings;
use crate::workspace::state::ProjectData;
use okena_core::api::ActionRequest;
use okena_core::doc_search::LibrarySearchFilter;
use okena_core::library::{
    LibraryOrigin, LibraryOrigins, LibrarySearchResult, OriginType, split_key,
};
use okena_git::repository as git;
use std::path::{Path, PathBuf};

/// What discovery looks at, for every origin type, copied out of the
/// workspace so the daemon can discover — and run the git a listing runs in
/// every checkout — without holding the workspace lock.
#[derive(Clone, Debug)]
pub struct LibrarySources {
    /// The projects knowledge is looked for in.
    pub(super) knowledge: Vec<okena_knowledge::discover::ProjectSource>,
    /// OpenSpec's registry switch, projects and folders.
    pub(super) spec: okena_openspec::discover::Sources,
    /// The freeform folders the space lists.
    pub(super) freeform: Vec<String>,
}

/// Everything Library discovery needs, for the active space.
pub fn library_sources(projects: &[ProjectData], settings: &AppSettings) -> LibrarySources {
    LibrarySources {
        knowledge: knowledge::knowledge_project_sources(projects, settings),
        spec: specs::spec_sources(projects, settings),
        freeform: settings.active_space().freeform_folders(),
    }
}

/// A change to the space's `library` setting that an action needs made.
///
/// Folder origins are not in a registry — a freeform origin, and an OpenSpec
/// folder, is a line in the space's settings — so adding or removing one is a
/// settings write. The action itself runs off the daemon's lock with a
/// settings snapshot and cannot write; it says what to write and the daemon
/// writes it, through the one path that persists settings.
///
/// The space is named rather than "the active one": a clone can take a
/// minute, and the origin belongs to the space it was added from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsEdit {
    AddFreeformFolder { space: String, folder: String },
    RemoveFreeformFolder { space: String, folder: String },
    RemoveSpecFolder { space: String, folder: String },
}

impl SettingsEdit {
    /// Make the change. Fails when the space has gone since the action began.
    pub fn apply(&self, settings: &mut AppSettings) -> Result<(), String> {
        let (space, folder) = match self {
            SettingsEdit::AddFreeformFolder { space, folder }
            | SettingsEdit::RemoveFreeformFolder { space, folder }
            | SettingsEdit::RemoveSpecFolder { space, folder } => (space, folder),
        };
        // Default is always a space, even in settings nobody has listed any
        // in yet.
        settings.ensure_spaces();
        let library = &mut settings
            .space_mut(space)
            .ok_or_else(|| format!("the space `{space}` no longer exists"))?
            .library;
        match self {
            SettingsEdit::AddFreeformFolder { .. } => {
                library.freeform.add_folder(folder);
            }
            SettingsEdit::RemoveFreeformFolder { .. } => {
                // Listed as typed (`~/notes`), named here by its real path.
                let wanted = okena_core::fs::canonical(Path::new(folder));
                library.freeform.folders.retain(|f| {
                    okena_core::fs::canonical(&okena_core::fs::expand_home(f.trim())) != wanted
                });
            }
            SettingsEdit::RemoveSpecFolder { .. } => {
                let wanted = okena_core::fs::canonical(Path::new(folder));
                library.spec.folders.retain(|f| {
                    okena_core::fs::canonical(&okena_core::fs::expand_home(f.trim())) != wanted
                });
            }
        }
        Ok(())
    }
}

/// What a Library action came to: its reply, and the settings change that
/// has to be saved for it to hold.
pub struct LibraryOutcome {
    pub result: ActionResult,
    pub edit: Option<SettingsEdit>,
}

impl From<ActionResult> for LibraryOutcome {
    fn from(result: ActionResult) -> Self {
        Self { result, edit: None }
    }
}

impl From<(ActionResult, Option<SettingsEdit>)> for LibraryOutcome {
    fn from((result, edit): (ActionResult, Option<SettingsEdit>)) -> Self {
        Self { result, edit }
    }
}

/// Whether `action` is a Library action [`execute_library_action`] runs —
/// every one but the two that start an agent session.
pub fn is_library_action(action: &ActionRequest) -> bool {
    matches!(
        action,
        ActionRequest::LibraryOrigins
            | ActionRequest::LibraryTree { .. }
            | ActionRequest::LibraryRead { .. }
            | ActionRequest::LibrarySearch { .. }
            | ActionRequest::LibraryWrite { .. }
            | ActionRequest::LibraryFileCreate { .. }
            | ActionRequest::LibraryFolderCreate { .. }
            | ActionRequest::LibraryFileRename { .. }
            | ActionRequest::LibraryFileDelete { .. }
            | ActionRequest::LibraryOverrides { .. }
            | ActionRequest::LibraryLayering
            | ActionRequest::LibraryOverride { .. }
            | ActionRequest::LibraryStoreClone { .. }
            | ActionRequest::LibraryStoreRegister { .. }
            | ActionRequest::LibraryStoreUnregister { .. }
            | ActionRequest::LibraryStoreSetup { .. }
            | ActionRequest::LibrarySetDefaultStore { .. }
            | ActionRequest::LibraryStoreFetch { .. }
            | ActionRequest::LibraryStorePull { .. }
            | ActionRequest::LibraryStoreCommit { .. }
            | ActionRequest::LibraryStorePush { .. }
    )
}

/// Run a Library action; `None` for any other action.
pub fn execute_library_action(
    action: &ActionRequest,
    sources: &LibrarySources,
    settings: &AppSettings,
) -> Option<LibraryOutcome> {
    let registry = knowledge::registry();
    execute_at(&registry, action, sources, settings)
}

/// [`execute_library_action`] against the knowledge registry at `registry`,
/// so a test never reads the developer's own.
pub(super) fn execute_at(
    registry: &Path,
    action: &ActionRequest,
    sources: &LibrarySources,
    settings: &AppSettings,
) -> Option<LibraryOutcome> {
    if !is_library_action(action) {
        return None;
    }
    let backends = Backends {
        registry,
        sources,
        settings,
    };
    Some(match action {
        ActionRequest::LibraryOrigins => match serde_json::to_value(backends.listing()) {
            Ok(v) => ActionResult::Ok(Some(v)).into(),
            Err(e) => ActionResult::Err(format!("could not serialize the origins: {e}")).into(),
        },
        ActionRequest::LibrarySearch {
            query,
            roots,
            types,
            kinds,
        } => backends.search(&LibrarySearchFilter {
            query: query.clone(),
            roots: roots.clone(),
            types: types.clone(),
            kinds: kinds.clone(),
        }),
        // Layering is a knowledge matter: the question is asked of the
        // knowledge origins whatever is open.
        ActionRequest::LibraryOverrides { .. } | ActionRequest::LibraryLayering => {
            backends.run(OriginType::Knowledge, action)
        }
        // `defaultStore` is OpenSpec's.
        ActionRequest::LibrarySetDefaultStore { .. } => backends.run(OriginType::Spec, action),
        // Adding an origin says which type it is adding.
        ActionRequest::LibraryStoreClone { origin_type, .. }
        | ActionRequest::LibraryStoreRegister { origin_type, .. }
        | ActionRequest::LibraryStoreSetup { origin_type, .. } => {
            backends.run(*origin_type, action)
        }
        // Everything else is about one origin, whose key says its type.
        _ => match backends.rooted(action) {
            Ok((origin_type, action)) => backends.run(origin_type, &action),
            Err(e) => ActionResult::Err(e).into(),
        },
    })
}

/// The three types' code, and what each needs to run.
struct Backends<'a> {
    registry: &'a Path,
    sources: &'a LibrarySources,
    settings: &'a AppSettings,
}

impl Backends<'_> {
    fn knowledge_sources(&self) -> okena_knowledge::discover::Sources {
        knowledge::knowledge_sources(self.registry, &self.sources.knowledge, self.settings)
    }

    /// Every origin, with what a listing adds: sync state, counts.
    fn listing(&self) -> LibraryOrigins {
        LibraryOrigins::assemble(
            knowledge::stores(&self.knowledge_sources()),
            specs::listing(&self.sources.spec, self.settings),
            freeform::listing(&self.sources.freeform),
        )
    }

    /// Every origin as discovery finds it, without the git and the counting a
    /// listing does: enough to pick the one to open.
    fn discovered(&self) -> LibraryOrigins {
        LibraryOrigins::assemble(
            knowledge::discovered(&self.knowledge_sources()),
            specs::discovered(&self.sources.spec, self.settings),
            freeform::discover(&self.sources.freeform),
        )
    }

    /// The type of the origin `action` names, and the action naming it by key.
    ///
    /// An action that may leave the origin out (`root: None`) is given the
    /// default one, so each type's code is only ever asked about a key.
    fn rooted(&self, action: &ActionRequest) -> Result<(OriginType, ActionRequest), String> {
        let named = match action {
            ActionRequest::LibraryTree { root }
            | ActionRequest::LibraryRead { root, .. }
            | ActionRequest::LibraryWrite { root, .. }
            | ActionRequest::LibraryFileCreate { root, .. }
            | ActionRequest::LibraryFolderCreate { root, .. }
            | ActionRequest::LibraryFileRename { root, .. }
            | ActionRequest::LibraryFileDelete { root, .. } => root.clone(),
            ActionRequest::LibraryOverride { root, .. }
            | ActionRequest::LibraryStoreUnregister { root }
            | ActionRequest::LibraryStoreFetch { root }
            | ActionRequest::LibraryStorePull { root }
            | ActionRequest::LibraryStoreCommit { root, .. }
            | ActionRequest::LibraryStorePush { root } => Some(root.clone()),
            _ => return Err("not an action on one origin".into()),
        };
        let key = match named.map(|k| k.trim().to_string()).filter(|k| !k.is_empty()) {
            Some(key) => key,
            None => self
                .discovered()
                .default_origin()
                .map(|o| o.key.clone())
                .ok_or_else(no_origins)?,
        };
        let (origin_type, _) = split_key(&key).ok_or_else(|| unknown_origin(&key))?;
        let mut action = action.clone();
        match &mut action {
            ActionRequest::LibraryTree { root }
            | ActionRequest::LibraryRead { root, .. }
            | ActionRequest::LibraryWrite { root, .. }
            | ActionRequest::LibraryFileCreate { root, .. }
            | ActionRequest::LibraryFolderCreate { root, .. }
            | ActionRequest::LibraryFileRename { root, .. }
            | ActionRequest::LibraryFileDelete { root, .. } => *root = Some(key),
            _ => {}
        }
        Ok((origin_type, action))
    }

    /// Hand `action` to the code for `origin_type`.
    fn run(&self, origin_type: OriginType, action: &ActionRequest) -> LibraryOutcome {
        let outcome: Option<LibraryOutcome> = match origin_type {
            OriginType::Knowledge => knowledge::execute_at(
                self.registry,
                action,
                &self.sources.knowledge,
                self.settings,
            )
            .map(Into::into),
            OriginType::Spec => {
                specs::execute(action, &self.sources.spec, self.settings).map(Into::into)
            }
            OriginType::Freeform => {
                freeform::execute(action, &self.sources.freeform, self.settings).map(Into::into)
            }
        };
        outcome.unwrap_or_else(|| {
            ActionResult::Err(format!(
                "{} origins have nothing to do that with",
                origin_type.label()
            ))
            .into()
        })
    }

    /// Search every type's origins the filter lets through, in list order.
    fn search(&self, filter: &LibrarySearchFilter) -> LibraryOutcome {
        let wanted = |t: OriginType| filter.types.is_empty() || filter.types.contains(&t);
        let mut result = LibrarySearchResult::default();
        if wanted(OriginType::Knowledge) {
            knowledge::search(&self.knowledge_sources(), filter, &mut result);
        }
        if wanted(OriginType::Spec) {
            specs::search(&self.sources.spec, self.settings, filter, &mut result);
        }
        if wanted(OriginType::Freeform) {
            freeform::search(&self.sources.freeform, filter, &mut result);
        }
        match serde_json::to_value(result) {
            Ok(v) => ActionResult::Ok(Some(v)).into(),
            Err(e) => ActionResult::Err(format!("could not serialize the search: {e}")).into(),
        }
    }
}

fn no_origins() -> String {
    "no Library origins yet — add one from the Library's Origins page, or in Settings → Library"
        .to_string()
}

pub(super) fn unknown_origin(key: &str) -> String {
    format!("unknown origin `{key}` — it is no longer listed; refresh the Library")
}

/// The origin a session is about to work in: the one `key` names, or the
/// default one, as discovery finds it for the active space.
///
/// For the two actions that start an agent, which run on the workspace path
/// and so discover from the workspace's own project list.
pub(super) fn resolve_origin(
    projects: &[ProjectData],
    settings: &AppSettings,
    key: Option<&str>,
) -> Result<LibraryOrigin, String> {
    let registry = knowledge::registry();
    let sources = library_sources(projects, settings);
    let all = Backends {
        registry: &registry,
        sources: &sources,
        settings,
    }
    .discovered();
    match key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(key) => all.origin(key).cloned().ok_or_else(|| unknown_origin(key)),
        None => all.default_origin().cloned().ok_or_else(no_origins),
    }
}

/// Clone `url` into `dest`, or into `clone_dir` named the way `git clone`
/// would. Returns the checkout.
///
/// Shared by the origin types that have no clone of their own in their crate:
/// `okena-openspec` deliberately carries no git beyond `init` and the first
/// commit, and a freeform origin has no crate at all. The clone is
/// `okena_git`'s — including its URL validation, which must not be
/// reimplemented where it could drift.
///
/// `what` names the thing being added, for the messages ("store", "origin").
pub(super) fn clone_checkout(
    url: &str,
    dest: Option<&str>,
    clone_dir: &Path,
    what: &str,
) -> Result<PathBuf, String> {
    let Ok(url) = git::validate_clone_url(url) else {
        return Err(
            "Enter a repository URL to clone, for example git@github.com:acme/team-plans.git."
                .into(),
        );
    };
    let target = match dest.map(str::trim).filter(|d| !d.is_empty()) {
        Some(dest) => okena_core::fs::expand_home(dest),
        None => match git::clone_dir_name(url) {
            Some(name) => clone_dir.join(name),
            None => {
                return Err(format!(
                    "No folder name can be derived from {url} — choose a destination folder."
                ));
            }
        },
    };

    let existed = target.exists();
    if let Err(e) = git::clone_repository(url, &target) {
        // Only clean up what this call created; a folder that was already
        // there is the user's.
        if !existed {
            let _ = std::fs::remove_dir_all(&target);
        }
        return Err(match e {
            okena_git::GitError::CloneTargetExists { .. } => format!(
                "{} already exists and is not empty — choose another destination, or add that folder as an existing {what}.",
                target.display()
            ),
            e => format!(
                "Could not clone {url}: {} — check the URL, and that git can reach it from a terminal.",
                e.user_detail()
            ),
        });
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_core::library::{LibraryDocument, LibraryTree};

    struct World {
        sandbox: PathBuf,
        registry: PathBuf,
        settings: AppSettings,
        knowledge: PathBuf,
        spec: PathBuf,
        freeform: PathBuf,
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// One origin of each type, all three named `eng`: the knowledge store by
    /// its id, the spec folder and the freeform folder by their directory.
    fn world(tag: &str) -> World {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let sandbox = std::env::temp_dir()
            .join(format!(
                "okena-library-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(&sandbox).unwrap();
        let sandbox = sandbox.canonicalize().unwrap();

        let knowledge = sandbox.join("knowledge/eng");
        write(
            &knowledge.join(".okena-knowledge/store.yaml"),
            "version: 1\nid: eng\nname: Engineering\n",
        );
        write(
            &knowledge.join("docs/release.md"),
            "---\ntitle: Release process\n---\n\nTag, then publish.",
        );
        write(
            &knowledge.join("templates/briefs/task-start.md"),
            "---\nfor: [task-start]\n---\n\nOur own brief for {task}.",
        );
        let registry = sandbox.join("cfg/knowledge/stores.yaml");
        okena_knowledge::registry::register(&registry, &knowledge.to_string_lossy(), None)
            .expect("register the knowledge store");

        let spec = sandbox.join("spec/eng");
        write(&spec.join("openspec/config.yaml"), "schema: spec-driven\n");
        write(&spec.join("openspec/specs/release/spec.md"), "# Release\n\nShip weekly.");
        write(
            &spec.join("openspec/changes/add-login/proposal.md"),
            "# Add login\n\nTag the build.",
        );

        let freeform = sandbox.join("freeform/eng");
        write(&freeform.join("workflows/release.md"), "# Release checklist\n\nTag it.");
        write(&freeform.join("adr/0001.md"), "# Two processes");

        let mut settings = AppSettings::default();
        let library = &mut settings.active_space_mut().library;
        // OpenSpec's machine directories in the sandbox, and its registry off,
        // so no test reads or writes the developer's real one.
        library.spec.data_dir = Some(sandbox.join("data").to_string_lossy().into());
        library.spec.config_dir = Some(sandbox.join("config").to_string_lossy().into());
        library.spec.registry = false;
        library.spec.folders = vec![spec.to_string_lossy().into()];
        library.freeform.folders = vec![freeform.to_string_lossy().into()];
        World {
            sandbox,
            registry,
            settings,
            knowledge,
            spec,
            freeform,
        }
    }

    impl World {
        fn outcome(&self, action: ActionRequest) -> LibraryOutcome {
            let sources = library_sources(&[], &self.settings);
            execute_at(&self.registry, &action, &sources, &self.settings)
                .expect("a library action")
        }

        fn ok(&self, action: ActionRequest) -> serde_json::Value {
            match self.outcome(action).result {
                ActionResult::Ok(Some(v)) => v,
                ActionResult::Ok(None) => serde_json::Value::Null,
                ActionResult::Err(e) => panic!("expected success, got: {e}"),
            }
        }

        fn err(&self, action: ActionRequest) -> String {
            match self.outcome(action).result {
                ActionResult::Err(e) => e,
                ActionResult::Ok(v) => panic!("expected a refusal, got: {v:?}"),
            }
        }

        fn origins(&self) -> LibraryOrigins {
            serde_json::from_value(self.ok(ActionRequest::LibraryOrigins)).unwrap()
        }

        fn knowledge_key(&self) -> String {
            "knowledge:store:eng".to_string()
        }

        fn spec_key(&self) -> String {
            format!("spec:path:{}", self.spec.display())
        }

        fn freeform_key(&self) -> String {
            format!("freeform:path:{}", self.freeform.display())
        }
    }

    impl Drop for World {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.sandbox).ok();
        }
    }

    #[test]
    fn one_listing_holds_every_type_under_keys_that_say_which() {
        let w = world("listing");
        let all = w.origins();
        let listed: Vec<(&str, OriginType)> = all
            .origins
            .iter()
            .map(|o| (o.key.as_str(), o.origin_type))
            .collect();
        assert_eq!(
            listed,
            [
                (w.knowledge_key().as_str(), OriginType::Knowledge),
                (w.spec_key().as_str(), OriginType::Spec),
                (w.freeform_key().as_str(), OriginType::Freeform),
            ],
            "knowledge first, then specs, then freeform"
        );
        // Each carries what its type has and nothing another's.
        let knowledge = all.origin(&w.knowledge_key()).unwrap();
        assert_eq!(knowledge.counts.map(|c| c.total()), Some(2));
        assert!(knowledge.takes_overrides());
        let spec = all.origin(&w.spec_key()).unwrap();
        assert_eq!(spec.schema.as_deref(), Some("spec-driven"));
        assert!(spec.counts.is_none() && !spec.takes_overrides());
        let freeform = all.origin(&w.freeform_key()).unwrap();
        assert_eq!(freeform.documents, Some(2));
        assert!(!freeform.takes_overrides());
        assert!(all.knowledge_registry_path.ends_with("stores.yaml"));
        assert!(all.spec_registry_path.contains("registry.yaml"));
    }

    #[test]
    fn a_tree_comes_back_in_its_own_types_shape() {
        let w = world("tree");
        let tree = |key: String| -> LibraryTree {
            serde_json::from_value(w.ok(ActionRequest::LibraryTree { root: Some(key) })).unwrap()
        };
        let LibraryTree::Knowledge(k) = tree(w.knowledge_key()) else {
            panic!("expected a knowledge tree");
        };
        assert_eq!(k.root_key, w.knowledge_key());
        assert_eq!(k.entries.len(), 2);

        let LibraryTree::Spec(s) = tree(w.spec_key()) else {
            panic!("expected a spec tree");
        };
        assert_eq!(s.root_key, w.spec_key());
        assert_eq!(s.specs[0].name, "release");
        assert_eq!(s.changes[0].name, "add-login");

        let LibraryTree::Freeform(f) = tree(w.freeform_key()) else {
            panic!("expected a freeform tree");
        };
        assert_eq!(f.root_key, w.freeform_key());
        assert_eq!(f.documents.len(), 2);
    }

    #[test]
    fn the_same_read_and_write_work_on_every_type() {
        let w = world("crud");
        for (key, path) in [
            (w.knowledge_key(), "docs/release.md"),
            (w.spec_key(), "openspec/specs/release/spec.md"),
            (w.freeform_key(), "workflows/release.md"),
        ] {
            let doc: LibraryDocument = serde_json::from_value(w.ok(ActionRequest::LibraryRead {
                root: Some(key.clone()),
                path: path.into(),
            }))
            .unwrap();
            assert_eq!(doc.root_key, key, "{key}");
            assert!(!doc.content.is_empty() && !doc.revision.is_empty(), "{key}");

            let saved = w.ok(ActionRequest::LibraryWrite {
                root: Some(key.clone()),
                path: path.into(),
                content: format!("{}\n\nEdited in the Library.", doc.content),
                revision: doc.revision,
            });
            assert_eq!(saved["root"], key);

            let made = w.ok(ActionRequest::LibraryFileCreate {
                root: Some(key.clone()),
                path: "added/note.md".into(),
                content: "# Note".into(),
            });
            assert_eq!(made["root"], key);
            w.ok(ActionRequest::LibraryFileDelete {
                root: Some(key.clone()),
                path: "added/note.md".into(),
            });
        }
        for file in [
            w.knowledge.join("docs/release.md"),
            w.spec.join("openspec/specs/release/spec.md"),
            w.freeform.join("workflows/release.md"),
        ] {
            assert!(
                std::fs::read_to_string(&file).unwrap().ends_with("Edited in the Library."),
                "{}",
                file.display()
            );
        }
    }

    #[test]
    fn a_key_no_discovery_produced_names_nothing() {
        let w = world("keys");
        // The key shape from before typed keys is not a Library key.
        let e = w.err(ActionRequest::LibraryTree {
            root: Some("store:eng".into()),
        });
        assert!(e.contains("unknown origin"), "{e}");
        // A typed key for a folder nobody listed is refused by its type.
        let e = w.err(ActionRequest::LibraryRead {
            root: Some(format!("freeform:path:{}", w.sandbox.display())),
            path: "cfg/knowledge/stores.yaml".into(),
        });
        assert!(e.contains("unknown origin"), "{e}");
        // The knowledge store's id under the spec type is another origin, and
        // there is none.
        let e = w.err(ActionRequest::LibraryTree {
            root: Some("spec:store:eng".into()),
        });
        assert!(e.contains("unknown"), "{e}");
    }

    #[test]
    fn no_origin_named_opens_the_default_one() {
        let w = world("default");
        let tree: LibraryTree =
            serde_json::from_value(w.ok(ActionRequest::LibraryTree { root: None })).unwrap();
        // The knowledge store: a store you can write in comes first.
        assert_eq!(tree.root_key(), w.knowledge_key());
    }

    #[test]
    fn one_search_covers_every_type_and_narrows_by_type_and_origin() {
        let w = world("search");
        let search = |query: &str, roots: Vec<String>, types: Vec<OriginType>| {
            let result: LibrarySearchResult =
                serde_json::from_value(w.ok(ActionRequest::LibrarySearch {
                    query: query.into(),
                    roots,
                    types,
                    kinds: Vec::new(),
                }))
                .unwrap();
            result
        };
        // "tag" is only in the content of one file per origin.
        let all = search("tag", Vec::new(), Vec::new());
        let hit_types: Vec<OriginType> = all.hits.iter().map(|h| h.origin_type).collect();
        assert_eq!(
            hit_types,
            [OriginType::Knowledge, OriginType::Spec, OriginType::Freeform],
            "{:?}",
            all.hits
        );
        // Two knowledge entries, a spec and a change's proposal, two documents.
        assert_eq!(all.total, 6);
        assert!(all.hits[0].facet.is_some() && all.hits[2].facet.is_none());

        let freeform_only = search("tag", Vec::new(), vec![OriginType::Freeform]);
        assert_eq!(freeform_only.hits.len(), 1);
        assert_eq!(freeform_only.hits[0].root_key, w.freeform_key());
        assert_eq!(freeform_only.total, 2, "only the type searched is counted");

        let one_origin = search("release", vec![w.spec_key()], Vec::new());
        assert!(one_origin.hits.iter().all(|h| h.root_key == w.spec_key()));
        assert_eq!(one_origin.hits.len(), 1);
    }

    #[test]
    fn only_a_knowledge_origin_takes_an_override() {
        let w = world("override");
        for key in [w.spec_key(), w.freeform_key()] {
            let e = w.err(ActionRequest::LibraryOverride {
                root: key.clone(),
                path: "templates/briefs/task-start.md".into(),
            });
            assert!(e.contains("do not layer"), "{key}: {e}");
        }
        assert!(!w.spec.join("templates").exists());
        assert!(!w.freeform.join("templates").exists());

        // The layering answer names knowledge origins only.
        let layering = w.ok(ActionRequest::LibraryLayering);
        let roots: Vec<&str> = layering["roots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["key"].as_str().unwrap())
            .collect();
        assert_eq!(roots, [w.knowledge_key().as_str()]);
        assert_eq!(
            layering["paths"]["templates/briefs/task-start.md"]["applied"],
            w.knowledge_key().as_str(),
            "the store's own brief is what a launch reads"
        );
    }

    #[test]
    fn removing_a_folder_origin_asks_for_the_setting_to_change() {
        let w = world("remove");
        let spec = w.outcome(ActionRequest::LibraryStoreUnregister { root: w.spec_key() });
        assert!(matches!(spec.result, ActionResult::Ok(_)));
        assert_eq!(
            spec.edit,
            Some(SettingsEdit::RemoveSpecFolder {
                space: "default".into(),
                folder: w.spec.to_string_lossy().into_owned(),
            })
        );
        let freeform = w.outcome(ActionRequest::LibraryStoreUnregister {
            root: w.freeform_key(),
        });
        assert_eq!(
            freeform.edit,
            Some(SettingsEdit::RemoveFreeformFolder {
                space: "default".into(),
                folder: w.freeform.to_string_lossy().into_owned(),
            })
        );

        // Applied, both folders are gone from the space and nothing else is.
        let mut settings = w.settings.clone();
        spec.edit.unwrap().apply(&mut settings).unwrap();
        freeform.edit.unwrap().apply(&mut settings).unwrap();
        let library = &settings.active_space().library;
        assert!(library.spec.folders.is_empty() && library.freeform.folders.is_empty());
        assert!(w.spec.is_dir() && w.freeform.is_dir(), "the folders stay on disk");

        // A registered store is forgotten by its registry, with no setting to
        // change.
        let knowledge = w.outcome(ActionRequest::LibraryStoreUnregister {
            root: w.knowledge_key(),
        });
        assert!(matches!(knowledge.result, ActionResult::Ok(_)), "unregistered");
        assert!(knowledge.edit.is_none());
        assert!(w.origins().origin(&w.knowledge_key()).is_none());
        assert!(w.knowledge.is_dir());
    }

    #[test]
    fn an_edit_for_a_space_that_has_gone_fails_rather_than_landing_elsewhere() {
        let mut settings = AppSettings::default();
        let edit = SettingsEdit::AddFreeformFolder {
            space: "client-a".into(),
            folder: "/notes".into(),
        };
        let e = edit.apply(&mut settings).unwrap_err();
        assert!(e.contains("client-a"), "{e}");
        assert!(settings.active_space().library.freeform.folders.is_empty());

        settings
            .spaces
            .push(okena_core::spaces::SpaceData::new("client-a", "Client A"));
        edit.apply(&mut settings).unwrap();
        assert_eq!(
            settings.space("client-a").unwrap().library.freeform.folders,
            ["/notes"]
        );
        assert!(
            settings.active_space().library.freeform.folders.is_empty(),
            "the active space was not the one named"
        );
    }

    #[test]
    fn the_session_actions_are_not_run_here() {
        let draft = ActionRequest::LibraryDraft {
            root: None,
            request: "x".into(),
            name: None,
            agent_command: None,
            model: None,
            context: Vec::new(),
        };
        assert!(!is_library_action(&draft));
        let w = world("session");
        let sources = library_sources(&[], &w.settings);
        assert!(execute_at(&w.registry, &draft, &sources, &w.settings).is_none());
    }
}
