//! The Library's `spec` origins: OpenSpec roots.
//!
//! `library.rs` hands this module every Library action that is about a spec
//! origin. The Library wraps OpenSpec and changes nothing about it: the
//! `openspec/` layout, the CLI's machine registry and its lock, and
//! `defaultStore` are read and written exactly as the CLI does, so whatever
//! okena does here the CLI reads back and the other way round.
//!
//! okena works with OpenSpec (<https://github.com/Fission-AI/OpenSpec>) the way
//! the `openspec` CLI does — stores registered on this machine, roots found in
//! projects, and folders from settings (<https://openspec.dev/docs/stores>) —
//! but reads and writes the files itself through `okena-openspec`. Browsing
//! works on a machine that has never installed the CLI, and an agent drafting a
//! change is still free to use it.
//!
//! Reads are scoped to roots discovery found and path-checked against them:
//! these actions are reachable by any client and by agents through okena's MCP
//! server, so they must not become a way to read arbitrary files.

use super::ActionResult;
use super::briefs::{self, PromptRoots};
use crate::workspace::persistence::AppSettings;
use crate::workspace::state::{ProjectData, WindowId, Workspace};
use super::library::SettingsEdit;
use okena_core::api::ActionRequest;
use okena_core::doc_search::{LibrarySearchFilter, SearchDoc, library_matches};
use okena_core::library::{
    LibraryDocument, LibraryHit, LibrarySearchResult, LibraryTree, OriginType,
};
use okena_core::specs::{SpecDoc, SpecRoot, SpecRootKind, SpecStores, change_slug};
use okena_knowledge::prompts::{Flow, Vars};
use okena_openspec::discover::{self, ProjectSource, Sources};
use okena_openspec::files::{self, DEFAULT_SCHEMA};
use okena_openspec::{OpenSpecDirs, registry, setup, tree};
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::TerminalBackend;
use okena_workspace::context::WorkspaceCx;
use std::path::{Path, PathBuf};

/// OpenSpec's machine directories, with overrides from settings.
pub(super) fn dirs(settings: &AppSettings) -> OpenSpecDirs {
    let specs = &settings.active_space().library.spec;
    OpenSpecDirs::detect(specs.data_dir.as_deref(), specs.config_dir.as_deref())
}

/// What discovery looks at: the registry, the projects and the folders.
///
/// Copied out of the workspace, so the daemon can run discovery — and the git
/// a listing runs in every store — without holding the workspace lock.
pub fn spec_sources(projects: &[ProjectData], settings: &AppSettings) -> Sources {
    let space = settings.active_space();
    let specs = &space.library.spec;
    let projects = if specs.projects {
        projects
            .iter()
            // Each space has its own roots, so only its own projects can
            // contribute one.
            .filter(|p| p.space_id == settings.active_space)
            // A worktree is a second checkout of a repo already listed, and a
            // session is rooted at a spec root or above several repos — neither
            // is a root of its own. Any session, not only spec drafts: one
            // refining a document sits at the root it edits.
            .filter(|p| p.worktree_info.is_none() && !p.is_any_agent_session())
            .map(|p| ProjectSource {
                name: p.name.clone(),
                path: p.path.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };
    Sources {
        registry: specs.registry,
        projects,
        folders: space.spec_folders(),
    }
}

fn to_result(value: serde_json::Result<serde_json::Value>, what: &str) -> ActionResult {
    match value {
        Ok(v) => ActionResult::Ok(Some(v)),
        Err(e) => ActionResult::Err(format!("could not serialize {what}: {e}")),
    }
}

/// Every OpenSpec root discovery finds, under its Library key.
///
/// The one place this module discovers, and so the one place a root's key
/// becomes its Library key (`spec:store:team-plans`): resolving the key a
/// client sent back, a tree's `root_key`, a search hit and a session's
/// recorded origin all read it from here.
pub(super) fn discovered(sources: &Sources, settings: &AppSettings) -> SpecStores {
    let mut stores = discover::discover(&dirs(settings), sources);
    okena_core::library::key_spec_stores(&mut stores);
    stores
}

/// Every root, with sync state on each store and folder at the top of a git
/// checkout. A project root carries none: its project's own git owns it.
pub(super) fn listing(sources: &Sources, settings: &AppSettings) -> SpecStores {
    let mut stores = discovered(sources, settings);
    for root in stores
        .roots
        .iter_mut()
        .filter(|r| r.kind != SpecRootKind::Project && r.healthy)
    {
        root.git = okena_git::store::status(Path::new(&root.path));
    }
    stores
}

/// A result, and the settings change that goes with it when there is one.
type Outcome = (ActionResult, Option<SettingsEdit>);

/// Run a Library action against the spec origins; `None` for an action this
/// type has nothing to do with.
///
/// None of them touches the workspace, so the daemon runs them on its
/// blocking pool, against discovery sources copied out of the workspace,
/// rather than under the workspace lock: the store changes may wait on the
/// lock an `openspec` command holds, setup commits with git, and the store
/// git reaches the network.
pub(super) fn execute(
    action: &ActionRequest,
    sources: &Sources,
    settings: &AppSettings,
) -> Option<Outcome> {
    use okena_git::store;
    let plain = |result: ActionResult| (result, None);
    Some(match action {
        ActionRequest::LibraryTree { root } => plain(tree_for(sources, settings, root.clone())),
        ActionRequest::LibraryRead { root, path } => {
            plain(read_for(sources, settings, root.clone(), path.clone()))
        }
        ActionRequest::LibraryWrite {
            root,
            path,
            content,
            revision,
        } => plain(write_for(
            sources,
            settings,
            root.clone(),
            path.clone(),
            content.clone(),
            revision.clone(),
        )),
        ActionRequest::LibraryFileCreate {
            root,
            path,
            content,
        } => plain(in_root(sources, settings, root.clone(), |key, dir| {
            super::document_files::create_file(key, dir, path, content, MAX_DOC_BYTES)
        })),
        ActionRequest::LibraryFolderCreate { root, path } => {
            plain(in_root(sources, settings, root.clone(), |key, dir| {
                super::document_files::create_folder(key, dir, path)
            }))
        }
        ActionRequest::LibraryFileRename { root, from, to } => {
            plain(in_root(sources, settings, root.clone(), |key, dir| {
                super::document_files::rename(key, dir, tree::resolve_document, from, to)
            }))
        }
        ActionRequest::LibraryFileDelete { root, path } => {
            plain(in_root(sources, settings, root.clone(), |key, dir| {
                super::document_files::delete(key, dir, tree::resolve_document, path)
            }))
        }
        ActionRequest::LibraryOverride { .. } => plain(ActionResult::Err(
            super::freeform::no_layers(OriginType::Spec),
        )),
        ActionRequest::LibraryStoreClone { url, path, .. } => {
            plain(clone_store(settings, url, path.as_deref()))
        }
        ActionRequest::LibraryStoreRegister { path, id, .. } => {
            plain(register_store(settings, path.clone(), id.clone()))
        }
        ActionRequest::LibraryStoreUnregister { root } => unregister(sources, settings, root),
        ActionRequest::LibraryStoreSetup {
            id,
            path,
            remote,
            init_git,
            ..
        } => plain(setup_store(
            settings,
            id.clone(),
            path.clone(),
            remote.clone(),
            *init_git,
        )),
        ActionRequest::LibrarySetDefaultStore { id } => {
            plain(set_default_store(settings, id.clone()))
        }
        ActionRequest::LibraryStoreFetch { root } => plain(sync(sources, settings, root, |path| {
            store::fetch(path).map(|()| store::status(path).unwrap_or_default())
        })),
        ActionRequest::LibraryStorePull { root } => {
            plain(sync(sources, settings, root, store::pull))
        }
        ActionRequest::LibraryStoreCommit {
            root,
            paths,
            message,
        } => plain(sync(sources, settings, root, |path| {
            store::commit(path, paths, message)
        })),
        ActionRequest::LibraryStorePush { root } => {
            plain(sync(sources, settings, root, store::push))
        }
        _ => return None,
    })
}

/// Take the root `key` names off the list. The folder stays on disk.
///
/// Found among everything discovered rather than only the usable roots: a
/// store whose checkout has gone missing is exactly the one to remove. A store
/// is forgotten by OpenSpec's registry (`openspec store unregister <id>`); a
/// folder is a line in the space's settings, so removing it is a settings
/// write; a project's root belongs to its repository and cannot be removed
/// here.
fn unregister(sources: &Sources, settings: &AppSettings, key: &str) -> Outcome {
    let Some(root) = discovered(sources, settings).root(key).cloned() else {
        return (ActionResult::Err(super::library::unknown_origin(key)), None);
    };
    match (root.kind, root.store_id.clone()) {
        (SpecRootKind::Store, Some(id)) => (unregister_store(settings, id), None),
        (SpecRootKind::Folder, _) => (
            ActionResult::Ok(Some(serde_json::json!({
                "id": root.name,
                "left_on_disk": root.path,
            }))),
            Some(SettingsEdit::RemoveSpecFolder {
                space: settings.active_space.clone(),
                folder: root.path,
            }),
        ),
        _ => (
            ActionResult::Err(format!(
                "`{}` is part of a project, not a store — it goes when its `openspec/` folder does",
                root.name
            )),
            None,
        ),
    }
}

/// Run `op` in the checkout of the store or folder root `key` names. Replies
/// with its sync state after.
fn sync(
    sources: &Sources,
    settings: &AppSettings,
    key: &str,
    op: impl FnOnce(
        &Path,
    )
        -> Result<okena_core::store_git::StoreGitStatus, okena_git::store::StoreGitError>,
) -> ActionResult {
    let root = match resolve_root_in(sources, settings, Some(key)) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    if root.kind == SpecRootKind::Project {
        return ActionResult::Err(format!(
            "`{}` is part of a project, not a store; commit and sync it with the project's own git",
            root.name
        ));
    }
    match op(Path::new(&root.path)) {
        Ok(status) => to_result(serde_json::to_value(status), "sync state"),
        Err(e) => ActionResult::Err(e.to_string()),
    }
}

/// A root the client named, checked against what discovery found — never a
/// path taken on trust.
pub(super) fn resolve_root(
    projects: &[ProjectData],
    settings: &AppSettings,
    key: Option<&str>,
) -> Result<SpecRoot, String> {
    resolve_root_in(&spec_sources(projects, settings), settings, key)
}

fn resolve_root_in(
    sources: &Sources,
    settings: &AppSettings,
    key: Option<&str>,
) -> Result<SpecRoot, String> {
    let stores = discovered(sources, settings);
    match key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(key) => stores.root(key).cloned().ok_or_else(|| {
            format!(
                "unknown spec origin `{key}` — it is no longer discovered; refresh the Library"
            )
        }),
        None => stores.default_root().cloned().ok_or_else(|| {
            "no OpenSpec roots found — register a store or add a folder in Settings → Library"
                .to_string()
        }),
    }
}

fn tree_for(sources: &Sources, settings: &AppSettings, root: Option<String>) -> ActionResult {
    let root = match resolve_root_in(sources, settings, root.as_deref()) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    let mut t = tree::read_tree(Path::new(&root.path));
    t.root_key = root.key.clone();
    // Only a registered store can be selected with `--store`; a folder that
    // merely holds store metadata cannot.
    if root.kind == SpecRootKind::Store {
        t.store_id = root.store_id.clone();
    }
    to_result(serde_json::to_value(LibraryTree::Spec(t)), "spec tree")
}

/// Add every spec document `filter` keeps to `result` (QBL-436).
///
/// Only roots discovery found are read, and only the documents their trees
/// list, through the path check a read goes through. A root that is not
/// usable is skipped rather than failing the search.
pub(super) fn search(
    sources: &Sources,
    settings: &AppSettings,
    filter: &LibrarySearchFilter,
    result: &mut LibrarySearchResult,
) {
    // The origin and type choices alone, to tell whether a document is worth
    // opening.
    let groups = filter.without_text();
    for root in discovered(sources, settings)
        .roots
        .iter()
        .filter(|r| r.healthy)
    {
        let dir = Path::new(&root.path);
        let t = tree::read_tree(dir);
        // The order the sidebar lists them in: changes, specs, the archive.
        let in_change = |c: &okena_core::specs::SpecChange, prefix: &str| -> Vec<(String, SpecDoc)> {
            c.artifacts
                .iter()
                .chain(c.specs.iter())
                .map(|d| (format!("{prefix}{}/{}", c.name, d.name), d.clone()))
                .collect()
        };
        let docs = t
            .changes
            .iter()
            .flat_map(|c| in_change(c, ""))
            .chain(t.specs.iter().map(|d| (d.name.clone(), d.clone())))
            .chain(t.archived.iter().flat_map(|c| in_change(c, "archive/")));
        for (label, d) in docs {
            result.total += 1;
            let names = [label.as_str()];
            let paths = [d.path.as_str()];
            let doc = |content| SearchDoc {
                root_key: &root.key,
                names: &names,
                paths: &paths,
                content,
            };
            let matches = |f: &LibrarySearchFilter, content| {
                library_matches(f, OriginType::Spec, None, &doc(content))
            };
            // Names and paths first, and a document its origin has already
            // ruled out is never opened.
            let hit = matches(filter, "")
                || (matches(&groups, "") && matches(filter, &searchable_text(dir, &d.path)));
            if hit {
                result.hits.push(LibraryHit {
                    root_key: root.key.clone(),
                    origin_type: OriginType::Spec,
                    path: d.path,
                    label,
                    facet: None,
                });
            }
        }
    }
}

/// A listed document's text for searching: empty when it cannot be read as
/// text or is past what a read would show, so it can still match by name.
fn searchable_text(root: &Path, path: &str) -> String {
    let Ok(real) = tree::resolve_document(root, path) else {
        return String::new();
    };
    if real.metadata().is_ok_and(|m| m.len() > MAX_DOC_BYTES) {
        return String::new();
    }
    std::fs::read_to_string(&real).unwrap_or_default()
}

/// Largest document this action will return, in bytes.
///
/// Specs are prose; anything past this is not a spec, and streaming a huge file
/// through a JSON action response would stall the client for no benefit.
const MAX_DOC_BYTES: u64 = 2 * 1024 * 1024;

fn read_for(
    sources: &Sources,
    settings: &AppSettings,
    root: Option<String>,
    path: String,
) -> ActionResult {
    let root = match resolve_root_in(sources, settings, root.as_deref()) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    let real = match tree::resolve_document(Path::new(&root.path), &path) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    if let Ok(m) = real.metadata()
        && m.len() > MAX_DOC_BYTES
    {
        return ActionResult::Err(format!(
            "document is too large to display ({} KB)",
            m.len() / 1024
        ));
    }
    match std::fs::read_to_string(&real) {
        Ok(content) => to_result(
            serde_json::to_value(LibraryDocument {
                root_key: root.key,
                revision: okena_core::fs::content_revision(&content),
                path,
                content,
            }),
            "spec document",
        ),
        Err(e) => ActionResult::Err(format!("could not read {path}: {e}")),
    }
}

/// Replace an existing document, through the same root and path checks as
/// [`read_for`]: a write must not be a way out of a root that a read is not.
fn write_for(
    sources: &Sources,
    settings: &AppSettings,
    root: Option<String>,
    path: String,
    content: String,
    revision: String,
) -> ActionResult {
    let root = match resolve_root_in(sources, settings, root.as_deref()) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    let real = match tree::resolve_document(Path::new(&root.path), &path) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    // What could not be opened must not be written either.
    if content.len() as u64 > MAX_DOC_BYTES {
        return ActionResult::Err(format!(
            "document is too large to save ({} KB)",
            content.len() / 1024
        ));
    }
    match okena_core::fs::replace_if_unchanged(&real, &content, &revision) {
        Ok(revision) => ActionResult::Ok(Some(serde_json::json!({
            "root": root.key,
            "path": path,
            "revision": revision,
        }))),
        Err(e) => ActionResult::Err(e.describe(&path)),
    }
}

// ─── Files and folders in a root ─────────────────────────────────────────────
//
// Deleting a change's last document leaves its folder, and the tree still
// lists that change ("no artifacts yet"): an empty change directory is what
// `openspec new change` makes too. A capability folder emptied of its `spec.md`
// is simply no longer listed.

/// Run `op` with the key and path of the root the client named, found exactly
/// as a read finds it.
fn in_root(
    sources: &Sources,
    settings: &AppSettings,
    root: Option<String>,
    op: impl FnOnce(&str, &Path) -> ActionResult,
) -> ActionResult {
    match resolve_root_in(sources, settings, root.as_deref()) {
        Ok(root) => op(&root.key, Path::new(&root.path)),
        Err(e) => ActionResult::Err(e),
    }
}

// ─── Store management ────────────────────────────────────────────────────────

/// Clone an OpenSpec store repository and register the checkout.
///
/// The clone itself is shared with freeform origins
/// (`library::clone_checkout`): `okena-openspec` deliberately carries no git
/// beyond `init` and the first commit.
///
/// A repository that turns out not to be an OpenSpec root is left on disk: the
/// clone is the user's to keep or remove, and the error says where it is.
fn clone_store(
    settings: &AppSettings,
    url: &str,
    dest: Option<&str>,
) -> ActionResult {
    let clone_dir = settings.active_space().library.spec.clone_dir();
    let target = match super::library::clone_checkout(url, dest, &clone_dir, "store") {
        Ok(t) => t,
        Err(e) => return ActionResult::Err(e),
    };
    match registry::register(&dirs(settings), &target.to_string_lossy(), None) {
        Ok(r) => ActionResult::Ok(Some(serde_json::json!({
            "id": r.id,
            "root": r.root.to_string_lossy(),
            "metadata_created": r.metadata_created,
            "already_registered": r.already_registered,
        }))),
        Err(e) => ActionResult::Err(format!(
            "Cloned into {}, but it can't be added: {e}",
            target.display()
        )),
    }
}

fn register_store(
    settings: &AppSettings,
    path: String,
    id: Option<String>,
) -> ActionResult {
    match registry::register(&dirs(settings), &path, id.as_deref()) {
        Ok(r) => ActionResult::Ok(Some(serde_json::json!({
            "id": r.id,
            "root": r.root.to_string_lossy(),
            "metadata_created": r.metadata_created,
            "already_registered": r.already_registered,
        }))),
        Err(e) => ActionResult::Err(e.to_string()),
    }
}

fn unregister_store(settings: &AppSettings, id: String) -> ActionResult {
    match registry::unregister(&dirs(settings), &id) {
        Ok(left) => ActionResult::Ok(Some(serde_json::json!({
            "id": id,
            "left_on_disk": left.to_string_lossy(),
        }))),
        Err(e) => ActionResult::Err(e.to_string()),
    }
}

fn setup_store(
    settings: &AppSettings,
    id: String,
    path: String,
    remote: Option<String>,
    init_git: bool,
) -> ActionResult {
    let request = setup::SetupRequest {
        id,
        path,
        remote,
        init_git,
    };
    match setup::setup_store(&dirs(settings), &request) {
        Ok(out) => ActionResult::Ok(Some(serde_json::json!({
            "id": out.id,
            "root": out.root.to_string_lossy(),
            "created": out.created,
            "git_initialized": out.git_initialized,
            "committed": out.committed,
            "already_registered": out.already_registered,
        }))),
        Err(e) => ActionResult::Err(e.to_string()),
    }
}

fn set_default_store(settings: &AppSettings, id: Option<String>) -> ActionResult {
    let id = id.map(|i| i.trim().to_string()).filter(|i| !i.is_empty());
    match setup::set_default_store(&dirs(settings), id.as_deref()) {
        Ok(()) => ActionResult::Ok(Some(serde_json::json!({ "default_store": id }))),
        Err(e) => ActionResult::Err(e.to_string()),
    }
}

// ─── Drafting a change ───────────────────────────────────────────────────────

/// The proposal stub okena writes when scaffolding a change.
///
/// Deliberately thin. Its job is to record the idea verbatim so the change is
/// browsable and nothing is lost if the agent is closed straight away — the
/// thinking is the agent's, and pre-filling headings it may not want would
/// fight OpenSpec's "fluid not rigid" stance.
fn proposal_stub(idea: &str) -> String {
    format!("# {idea}\n\n## Why\n\n{idea}\n\n## What Changes\n\n_Drafting._\n")
}

/// Today's local date as `YYYY-MM-DD` — what `openspec new change` records
/// (`formatLocalDate`). Falls back to UTC where the local offset cannot be
/// determined safely.
fn today() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    date_string(now.date())
}

fn date_string(date: time::Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

/// Brief an agent to fill in a scaffolded change.
///
/// The prose is a template now (`spec-draft`), so an organisation can change
/// how its agents are briefed without a release. What stays here is the part a
/// template cannot decide: whether this root is a store worth naming, and
/// which referenced stores exist to cite.
fn brief(
    idea: &str,
    change: &str,
    change_dir: &str,
    root: &SpecRoot,
    context_items: &[okena_core::context::ContextItem],
    loaded: bool,
    prompts: &PromptRoots,
) -> String {
    let mut vars = Vars::new();
    vars.insert("idea", idea.to_string());
    vars.insert("change", change.to_string());
    vars.insert("change_dir", change_dir.to_string());
    vars.insert("root_path", root.path.clone());
    vars.insert("store_note", store_note(change, root, prompts));
    vars.insert("references", reference_note(root, prompts));
    vars.insert(
        "context",
        briefs::context_block(context_items, loaded, prompts),
    );
    briefs::build(Flow::SpecDraft, prompts, &vars)
        .rendered
        .text
}

/// What to say about the `openspec` CLI, which depends on whether this root is
/// a store the CLI can be pointed at by id. The decision is here; the words
/// are the `spec-store-note` and `spec-folder-note` partials.
fn store_note(change: &str, root: &SpecRoot, prompts: &PromptRoots) -> String {
    let (name, vars) = match (root.kind, root.store_id.as_deref()) {
        (SpecRootKind::Store, Some(id)) => (
            "spec-store-note",
            Vars::from([("store_id", id.to_string()), ("change", change.to_string())]),
        ),
        _ => ("spec-folder-note", Vars::new()),
    };
    briefs::block(&briefs::fragment(name, prompts, &vars))
}

/// The referenced stores, as read-only upstream context — or nothing.
fn reference_note(root: &SpecRoot, prompts: &PromptRoots) -> String {
    let lines: Vec<String> = root
        .references
        .iter()
        .filter_map(|r| r.root.as_ref().map(|path| (r.id.as_str(), path.as_str())))
        .map(|(id, path)| {
            briefs::fragment(
                "spec-reference",
                prompts,
                &Vars::from([("store_id", id.to_string()), ("path", path.to_string())]),
            )
        })
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    briefs::block(&briefs::fragment(
        "spec-references",
        prompts,
        &Vars::from([("list", lines.join("\n"))]),
    ))
}

/// How to hand `command` an opening prompt.
///
/// Agents disagree here, so this is a small per-agent table like the MCP-flag
/// one. Note the difference in kind: `claude` takes a positional prompt and
/// stays interactive, while `copilot`'s only prompt flag is non-interactive and
/// exits when it is done — a drafted spec either way, but only one of them
/// leaves you in a conversation.
pub(super) fn prompt_args(command: &str, prompt: &str) -> Vec<String> {
    let program = Path::new(command)
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match program.as_str() {
        "copilot" => vec!["--prompt".into(), prompt.into()],
        // `claude` and, by convention, anything else: a positional prompt.
        _ => vec![prompt.into()],
    }
}

/// Shell for an agent session opened with `prompt` — a spec draft here, and a
/// knowledge draft in `knowledge.rs`.
pub(super) fn spec_agent_shell(
    settings: &AppSettings,
    override_command: Option<&str>,
    prompt: &str,
    install: &super::agent_context::Install,
    model: &super::agent_options::LaunchModel,
) -> Option<okena_terminal::shell_config::ShellType> {
    // An explicit empty string means "scaffold only, no agent", even when a
    // default agent is configured — same contract as starting work on a task.
    let command = match override_command {
        Some(c) => c,
        None => settings.harness.agent_command.as_deref().unwrap_or(""),
    }
    .trim()
    .to_string();
    if command.is_empty() {
        return None;
    }
    // Named first, so a restart can resume this exact conversation; the
    // agent's own options before the prompt, which they never replace.
    let mut args = super::agent_resume::session_args(&command);
    args.extend(super::agent_options::option_args(
        &command,
        settings,
        model.for_agent(&command).as_deref(),
    ));
    args.extend(super::briefs::brief_args(&command, prompt));
    args.extend(super::agent_mcp::injection_args(&command, settings));
    // Skills and agents picked at launch, where this agent loads them itself.
    args.extend(install.args.iter().cloned());
    Some(okena_terminal::shell_config::ShellType::Custom {
        path: command,
        args,
    })
}

/// Create `openspec/changes/<slug>/` the way `openspec new change` does, plus
/// the proposal stub. Returns the change directory.
fn scaffold_change(root: &SpecRoot, slug: &str, idea: &str) -> Result<PathBuf, String> {
    let change_dir = Path::new(&root.path)
        .join("openspec")
        .join("changes")
        .join(slug);
    // Refuse rather than merge into an existing change: the user asked to start
    // something new, and writing a fresh stub over a change already being
    // worked on would destroy it.
    if change_dir.exists() {
        return Err(format!(
            "a change named `{slug}` already exists — open it, or reword the idea"
        ));
    }
    std::fs::create_dir_all(&change_dir)
        .map_err(|e| format!("could not create the change directory: {e}"))?;
    let schema = root.schema.as_deref().unwrap_or(DEFAULT_SCHEMA);
    std::fs::write(
        change_dir.join(files::CHANGE_METADATA_FILE),
        files::change_metadata_yaml(schema, &today()),
    )
    .map_err(|e| format!("could not write {}: {e}", files::CHANGE_METADATA_FILE))?;
    std::fs::write(change_dir.join("proposal.md"), proposal_stub(idea))
        .map_err(|e| format!("could not write proposal.md: {e}"))?;
    Ok(change_dir)
}

/// Scaffold a change directory and open an agent session to fill it in.
#[allow(clippy::too_many_arguments)]
pub(super) fn draft_change(
    ws: &mut Workspace,
    window_id: WindowId,
    idea: String,
    name: Option<String>,
    agent_command: Option<String>,
    model: Option<String>,
    root: Option<String>,
    context_refs: Vec<okena_core::context::ContextRef>,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let idea = idea.trim().to_string();
    if idea.is_empty() {
        return ActionResult::Err("describe the change in a sentence first".into());
    }
    let context_items =
        super::context::resolve_for_launch(&ws.data.projects, settings, &context_refs);
    let root = match resolve_root(&ws.data.projects, settings, root.as_deref()) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    if !root.healthy {
        let why = root
            .status
            .first()
            .map(|d| d.message.clone())
            .unwrap_or_else(|| "it is not a usable OpenSpec root".into());
        return ActionResult::Err(format!("can't draft in `{}`: {why}", root.name));
    }
    // An explicit name wins; otherwise derive one from the prompt. Slugged
    // either way, since a name typed by hand is no more filesystem-safe than a
    // sentence.
    let slug = match name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => change_slug(n),
        None => change_slug(&idea),
    };
    if slug.is_empty() {
        return ActionResult::Err(
            "that name has no letters or numbers to name a change after".into(),
        );
    }
    let change_dir = match scaffold_change(&root, &slug, &idea) {
        Ok(d) => d,
        Err(e) => return ActionResult::Err(e),
    };
    let root_path = PathBuf::from(&root.path);
    let change_rel = tree::rel(&root_path, &change_dir);

    // ── Agent session ────────────────────────────────────────────────────────
    //
    // Rooted at the OpenSpec root, not the change directory: OpenSpec asks an
    // author to read the existing `openspec/specs/` before proposing, and an
    // agent confined to the new directory cannot.
    let mut session: Option<serde_json::Value> = None;
    // No "(spec)" suffix: the sidebar badges the row with what it is.
    let name = slug.clone();
    match ws.add_project(
        name.clone(),
        root.path.clone(),
        true,
        &settings.hooks,
        window_id,
        cx,
    ) {
        Ok(project_id) => {
            // Mark it before spawning, so the session is recognizable as a
            // spec session from the first snapshot the client sees.
            if let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == project_id) {
                p.spec_change = Some(slug.clone());
                // With its root: two roots can each hold a change of this name.
                p.agent_purpose = Some(okena_core::harness::AgentPurpose::SpecDraft {
                    root: root.key.clone(),
                    change: slug.clone(),
                });
                p.context_projects = super::context::scope_projects(&[], &context_items);
            }
            // Set before spawning: the terminal reads the project's default
            // shell as it starts.
            let command = super::agent_context::launch_command(settings, agent_command.as_deref());
            let install = super::agent_context::install(&command, &context_items);
            let prompts = briefs::prompt_roots(&ws.data.projects, settings);
            if let Some(shell) = spec_agent_shell(
                settings,
                agent_command.as_deref(),
                &brief(
                    &idea,
                    &slug,
                    &change_rel,
                    &root,
                    &context_items,
                    install.loaded(),
                    &prompts,
                ),
                &install,
                &briefs::launch_model(Flow::SpecDraft, &prompts, model),
            ) && let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == project_id)
            {
                p.default_shell = Some(shell);
            }
            if let ActionResult::Err(e) =
                super::spawn_session_terminals(ws, &project_id, backend, terminals, settings, cx)
            {
                log::warn!("[specs] spec session terminal failed to spawn: {e}");
            }
            session = Some(serde_json::json!({
                "project_id": project_id,
                "name": name,
            }));
        }
        // The change directory is real and browsable even without a session, so
        // report the failure rather than failing the whole call.
        Err(e) => log::warn!("[specs] could not create spec session project: {e}"),
    }

    ws.notify_data(cx);

    ActionResult::Ok(Some(serde_json::json!({
        "root": root.key,
        "change": slug,
        "path": change_rel,
        "session": session,
    })))
}

#[cfg(test)]
mod tests {
    use super::{
        ActionResult, brief, date_string, prompt_args, proposal_stub, read_for, resolve_root,
        scaffold_change, tree_for, write_for,
    };
    use super::{clone_store, dirs};
    use okena_core::doc_search::LibrarySearchFilter;
    use okena_core::library::LibrarySearchResult;

    /// Discovery sources with no projects: the registry and the folders.
    fn src(settings: &AppSettings) -> super::Sources {
        super::spec_sources(&[], settings)
    }
    use crate::workspace::persistence::AppSettings;
    use okena_core::specs::{SpecRootKind, SpecTree};
    use std::path::{Path, PathBuf};

    fn tmpdir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "okena-specs-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// Settings whose OpenSpec directories live in `sandbox`, so no test ever
    /// reads — or writes — the developer's real store registry.
    fn sandboxed(sandbox: &Path) -> AppSettings {
        let mut settings = AppSettings::default();
        settings.active_space_mut().library.spec.data_dir = Some(sandbox.join("data").to_string_lossy().into());
        settings.active_space_mut().library.spec.config_dir = Some(sandbox.join("config").to_string_lossy().into());
        settings
    }

    fn populated_root(root: &Path) {
        let os = root.join("openspec");
        write(&os.join("config.yaml"), "schema: spec-driven\n");
        write(&os.join("specs/auth/spec.md"), "# Auth");
        write(&os.join("changes/add-login/proposal.md"), "# Why");
        write(&os.join("changes/add-login/specs/login/spec.md"), "# Login");
        write(
            &os.join("changes/archive/2026-01-01-old-thing/proposal.md"),
            "# Old",
        );
    }

    fn tree_of(settings: &AppSettings, root: Option<String>) -> SpecTree {
        let ActionResult::Ok(Some(v)) = tree_for(&src(settings), settings, root) else {
            panic!("expected a spec tree");
        };
        serde_json::from_value(v).unwrap()
    }

    /// Run `git` in `dir`, failing loudly: a silent git failure would make the
    /// clone test pass for the wrong reason.
    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "okena")
            .env("GIT_AUTHOR_EMAIL", "okena@example.com")
            .env("GIT_COMMITTER_NAME", "okena")
            .env("GIT_COMMITTER_EMAIL", "okena@example.com")
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn a_spec_store_can_be_cloned_and_lands_registered_in_the_clone_folder() {
        // QBL-415: Specs gained clone so it offers the same three ways to add
        // a root as Knowledge. Served over file:// so the test needs no network.
        let sandbox = tmpdir("clone");
        let seed = sandbox.join("seed");
        populated_root(&seed);
        git(&sandbox, &["init", "-q", "-b", "main", "seed"]);
        git(&seed, &["add", "."]);
        git(&seed, &["commit", "-q", "-m", "seed"]);
        git(&sandbox, &["clone", "-q", "--bare", "seed", "remote.git"]);

        let mut settings = sandboxed(&sandbox);
        let clone_dir = sandbox.join("openspec");
        settings.active_space_mut().library.spec.clone_dir = Some(clone_dir.to_string_lossy().into_owned());
        let url = format!("file://{}", sandbox.join("remote.git").display());

        let ActionResult::Ok(Some(v)) = clone_store(&settings, &url, None) else {
            panic!("expected the clone to succeed");
        };
        // Named the way `git clone` would, under the configured clone folder.
        let root = clone_dir.join("remote");
        assert!(root.join("openspec/config.yaml").is_file(), "{v}");
        assert_eq!(
            okena_openspec::registry::list(&dirs(&settings)).unwrap().len(),
            1,
            "the checkout was not registered"
        );

        // A second clone into the same place is refused, and says why.
        let ActionResult::Err(e) = clone_store(&settings, &url, None) else {
            panic!("expected a refusal");
        };
        assert!(e.contains("already exists"), "{e}");

        // A URL git would treat as an option is refused before anything runs.
        let ActionResult::Err(e) = clone_store(&settings, " --upload-pack=touch ", None) else {
            panic!("expected a refusal");
        };
        assert!(e.contains("repository URL"), "{e}");
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn the_legacy_spec_repo_setting_still_opens_as_a_folder_root() {
        // Existing installs set `spec_repo`; they must keep working unchanged.
        let sandbox = tmpdir("legacy");
        let repo = sandbox.join("specs");
        populated_root(&repo);
        // `spec_repo` is a legacy key now: the migration folds it into the
        // Default space's folders, which is what this exercises end to end.
        let mut settings = AppSettings::default();
        settings.spaces.clear();
        settings.harness.legacy_spec_repo = Some(repo.to_string_lossy().into_owned());
        settings.ensure_spaces();
        settings.active_space_mut().library.spec.data_dir =
            Some(sandbox.join("data").to_string_lossy().into());
        settings.active_space_mut().library.spec.config_dir =
            Some(sandbox.join("config").to_string_lossy().into());

        let t = tree_of(&settings, None);
        assert!(t.initialized);
        assert!(t.root_key.starts_with("spec:path:"));
        assert!(t.store_id.is_none());
        assert_eq!(t.specs[0].name, "auth");
        assert_eq!(t.changes[0].name, "add-login");
        assert_eq!(t.changes[0].specs[0].name, "login");
        assert_eq!(t.archived[0].name, "2026-01-01-old-thing");
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn the_default_store_opens_by_default_and_carries_its_id() {
        let sandbox = tmpdir("store");
        let settings = sandboxed(&sandbox);
        let dirs = super::dirs(&settings);
        let store = sandbox.join("stores/team-plans");
        populated_root(&store);
        write(
            &store.join(".openspec-store/store.yaml"),
            "version: 1\nid: team-plans\n",
        );
        okena_openspec::registry::register(&dirs, &store.to_string_lossy(), None).unwrap();
        okena_openspec::setup::set_default_store(&dirs, Some("team-plans")).unwrap();

        let t = tree_of(&settings, None);
        assert_eq!(t.root_key, "spec:store:team-plans");
        assert_eq!(t.store_id.as_deref(), Some("team-plans"));
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn nothing_configured_says_where_to_configure_it() {
        let sandbox = tmpdir("empty");
        let ActionResult::Err(e) = tree_for(&src(&sandboxed(&sandbox)), &sandboxed(&sandbox), None) else {
            panic!("expected an error");
        };
        assert!(e.contains("Settings → Library"), "unhelpful: {e}");
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn a_root_key_the_daemon_did_not_discover_is_refused() {
        // The key names a root; it must not be a way to name any directory.
        let sandbox = tmpdir("unknown-key");
        let secret = sandbox.join("elsewhere/openspec/secret.md");
        write(&secret, "SECRET");
        let key = format!("spec:path:{}", sandbox.join("elsewhere").to_string_lossy());
        let settings = sandboxed(&sandbox);
        assert!(resolve_root(&[], &settings, Some(&key)).is_err());
        assert!(matches!(
            read_for(&src(&settings), &settings, Some(key), "openspec/secret.md".into()),
            ActionResult::Err(_)
        ));
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn reading_outside_the_root_is_refused_through_the_action() {
        let sandbox = tmpdir("escape");
        let repo = sandbox.join("specs");
        populated_root(&repo);
        write(&sandbox.join("outside.md"), "SECRET");
        let mut settings = sandboxed(&sandbox);
        settings.active_space_mut().library.spec.folders = vec![repo.to_string_lossy().into_owned()];

        let ActionResult::Ok(Some(v)) =
            read_for(&src(&settings), &settings, None, "openspec/specs/auth/spec.md".into())
        else {
            panic!("expected content");
        };
        assert_eq!(v["content"], "# Auth");
        assert!(matches!(
            read_for(&src(&settings), &settings, None, "openspec/../../outside.md".into()),
            ActionResult::Err(_)
        ));
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn a_write_lands_in_the_root_and_nowhere_else() {
        let sandbox = tmpdir("write");
        let repo = sandbox.join("specs");
        populated_root(&repo);
        write(&sandbox.join("outside.md"), "SECRET");
        let mut settings = sandboxed(&sandbox);
        settings.active_space_mut().library.spec.folders = vec![repo.to_string_lossy().into_owned()];
        let path = "openspec/changes/add-login/proposal.md";

        let ActionResult::Ok(Some(read)) = read_for(&src(&settings), &settings, None, path.into()) else {
            panic!("expected content");
        };
        let revision = read["revision"].as_str().unwrap().to_string();
        let ActionResult::Ok(Some(saved)) = write_for(
            &src(&settings),
            &settings,
            None,
            path.into(),
            "# Why not\n".into(),
            revision.clone(),
        ) else {
            panic!("expected the write to land");
        };
        let ActionResult::Ok(Some(reopened)) = read_for(&src(&settings), &settings, None, path.into()) else {
            panic!("expected content");
        };
        assert_eq!(reopened["content"], "# Why not\n");
        assert_eq!(saved["revision"], reopened["revision"]);

        // Saving again from the pre-save revision is a stale buffer.
        let ActionResult::Err(e) = write_for(
            &src(&settings),
            &settings,
            None,
            path.into(),
            "# Clobber".into(),
            revision,
        ) else {
            panic!("a stale revision must be refused");
        };
        assert!(e.contains("changed on disk"), "{e}");
        assert_eq!(
            std::fs::read_to_string(repo.join(path)).unwrap(),
            "# Why not\n"
        );

        // Containment, the way reads are checked: through `..`, and through a
        // root key discovery never found.
        let outside = okena_core::fs::content_revision("SECRET");
        assert!(matches!(
            write_for(
                &src(&settings),
                &settings,
                None,
                "openspec/../../outside.md".into(),
                "pwned".into(),
                outside.clone(),
            ),
            ActionResult::Err(_)
        ));
        let key = format!("spec:path:{}", sandbox.to_string_lossy());
        assert!(matches!(
            write_for(
                &src(&settings),
                &settings,
                Some(key),
                "outside.md".into(),
                "pwned".into(),
                outside
            ),
            ActionResult::Err(_)
        ));
        assert_eq!(
            std::fs::read_to_string(sandbox.join("outside.md")).unwrap(),
            "SECRET"
        );
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn files_are_created_renamed_and_deleted_inside_the_root_and_nowhere_else() {
        use super::super::document_files as files;
        use okena_openspec::tree::resolve_document;

        let sandbox = tmpdir("files");
        let repo = sandbox.join("specs");
        populated_root(&repo);
        write(&sandbox.join("outside.md"), "SECRET");
        let mut settings = sandboxed(&sandbox);
        settings.active_space_mut().library.spec.folders = vec![repo.to_string_lossy().into_owned()];
        let run =
            |op: &dyn Fn(&str, &Path) -> ActionResult| super::in_root(&src(&settings), &settings, None, op);
        let refused = |result: ActionResult| match result {
            ActionResult::Err(e) => e,
            ActionResult::Ok(_) => panic!("expected a refusal"),
        };

        // A new change directory, then a document in it: both show in the tree.
        assert!(matches!(
            run(&|k, d| files::create_folder(k, d, "openspec/changes/add-sso")),
            ActionResult::Ok(_)
        ));
        let ActionResult::Ok(Some(created)) = run(&|k, d| {
            files::create_file(
                k,
                d,
                "openspec/changes/add-sso/proposal.md",
                "# SSO\n",
                1024,
            )
        }) else {
            panic!("expected the file to be created");
        };
        assert_eq!(created["path"], "openspec/changes/add-sso/proposal.md");
        let t = tree_of(&settings, None);
        let change = t
            .changes
            .iter()
            .find(|c| c.name == "add-sso")
            .expect("listed");
        assert_eq!(change.artifacts[0].name, "proposal.md");

        // Collisions and names the tree could never list.
        assert!(
            refused(run(&|k, d| files::create_folder(
                k,
                d,
                "openspec/changes/add-sso"
            )))
            .contains("already exists")
        );
        assert!(
            refused(run(&|k, d| {
                files::create_file(k, d, "openspec/specs/auth/spec.md", "clobber", 1024)
            }))
            .contains("already exists")
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("openspec/specs/auth/spec.md")).unwrap(),
            "# Auth"
        );
        for bad in ["", "openspec/.draft.md", "openspec/../../outside-new.md"] {
            refused(run(&|k, d| files::create_file(k, d, bad, "x", 1024)));
        }
        assert!(!sandbox.join("outside-new.md").exists());

        // Rename follows the file, refuses an occupied or escaping target, and
        // refuses a source outside the root.
        let ActionResult::Ok(Some(renamed)) = run(&|k, d| {
            files::rename(
                k,
                d,
                resolve_document,
                "openspec/changes/add-sso/proposal.md",
                "openspec/changes/add-sso/design.md",
            )
        }) else {
            panic!("expected the rename to land");
        };
        assert_eq!(renamed["path"], "openspec/changes/add-sso/design.md");
        assert!(!repo.join("openspec/changes/add-sso/proposal.md").exists());
        assert!(
            refused(run(&|k, d| {
                files::rename(
                    k,
                    d,
                    resolve_document,
                    "openspec/changes/add-sso/design.md",
                    "openspec/specs/auth/spec.md",
                )
            }))
            .contains("already exists")
        );
        refused(run(&|k, d| {
            files::rename(
                k,
                d,
                resolve_document,
                "openspec/../../outside.md",
                "openspec/x.md",
            )
        }));
        refused(run(&|k, d| {
            files::rename(
                k,
                d,
                resolve_document,
                "openspec/changes/add-sso/design.md",
                "../../moved.md",
            )
        }));

        // Delete removes one file; folders and escapes are refused.
        refused(run(&|k, d| {
            files::delete(k, d, resolve_document, "openspec/changes/add-sso")
        }));
        refused(run(&|k, d| {
            files::delete(k, d, resolve_document, "openspec/../../outside.md")
        }));
        assert!(matches!(
            run(&|k, d| files::delete(
                k,
                d,
                resolve_document,
                "openspec/changes/add-sso/design.md"
            )),
            ActionResult::Ok(_)
        ));
        assert!(!repo.join("openspec/changes/add-sso/design.md").exists());
        assert_eq!(
            std::fs::read_to_string(sandbox.join("outside.md")).unwrap(),
            "SECRET"
        );

        // A root key discovery never found is no way in either.
        let key = format!("spec:path:{}", sandbox.to_string_lossy());
        assert!(matches!(
            super::in_root(&src(&settings), &settings, Some(key), |k, d| {
                files::create_file(k, d, "pwned.md", "x", 1024)
            }),
            ActionResult::Err(_)
        ));
        assert!(!sandbox.join("pwned.md").exists());
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn project_discovery_can_be_turned_off() {
        let sandbox = tmpdir("projects");
        let repo = sandbox.join("app");
        populated_root(&repo);
        let project: crate::workspace::state::ProjectData =
            serde_json::from_value(serde_json::json!({
                "id": "p1", "name": "app", "path": repo.to_string_lossy(),
            }))
            .unwrap();
        let mut settings = sandboxed(&sandbox);
        let found = resolve_root(std::slice::from_ref(&project), &settings, None).unwrap();
        assert_eq!(found.kind, SpecRootKind::Project);

        settings.active_space_mut().library.spec.projects = false;
        assert!(resolve_root(std::slice::from_ref(&project), &settings, None).is_err());
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn scaffolding_writes_what_openspec_new_change_writes_and_refuses_to_overwrite() {
        let sandbox = tmpdir("scaffold");
        let repo = sandbox.join("specs");
        populated_root(&repo);
        let mut settings = sandboxed(&sandbox);
        settings.active_space_mut().library.spec.folders = vec![repo.to_string_lossy().into_owned()];
        let root = resolve_root(&[], &settings, None).unwrap();

        let dir = scaffold_change(&root, "add-sso", "Add SSO").unwrap();
        let meta = std::fs::read_to_string(dir.join(".openspec.yaml")).unwrap();
        assert!(meta.starts_with("schema: spec-driven\ncreated: "), "{meta}");
        assert!(dir.join("proposal.md").is_file());
        assert!(scaffold_change(&root, "add-sso", "again").is_err());
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn dates_are_zero_padded_like_openspec_writes_them() {
        let date = time::Date::from_calendar_date(2026, time::Month::September, 1).unwrap();
        assert_eq!(date_string(date), "2026-09-01");
    }

    #[test]
    fn copilot_and_claude_take_prompts_differently() {
        assert_eq!(prompt_args("claude", "hi"), ["hi"]);
        assert_eq!(
            prompt_args("/usr/local/bin/copilot", "hi"),
            ["--prompt", "hi"]
        );
        // An unknown agent gets the positional form rather than nothing, so the
        // prompt is never silently dropped.
        assert_eq!(prompt_args("aider", "hi"), ["hi"]);
    }

    #[test]
    fn the_stub_keeps_the_idea_verbatim() {
        let s = proposal_stub("Add login with Google");
        assert!(s.contains("Add login with Google"));
    }

    fn root_of(kind: SpecRootKind, store_id: Option<&str>) -> okena_core::specs::SpecRoot {
        okena_core::specs::SpecRoot {
            key: "k".into(),
            kind,
            name: "n".into(),
            path: "/r".into(),
            store_id: store_id.map(str::to_string),
            remote: None,
            schema: None,
            healthy: true,
            is_default: false,
            git: None,
            references: vec![okena_core::specs::SpecReference {
                id: "design-system".into(),
                remote: None,
                root: Some("/stores/design-system".into()),
                status: Vec::new(),
            }],
            used_by: Vec::new(),
            status: Vec::new(),
        }
    }

    #[test]
    fn the_brief_names_the_directory_and_the_artifacts() {
        let b = brief(
            "Add login",
            "add-login",
            "openspec/changes/add-login",
            &root_of(SpecRootKind::Folder, None),
            &[],
            false,
            &Vec::new(),
        );
        assert!(b.contains("openspec/changes/add-login"));
        for f in okena_core::specs::CHANGE_ARTIFACTS {
            assert!(b.contains(f), "brief never mentions {f}");
        }
        // A folder cannot be selected with --store; only its references can.
        assert!(!b.contains("--change add-login --store"), "{b}");
    }

    #[test]
    fn a_store_brief_targets_the_store_and_lists_its_references() {
        // Without `--store`, an agent's CLI calls resolve against its working
        // directory — not necessarily this store.
        let b = brief(
            "Add login",
            "add-login",
            "openspec/changes/add-login",
            &root_of(SpecRootKind::Store, Some("team-plans")),
            &[],
            false,
            &Vec::new(),
        );
        assert!(b.contains("--change add-login --store team-plans"));
        assert!(b.contains("`design-system` at `/stores/design-system`"));
    }

    #[test]
    fn the_builtin_template_says_exactly_what_the_hardcoded_brief_said() {
        // The point of the switch is that it changes nothing for somebody with
        // no templates of their own. This is the whole text, so a reworded
        // default is a deliberate edit rather than a silent drift.
        let b = brief(
            "Add login",
            "add-login",
            "openspec/changes/add-login",
            &root_of(SpecRootKind::Folder, None),
            &[],
            false,
            &Vec::new(),
        );
        assert_eq!(
            b,
            "Draft an OpenSpec change for this idea: Add login\n\n\
             The change directory already exists at `openspec/changes/add-login` with its \
             `.openspec.yaml` and a stub `proposal.md` holding the idea. Work only \
             inside that directory.\n\n\
             Follow OpenSpec conventions (https://github.com/Fission-AI/OpenSpec):\n\
             - `proposal.md` — why this change, and what changes.\n\
             - `design.md` — the technical approach, when the change needs one.\n\
             - `tasks.md` — an implementation checklist.\n\
             - `specs/<capability>/spec.md` — delta specs for the requirements this \
             change adds, modifies or removes.\n\n\
             Read the existing `openspec/specs/` before proposing. Prefer plain \
             Markdown and keep it short. Ask me about anything ambiguous rather \
             than inventing requirements.\n\n\
             If the `openspec` CLI is installed you may use it; do not install \
             it if it is not.\n\n\
             Referenced stores — read-only upstream context. Fetch what you \
             need and cite what you use:\n\
             - `design-system` at `/stores/design-system` \
             (e.g. `openspec show <spec-id> --type spec --store design-system`)\n\n"
                .to_string()
                // No context was picked, so no context block — only the line
                // saying how to look it up, which every brief carries.
                + &okena_knowledge::prompts::defaults::partial_body("context-lookup")
                    .expect("context-lookup")
                + "\n\n"
                + &okena_knowledge::prompts::defaults::partial_body("reporting")
                    .expect("reporting")
        );
    }

    // ---- searching every root (QBL-436) ----

    /// Two folder roots: the usual tree, and one with a single capability.
    fn two_roots(sandbox: &Path) -> (AppSettings, String, String) {
        let plans = sandbox.join("plans");
        populated_root(&plans);
        write(
            &plans.join("openspec/changes/add-login/design.md"),
            "# Design\nSign in with Google through OAuth.\n",
        );
        let billing = sandbox.join("billing");
        write(&billing.join("openspec/config.yaml"), "schema: spec-driven\n");
        write(
            &billing.join("openspec/specs/invoices/spec.md"),
            "# Invoices\nAn invoice is immutable once sent.\n",
        );
        let mut settings = sandboxed(sandbox);
        settings.active_space_mut().library.spec.folders = vec![
            plans.to_string_lossy().into_owned(),
            billing.to_string_lossy().into_owned(),
        ];
        let stores = super::listing(&super::spec_sources(&[], &settings), &settings);
        // By folder name: discovery reports the path as the filesystem
        // resolves it, which is not how a temp dir is spelled on every OS.
        let key = |folder: &str| {
            stores
                .roots
                .iter()
                .find(|r| Path::new(&r.path).ends_with(folder))
                .map(|r| r.key.clone())
                .expect("the folder is a root")
        };
        let (plans, billing) = (key("plans"), key("billing"));
        (settings, plans, billing)
    }

    fn search(settings: &AppSettings, query: &str, roots: &[&str]) -> LibrarySearchResult {
        let filter = LibrarySearchFilter {
            query: query.to_string(),
            roots: roots.iter().map(|r| (*r).to_string()).collect(),
            ..Default::default()
        };
        let mut result = LibrarySearchResult::default();
        super::search(&src(settings), settings, &filter, &mut result);
        result
    }

    fn labels(result: &LibrarySearchResult) -> Vec<&str> {
        result.hits.iter().map(|h| h.label.as_str()).collect()
    }

    #[test]
    fn a_spec_search_covers_every_root_by_name_path_and_content() {
        let sandbox = tmpdir("search");
        let (settings, _, _) = two_roots(&sandbox);

        // Nothing narrowing: every document of both roots, in sidebar order.
        let everything = search(&settings, "", &[]);
        assert_eq!(everything.total, 6);
        assert_eq!(
            labels(&everything),
            [
                "add-login/proposal.md",
                "add-login/design.md",
                "add-login/login",
                "auth",
                "archive/2026-01-01-old-thing/proposal.md",
                "invoices",
            ]
        );

        // A word only the text has, whatever its case or padding.
        assert_eq!(labels(&search(&settings, " OAUTH ", &[])), ["add-login/design.md"]);
        assert_eq!(labels(&search(&settings, "immutable", &[])), ["invoices"]);
        // A file name, and a directory through the path.
        assert_eq!(
            labels(&search(&settings, "proposal", &[])),
            ["add-login/proposal.md", "archive/2026-01-01-old-thing/proposal.md"]
        );
        assert_eq!(
            labels(&search(&settings, "changes/archive", &[])),
            ["archive/2026-01-01-old-thing/proposal.md"]
        );
        assert_eq!(search(&settings, "proposal", &[]).total, 6);
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn a_spec_root_narrows_a_search_and_text_narrows_within_it() {
        let sandbox = tmpdir("search-root");
        let (settings, plans, billing) = two_roots(&sandbox);

        let only_billing = search(&settings, "", &[&billing]);
        assert_eq!(labels(&only_billing), ["invoices"]);
        assert!(only_billing.hits.iter().all(|h| h.root_key == billing));

        // "spec" is in a path of both roots; the root choice keeps one.
        assert_eq!(search(&settings, "spec.md", &[]).hits.len(), 3);
        assert_eq!(labels(&search(&settings, "spec.md", &[&plans])), ["add-login/login", "auth"]);
        // Two roots widen back to both.
        assert_eq!(search(&settings, "spec.md", &[&plans, &billing]).hits.len(), 3);

        // A key discovery never produced names nothing.
        let outside = format!("spec:path:{}", sandbox.to_string_lossy());
        assert!(search(&settings, "", &[&outside]).hits.is_empty());
        std::fs::remove_dir_all(&sandbox).ok();
    }
}
