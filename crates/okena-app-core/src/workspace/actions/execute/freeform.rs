//! Freeform Library origins: any folder of markdown (QBL-440).
//!
//! The third origin type, beside knowledge stores and OpenSpec roots. It
//! promises nothing about what is inside — no layout, no identity file, no
//! registry — so an origin is exactly a folder the space's settings name
//! (`library.freeform.folders`), and everything here is plain files: list the
//! markdown, read and write a file, and the git of the checkout it sits in.
//! There is no layering, so nothing here ever overrides anything.
//!
//! An agent can be started in one all the same ([`draft`]): with no layout to
//! brief it on, it is told to follow what the folder already does.
//!
//! Reads are scoped to the folders discovery found and path-checked against
//! them, exactly as the other two types' are: these actions are reachable by
//! any paired client and by agents through okena's MCP server.

use super::ActionResult;
use super::library::SettingsEdit;
use crate::workspace::persistence::AppSettings;
use okena_core::api::ActionRequest;
use okena_core::diagnostic::Diagnostic;
use okena_core::doc_search::{LibrarySearchFilter, SearchDoc, library_matches};
use okena_core::freeform::{read_tree, resolve_document};
use okena_core::library::{
    LibraryDocument, LibraryHit, LibraryOrigin, LibrarySearchResult, LibraryTree, OriginType,
};
use okena_git::repository as git;
use std::path::{Path, PathBuf};

/// Largest file a read returns, the same bound the other two types use.
const MAX_DOC_BYTES: u64 = 2 * 1024 * 1024;

/// What a new freeform origin starts with, so the folder is not empty and the
/// first commit has something in it.
const README: &str = "README.md";

fn failed(message: impl Into<String>) -> ActionResult {
    ActionResult::Err(message.into())
}

fn to_result(value: serde_json::Result<serde_json::Value>, what: &str) -> ActionResult {
    match value {
        Ok(v) => ActionResult::Ok(Some(v)),
        Err(e) => failed(format!("could not serialize {what}: {e}")),
    }
}

/// Every freeform origin the space lists, in the order it lists them.
///
/// A folder that is gone is still an origin — it is still in the settings,
/// and the list is where its problem has to show — so it is listed unhealthy
/// rather than dropped. A folder named twice is listed once.
pub(super) fn discover(folders: &[String]) -> Vec<LibraryOrigin> {
    let mut origins: Vec<LibraryOrigin> = Vec::new();
    for folder in folders {
        let expanded = okena_core::fs::expand_home(folder);
        let path = okena_core::fs::canonical(&expanded);
        let shown = path.to_string_lossy().into_owned();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| shown.clone());
        let mut origin = LibraryOrigin::freeform(name, shown.clone());
        if origins.iter().any(|o| o.key == origin.key) {
            continue;
        }
        if !path.is_dir() {
            origin.healthy = false;
            origin.status.push(
                Diagnostic::error(
                    "freeform_folder_missing",
                    format!("{shown} is not a folder on this machine."),
                )
                .with_fix("Restore the folder, or remove this origin."),
            );
        }
        origins.push(origin);
    }
    origins
}

/// [`discover`], with what a listing adds: how many documents each origin
/// holds, and its sync state when it is the top of a git checkout.
pub(super) fn listing(folders: &[String]) -> Vec<LibraryOrigin> {
    let mut origins = discover(folders);
    for origin in origins.iter_mut().filter(|o| o.healthy) {
        let dir = Path::new(&origin.path);
        let tree = read_tree(dir);
        origin.documents = Some(tree.documents.len() as u32);
        origin.status.extend(tree.status);
        origin.git = okena_git::store::status(dir);
        origin.remote = git::is_repository_at_root(dir)
            .then(|| git::origin_url(dir))
            .flatten();
    }
    origins
}

/// A usable origin the client named, checked against what discovery found —
/// never a path taken on trust.
pub(super) fn resolve_root(folders: &[String], key: &str) -> Result<LibraryOrigin, String> {
    let origin = discover(folders)
        .into_iter()
        .find(|o| o.key == key)
        .ok_or_else(|| {
            format!("unknown origin `{key}` — it is no longer listed; refresh the Library")
        })?;
    if !origin.healthy {
        let why = origin
            .status
            .first()
            .map(|d| d.message.clone())
            .unwrap_or_else(|| "it is not a usable folder".into());
        return Err(format!("can't open `{}`: {why}", origin.name));
    }
    Ok(origin)
}

/// The usable origin `key` names among the active space's freeform folders.
pub(super) fn resolve_listed(settings: &AppSettings, key: &str) -> Result<LibraryOrigin, String> {
    resolve_root(&settings.active_space().freeform_folders(), key)
}

// ─── Drafting with an agent ─────────────────────────────────────────────────

/// Brief an agent to write or update documents in a freeform origin.
///
/// The prose is the `freeform-draft` template. There is no layout to describe
/// and nobody to decide who commits for: the agent never does.
fn draft_brief(
    request: &str,
    origin: &LibraryOrigin,
    context_items: &[okena_core::context::ContextItem],
    loaded: bool,
    prompts: &super::briefs::PromptRoots,
) -> String {
    use okena_knowledge::prompts::{Flow, Vars};
    let mut vars = Vars::new();
    vars.insert(
        "context",
        super::briefs::context_block(context_items, loaded, prompts),
    );
    vars.insert("request", request.to_string());
    vars.insert("path", origin.path.clone());
    super::briefs::build(Flow::FreeformDraft, prompts, &vars)
        .rendered
        .text
}

/// Open an agent session in a freeform origin, briefed to write there.
///
/// Runs on the workspace path, unlike every other freeform action: it creates
/// a session project.
#[allow(clippy::too_many_arguments)]
pub(super) fn draft(
    ws: &mut crate::workspace::state::Workspace,
    window_id: crate::workspace::state::WindowId,
    root: &str,
    request: String,
    agent_command: Option<String>,
    model: Option<String>,
    context_refs: Vec<okena_core::context::ContextRef>,
    backend: &dyn okena_terminal::backend::TerminalBackend,
    terminals: &okena_terminal::TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl okena_workspace::context::WorkspaceCx,
) -> ActionResult {
    use okena_knowledge::prompts::Flow;
    let request = request.trim().to_string();
    if request.is_empty() {
        return failed("say what to write first");
    }
    // Checked as a read checks it: a folder that is gone has nowhere for the
    // session to run.
    let origin = match resolve_listed(settings, root) {
        Ok(o) => o,
        Err(e) => return failed(e),
    };
    let context_items =
        super::context::resolve_for_launch(&ws.data.projects, settings, &context_refs);
    let command = super::agent_context::launch_command(settings, agent_command.as_deref());
    let install = super::agent_context::install(&command, &context_items);
    let prompts = super::briefs::prompt_roots(&ws.data.projects, settings);
    // Nothing is scaffolded, so a session without an agent would do nothing.
    let Some(shell) = super::specs::spec_agent_shell(
        settings,
        agent_command.as_deref(),
        &draft_brief(&request, &origin, &context_items, install.loaded(), &prompts),
        &install,
        &super::briefs::launch_model(Flow::FreeformDraft, &prompts, model),
    ) else {
        return failed(
            "no agent to start — pick one, or set the agent command in Settings → Harness",
        );
    };
    let name = origin.name.clone();
    let project_id = match ws.add_project(
        name.clone(),
        origin.path.clone(),
        true,
        &settings.hooks,
        window_id,
        cx,
    ) {
        Ok(id) => id,
        Err(e) => {
            return failed(format!("could not open a session in `{}`: {e}", origin.name));
        }
    };
    // Marked and given its agent before the terminal spawns: the terminal
    // reads the project's shell as it starts, and the markers keep the session
    // out of discovery from the first snapshot.
    if let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == project_id) {
        p.custom_session = Some(format!("Documents: {request}"));
        // The origin it writes into, so the Library can list it beside the
        // documents rather than matching on the goal text.
        p.agent_purpose = Some(okena_core::harness::AgentPurpose::FreeformDraft {
            root: origin.key.clone(),
        });
        p.context_projects = super::context::scope_projects(&[], &context_items);
        p.default_shell = Some(shell);
    }
    if let ActionResult::Err(e) =
        super::spawn_session_terminals(ws, &project_id, backend, terminals, settings, cx)
    {
        log::warn!("[library] freeform draft session terminal failed to spawn: {e}");
    }
    ws.notify_data(cx);
    ActionResult::Ok(Some(serde_json::json!({
        "root": origin.key,
        "project_id": project_id,
        "name": name,
    })))
}

/// A listed document's text for searching: empty when it cannot be read as
/// text or is past what a read would show, so it can still match by name.
fn searchable_text(root: &Path, path: &str) -> String {
    let Ok(real) = resolve_document(root, path) else {
        return String::new();
    };
    if real.metadata().is_ok_and(|m| m.len() > MAX_DOC_BYTES) {
        return String::new();
    }
    std::fs::read_to_string(&real).unwrap_or_default()
}

/// Add every freeform document `filter` keeps to `result`.
pub(super) fn search(
    folders: &[String],
    filter: &LibrarySearchFilter,
    result: &mut LibrarySearchResult,
) {
    let groups = filter.without_text();
    for origin in discover(folders).iter().filter(|o| o.healthy) {
        let dir = Path::new(&origin.path);
        for d in read_tree(dir).documents {
            result.total += 1;
            let names = [d.title.as_str()];
            let paths = [d.path.as_str()];
            let doc = |content| SearchDoc {
                root_key: &origin.key,
                names: &names,
                paths: &paths,
                content,
            };
            let matches = |f: &LibrarySearchFilter, content| {
                library_matches(f, OriginType::Freeform, None, &doc(content))
            };
            // Names and paths first, and a document its origin or type has
            // already ruled out is never opened.
            let hit = matches(filter, "")
                || (matches(&groups, "") && matches(filter, &searchable_text(dir, &d.path)));
            if hit {
                result.hits.push(LibraryHit {
                    root_key: origin.key.clone(),
                    origin_type: OriginType::Freeform,
                    path: d.path,
                    label: d.title,
                    facet: None,
                });
            }
        }
    }
}

/// Why a freeform origin refuses something only a knowledge origin does.
pub(super) fn no_layers(origin_type: OriginType) -> String {
    format!(
        "{} origins do not layer, so nothing in one overrides anything — only knowledge origins take overrides",
        origin_type.label()
    )
}

fn in_root(
    folders: &[String],
    key: &str,
    op: impl FnOnce(&LibraryOrigin, &Path) -> ActionResult,
) -> ActionResult {
    match resolve_root(folders, key) {
        Ok(origin) => {
            let dir = PathBuf::from(&origin.path);
            op(&origin, &dir)
        }
        Err(e) => failed(e),
    }
}

/// Run `op` — fetch, pull, commit or push — in an origin's checkout.
fn sync(
    folders: &[String],
    key: &str,
    op: impl FnOnce(
        &Path,
    )
        -> Result<okena_core::store_git::StoreGitStatus, okena_git::store::StoreGitError>,
) -> ActionResult {
    in_root(folders, key, |origin, dir| {
        if !git::is_repository_at_root(dir) {
            return failed(format!(
                "`{}` is not the top of a git checkout, so there is nothing to sync",
                origin.name
            ));
        }
        match op(dir) {
            Ok(status) => to_result(serde_json::to_value(status), "sync state"),
            Err(e) => failed(e.to_string()),
        }
    })
}

/// What adding an origin replies with, and the settings change that makes it
/// one: a freeform origin is a line in the space's settings and nothing else.
fn added(settings: &AppSettings, root: &Path, extra: serde_json::Value) -> Outcome {
    let folder = root.to_string_lossy().into_owned();
    let mut reply = serde_json::json!({
        "type": OriginType::Freeform.slug(),
        "root": folder,
        "key": LibraryOrigin::freeform("", folder.clone()).key,
        "already_registered": settings
            .active_space()
            .freeform_folders()
            .iter()
            .any(|f| okena_core::fs::canonical(&okena_core::fs::expand_home(f)) == root),
    });
    if let (Some(reply), Some(extra)) = (reply.as_object_mut(), extra.as_object()) {
        reply.extend(extra.clone());
    }
    (
        ActionResult::Ok(Some(reply)),
        Some(SettingsEdit::AddFreeformFolder {
            space: settings.active_space.clone(),
            folder,
        }),
    )
}

/// A result, and the settings change that goes with it when there is one.
pub(super) type Outcome = (ActionResult, Option<SettingsEdit>);

fn plain(result: ActionResult) -> Outcome {
    (result, None)
}

/// Clone a repository to be a freeform origin.
fn clone(settings: &AppSettings, url: &str, dest: Option<&str>) -> Outcome {
    let clone_dir = settings.active_space().library.freeform.clone_dir();
    match super::library::clone_checkout(url, dest, &clone_dir, "origin") {
        Ok(target) => added(
            settings,
            &okena_core::fs::canonical(&target),
            serde_json::json!({}),
        ),
        Err(e) => plain(failed(e)),
    }
}

/// Add a folder that already exists.
fn register(settings: &AppSettings, path: &str) -> Outcome {
    let path = path.trim();
    if path.is_empty() {
        return plain(failed("Enter the folder to add."));
    }
    let dir = okena_core::fs::expand_home(path);
    if !dir.is_dir() {
        return plain(failed(format!(
            "{} is not a folder — choose one that exists, or create a new origin instead.",
            dir.display()
        )));
    }
    added(
        settings,
        &okena_core::fs::canonical(&dir),
        serde_json::json!({}),
    )
}

/// Create a new folder to be a freeform origin: a README, and one commit when
/// asked.
fn setup(settings: &AppSettings, title: &str, path: &str, init_git: bool) -> Outcome {
    let path = path.trim();
    if path.is_empty() {
        return plain(failed("Enter the folder to create."));
    }
    let root = okena_core::fs::expand_home(path);
    let at = root.display().to_string();
    let existed = root.exists();
    if existed && !root.is_dir() {
        return plain(failed(format!("{at} is a file.")));
    }
    let empty = |dir: &Path| {
        std::fs::read_dir(dir).is_ok_and(|entries| entries.flatten().all(|e| e.file_name() == ".git"))
    };
    if existed && !empty(&root) {
        return plain(failed(format!(
            "{at} is not empty — to use a folder that already holds documents, add it as an existing folder."
        )));
    }
    if init_git {
        let probe = root
            .ancestors()
            .find(|p| p.is_dir())
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir);
        if !git::has_commit_identity(&probe) {
            return plain(failed(
                "git has no author name and email configured, so the initial commit can't be made — run `git config --global user.name \"Your Name\"` and `git config --global user.email you@example.com`, or create the origin without git.",
            ));
        }
    }
    if let Err(e) = std::fs::create_dir_all(&root) {
        return plain(failed(format!("could not create {at}: {e}")));
    }
    let title = Some(title.trim())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .or_else(|| root.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "Notes".to_string());
    if let Err(e) = std::fs::write(root.join(README), format!("# {title}\n")) {
        return plain(failed(format!("could not write {README} in {at}: {e}")));
    }
    let (mut git_initialized, mut committed) = (false, false);
    if init_git {
        if !git::is_repository_at_root(&root) {
            if let Err(e) = git::init_repository(&root) {
                return plain(failed(format!(
                    "Created {at}, but git could not be initialized there: {}",
                    e.user_detail()
                )));
            }
            git_initialized = true;
        }
        if let Err(e) = git::commit_paths(&root, &format!("Initialize {title}"), &[README]) {
            return plain(failed(format!(
                "Created {at}, but the initial commit failed: {}",
                e.user_detail()
            )));
        }
        committed = true;
    }
    added(
        settings,
        &okena_core::fs::canonical(&root),
        serde_json::json!({
            "created": !existed,
            "git_initialized": git_initialized,
            "committed": committed,
        }),
    )
}

/// Run a Library action against a freeform origin; `None` for an action this
/// type has nothing to do with. `key` is the origin the action names, when it
/// names one.
pub(super) fn execute(
    action: &ActionRequest,
    folders: &[String],
    settings: &AppSettings,
) -> Option<Outcome> {
    Some(match action {
        ActionRequest::LibraryTree { root: Some(root) } => plain(in_root(folders, root, |origin, dir| {
            let mut tree = read_tree(dir);
            tree.root_key = origin.key.clone();
            to_result(
                serde_json::to_value(LibraryTree::Freeform(tree)),
                "freeform tree",
            )
        })),
        ActionRequest::LibraryRead {
            root: Some(root),
            path,
        } => plain(in_root(folders, root, |origin, dir| {
            let real = match resolve_document(dir, path) {
                Ok(p) => p,
                Err(e) => return failed(e),
            };
            if let Ok(meta) = real.metadata()
                && meta.len() > MAX_DOC_BYTES
            {
                return failed(format!(
                    "{path} is too large to display ({} KB)",
                    meta.len() / 1024
                ));
            }
            match std::fs::read_to_string(&real) {
                Ok(content) => to_result(
                    serde_json::to_value(LibraryDocument {
                        root_key: origin.key.clone(),
                        path: path.clone(),
                        revision: okena_core::fs::content_revision(&content),
                        content,
                    }),
                    "document",
                ),
                Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                    failed(format!("{path} is not a text file"))
                }
                Err(e) => failed(format!("could not read {path}: {e}")),
            }
        })),
        ActionRequest::LibraryWrite {
            root: Some(root),
            path,
            content,
            revision,
        } => plain(in_root(folders, root, |origin, dir| {
            let real = match resolve_document(dir, path) {
                Ok(p) => p,
                Err(e) => return failed(e),
            };
            // What could not be opened must not be written either.
            if content.len() as u64 > MAX_DOC_BYTES {
                return failed(format!(
                    "{path} is too large to save ({} KB)",
                    content.len() / 1024
                ));
            }
            match okena_core::fs::replace_if_unchanged(&real, content, revision) {
                Ok(revision) => ActionResult::Ok(Some(serde_json::json!({
                    "root": origin.key,
                    "path": path,
                    "revision": revision,
                }))),
                Err(e) => failed(e.describe(path)),
            }
        })),
        ActionRequest::LibraryFileCreate {
            root: Some(root),
            path,
            content,
        } => plain(in_root(folders, root, |origin, dir| {
            super::document_files::create_file(&origin.key, dir, path, content, MAX_DOC_BYTES)
        })),
        ActionRequest::LibraryFolderCreate {
            root: Some(root),
            path,
        } => plain(in_root(folders, root, |origin, dir| {
            super::document_files::create_folder(&origin.key, dir, path)
        })),
        ActionRequest::LibraryFileRename {
            root: Some(root),
            from,
            to,
        } => plain(in_root(folders, root, |origin, dir| {
            super::document_files::rename(&origin.key, dir, resolve_document, from, to)
        })),
        ActionRequest::LibraryFileDelete {
            root: Some(root),
            path,
        } => plain(in_root(folders, root, |origin, dir| {
            super::document_files::delete(&origin.key, dir, resolve_document, path)
        })),
        ActionRequest::LibraryOverride { .. } => plain(failed(no_layers(OriginType::Freeform))),
        ActionRequest::LibraryStoreClone { url, path, .. } => clone(settings, url, path.as_deref()),
        ActionRequest::LibraryStoreRegister { path, .. } => register(settings, path),
        ActionRequest::LibraryStoreSetup {
            id, path, init_git, ..
        } => setup(settings, id, path, *init_git),
        ActionRequest::LibraryStoreUnregister { root } => {
            // Found among the folders as listed, not through `resolve_root`:
            // a folder that has gone missing is exactly the one to remove.
            match discover(folders).into_iter().find(|o| &o.key == root) {
                Some(origin) => (
                    ActionResult::Ok(Some(serde_json::json!({
                        "id": origin.name,
                        "left_on_disk": origin.path,
                    }))),
                    Some(SettingsEdit::RemoveFreeformFolder {
                        space: settings.active_space.clone(),
                        folder: origin.path,
                    }),
                ),
                None => plain(failed(format!(
                    "unknown origin `{root}` — it is no longer listed; refresh the Library"
                ))),
            }
        }
        ActionRequest::LibraryStoreFetch { root } => plain(sync(folders, root, |path| {
            okena_git::store::fetch(path).map(|()| okena_git::store::status(path).unwrap_or_default())
        })),
        ActionRequest::LibraryStorePull { root } => {
            plain(sync(folders, root, okena_git::store::pull))
        }
        ActionRequest::LibraryStoreCommit {
            root,
            paths,
            message,
        } => plain(sync(folders, root, |path| {
            okena_git::store::commit(path, paths, message)
        })),
        ActionRequest::LibraryStorePush { root } => {
            plain(sync(folders, root, okena_git::store::push))
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "okena-freeform-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // Canonical, so a key built from it is the key discovery builds.
        dir.canonicalize().unwrap()
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn notes() -> PathBuf {
        let root = tmpdir("notes");
        write(&root.join("README.md"), "# Team notes\n\nStart here.");
        write(
            &root.join("workflows/release.md"),
            "---\ntitle: Cutting a release\n---\n\n# Ignored heading\n\nTag, then publish.",
        );
        write(&root.join("decisions/0001-daemon.MD"), "Context first.\n\n# Two processes\n");
        write(&root.join("decisions/untitled.md"), "No heading at all.");
        write(&root.join("scripts/run.sh"), "echo hi");
        write(&root.join(".obsidian/workspace.md"), "# Hidden");
        write(&root.join("node_modules/pkg/README.md"), "# Not ours");
        root
    }

    fn folders(root: &Path) -> Vec<String> {
        vec![root.to_string_lossy().into_owned()]
    }

    /// The Library key of the freeform origin at `root`.
    fn key(root: &Path) -> String {
        LibraryOrigin::freeform("", root.to_string_lossy()).key
    }

    fn ok(outcome: Option<Outcome>) -> serde_json::Value {
        match outcome.expect("a freeform action").0 {
            ActionResult::Ok(Some(v)) => v,
            ActionResult::Ok(None) => serde_json::Value::Null,
            ActionResult::Err(e) => panic!("expected success, got: {e}"),
        }
    }

    fn err(outcome: Option<Outcome>) -> String {
        match outcome.expect("a freeform action").0 {
            ActionResult::Err(e) => e,
            ActionResult::Ok(v) => panic!("expected a refusal, got: {v:?}"),
        }
    }

    #[test]
    fn the_tree_lists_markdown_only_titled_from_frontmatter_heading_or_name() {
        let root = notes();
        let tree = read_tree(&root);
        let listed: Vec<(&str, &str)> = tree
            .documents
            .iter()
            .map(|d| (d.path.as_str(), d.title.as_str()))
            .collect();
        assert_eq!(
            listed,
            [
                ("README.md", "Team notes"),
                // A heading further down still names the document.
                ("decisions/0001-daemon.MD", "Two processes"),
                ("decisions/untitled.md", "untitled"),
                // Frontmatter wins over the body's heading.
                ("workflows/release.md", "Cutting a release"),
            ],
            "no script, nothing hidden, nothing from node_modules"
        );
        assert!(tree.status.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_folder_is_an_origin_under_a_freeform_key_and_a_missing_one_says_so() {
        let root = notes();
        let gone = root.join("gone");
        let listed = listing(&[
            root.to_string_lossy().into_owned(),
            gone.to_string_lossy().into_owned(),
            // The same folder again is one origin, not two.
            root.to_string_lossy().into_owned(),
        ]);
        assert_eq!(listed.len(), 2);
        let origin = &listed[0];
        assert_eq!(origin.key, key(&root));
        assert_eq!(origin.origin_type, OriginType::Freeform);
        assert_eq!(origin.documents, Some(4));
        assert!(origin.healthy && origin.git.is_none(), "not a checkout: no sync state");
        assert!(!origin.takes_overrides(), "freeform origins never take overrides");

        assert!(!listed[1].healthy);
        assert_eq!(listed[1].status[0].code, "freeform_folder_missing");
        let e = resolve_root(&folders(&gone), &key(&gone)).unwrap_err();
        assert!(e.contains("not a folder"), "{e}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_document_is_read_written_created_renamed_and_deleted_inside_the_origin() {
        let root = notes();
        let (folders, key) = (folders(&root), key(&root));
        let settings = AppSettings::default();
        let run = |action: ActionRequest| execute(&action, &folders, &settings);

        let doc: LibraryDocument = serde_json::from_value(ok(run(ActionRequest::LibraryRead {
            root: Some(key.clone()),
            path: "workflows/release.md".into(),
        })))
        .unwrap();
        assert_eq!(doc.root_key, key);
        assert!(doc.content.contains("Tag, then publish."));

        let saved = ok(run(ActionRequest::LibraryWrite {
            root: Some(key.clone()),
            path: "workflows/release.md".into(),
            content: "# Release\n".into(),
            revision: doc.revision.clone(),
        }));
        assert_eq!(saved["root"], key);
        assert_eq!(
            std::fs::read_to_string(root.join("workflows/release.md")).unwrap(),
            "# Release\n"
        );
        // The buffer that was read is stale now, so a second save is refused.
        let stale = err(run(ActionRequest::LibraryWrite {
            root: Some(key.clone()),
            path: "workflows/release.md".into(),
            content: "clobber".into(),
            revision: doc.revision,
        }));
        assert!(stale.contains("changed"), "{stale}");

        let made = ok(run(ActionRequest::LibraryFileCreate {
            root: Some(key.clone()),
            path: "workflows/hotfix.md".into(),
            content: "# Hotfix\n".into(),
        }));
        assert_eq!(made["path"], "workflows/hotfix.md");
        let moved = ok(run(ActionRequest::LibraryFileRename {
            root: Some(key.clone()),
            from: "workflows/hotfix.md".into(),
            to: "runbooks/hotfix.md".into(),
        }));
        assert_eq!(moved["path"], "runbooks/hotfix.md");
        assert!(root.join("runbooks/hotfix.md").is_file());
        ok(run(ActionRequest::LibraryFileDelete {
            root: Some(key.clone()),
            path: "runbooks/hotfix.md".into(),
        }));
        assert!(!root.join("runbooks/hotfix.md").exists());

        let LibraryTree::Freeform(tree) =
            serde_json::from_value(ok(run(ActionRequest::LibraryTree {
                root: Some(key.clone()),
            })))
            .unwrap()
        else {
            panic!("expected a freeform tree");
        };
        assert_eq!(tree.root_key, key);
        assert_eq!(tree.documents.len(), 4);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_path_cannot_leave_the_origin_and_an_unlisted_folder_is_not_an_origin() {
        let root = notes();
        let outside = tmpdir("outside");
        write(&outside.join("secret.md"), "do not read");
        let (folders, key) = (folders(&root), key(&root));
        let settings = AppSettings::default();

        let e = err(execute(
            &ActionRequest::LibraryRead {
                root: Some(key.clone()),
                path: format!("../{}/secret.md", outside.file_name().unwrap().to_string_lossy()),
            },
            &folders,
            &settings,
        ));
        assert!(e.contains("outside the origin"), "{e}");
        // A symlink inside the origin is not a way out either.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.join("secret.md"), root.join("link.md")).unwrap();
            let e = err(execute(
                &ActionRequest::LibraryRead {
                    root: Some(key.clone()),
                    path: "link.md".into(),
                },
                &folders,
                &settings,
            ));
            assert!(e.contains("outside the origin"), "{e}");
            assert!(
                read_tree(&root).documents.iter().all(|d| d.path != "link.md"),
                "and the tree never lists it"
            );
        }
        // A key for a folder the space does not list names nothing.
        let e = err(execute(
            &ActionRequest::LibraryRead {
                root: Some(LibraryOrigin::freeform("", outside.to_string_lossy()).key),
                path: "secret.md".into(),
            },
            &folders,
            &settings,
        ));
        assert!(e.contains("unknown origin"), "{e}");
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn a_freeform_origin_takes_no_override() {
        let root = notes();
        let e = err(execute(
            &ActionRequest::LibraryOverride {
                root: key(&root),
                path: "templates/briefs/task-start.md".into(),
            },
            &folders(&root),
            &AppSettings::default(),
        ));
        assert!(e.contains("do not layer"), "{e}");
        assert!(
            !root.join("templates").exists(),
            "nothing was copied into the folder"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_search_finds_by_title_path_and_content_and_counts_what_it_read() {
        let root = notes();
        let found = |query: &str| {
            let mut result = LibrarySearchResult::default();
            search(
                &folders(&root),
                &LibrarySearchFilter {
                    query: query.into(),
                    ..Default::default()
                },
                &mut result,
            );
            let paths: Vec<String> = result.hits.iter().map(|h| h.path.clone()).collect();
            (paths, result.total)
        };
        assert_eq!(found("cutting a release"), (vec!["workflows/release.md".to_string()], 4));
        assert_eq!(
            found("decisions/").0,
            ["decisions/0001-daemon.MD", "decisions/untitled.md"]
        );
        assert_eq!(found("then publish").0, ["workflows/release.md"], "by content");
        assert!(found("kubernetes").0.is_empty());

        // A kind is a knowledge thing: choosing one leaves freeform files out.
        let mut result = LibrarySearchResult::default();
        search(
            &folders(&root),
            &LibrarySearchFilter {
                kinds: vec![okena_core::doc_search::KnowledgeFacet::Doc],
                ..Default::default()
            },
            &mut result,
        );
        assert!(result.hits.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn adding_a_folder_asks_for_it_to_be_listed_and_removing_it_to_be_dropped() {
        let root = notes();
        let settings = AppSettings::default();
        let (result, edit) = execute(
            &ActionRequest::LibraryStoreRegister {
                origin_type: OriginType::Freeform,
                path: root.to_string_lossy().into_owned(),
                id: None,
            },
            &[],
            &settings,
        )
        .expect("a freeform action");
        let ActionResult::Ok(Some(reply)) = result else {
            panic!("expected the folder to be accepted");
        };
        assert_eq!(reply["key"], key(&root));
        assert_eq!(reply["already_registered"], false);
        assert_eq!(
            edit,
            Some(SettingsEdit::AddFreeformFolder {
                space: "default".into(),
                folder: root.to_string_lossy().into_owned(),
            })
        );

        // A folder that is not there is refused, and nothing is listed.
        let (result, edit) = execute(
            &ActionRequest::LibraryStoreRegister {
                origin_type: OriginType::Freeform,
                path: root.join("nope").to_string_lossy().into_owned(),
                id: None,
            },
            &[],
            &settings,
        )
        .expect("a freeform action");
        assert!(matches!(result, ActionResult::Err(_)) && edit.is_none());

        let (result, edit) = execute(
            &ActionRequest::LibraryStoreUnregister { root: key(&root) },
            &folders(&root),
            &settings,
        )
        .expect("a freeform action");
        let ActionResult::Ok(Some(reply)) = result else {
            panic!("expected the origin to be removed");
        };
        assert_eq!(reply["left_on_disk"], root.to_string_lossy().as_ref());
        assert_eq!(
            edit,
            Some(SettingsEdit::RemoveFreeformFolder {
                space: "default".into(),
                folder: root.to_string_lossy().into_owned(),
            })
        );
        assert!(root.join("README.md").is_file(), "removing never deletes the folder");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_new_origin_is_a_folder_with_a_readme_and_refuses_one_that_holds_files() {
        let sandbox = tmpdir("setup");
        let settings = AppSettings::default();
        let target = sandbox.join("runbooks");
        let (result, edit) = execute(
            &ActionRequest::LibraryStoreSetup {
                origin_type: OriginType::Freeform,
                id: "Runbooks".into(),
                path: target.to_string_lossy().into_owned(),
                name: None,
                description: None,
                remote: None,
                init_git: false,
            },
            &[],
            &settings,
        )
        .expect("a freeform action");
        let ActionResult::Ok(Some(reply)) = result else {
            panic!("expected the origin to be created");
        };
        assert_eq!(reply["created"], true);
        assert_eq!(reply["committed"], false);
        assert_eq!(
            std::fs::read_to_string(target.join("README.md")).unwrap(),
            "# Runbooks\n"
        );
        assert!(matches!(edit, Some(SettingsEdit::AddFreeformFolder { .. })));

        // Setting up over it would bury what is there.
        let e = err(execute(
            &ActionRequest::LibraryStoreSetup {
                origin_type: OriginType::Freeform,
                id: "Runbooks".into(),
                path: target.to_string_lossy().into_owned(),
                name: None,
                description: None,
                remote: None,
                init_git: false,
            },
            &[],
            &settings,
        ));
        assert!(e.contains("not empty"), "{e}");
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn the_draft_brief_names_the_folder_and_the_request_and_no_layout() {
        let root = notes();
        let origin = resolve_root(&folders(&root), &key(&root)).unwrap();
        let brief = draft_brief("write up the release workflow", &origin, &[], false, &Vec::new());
        for needle in [
            "write up the release workflow",
            origin.path.as_str(),
            "no fixed layout",
            "Do not commit",
            "okena_report_status",
        ] {
            assert!(brief.contains(needle), "missing {needle:?}:\n{brief}");
        }
        // A knowledge origin's layout is not this one's.
        assert!(!brief.contains("skills/"), "{brief}");
        for var in ["{request}", "{path}", "{context}"] {
            assert!(!brief.contains(var), "unfilled {var}:\n{brief}");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_session_is_only_started_in_a_listed_folder_that_is_there() {
        let root = notes();
        let mut settings = AppSettings::default();
        settings.ensure_spaces();
        let listed = key(&root);
        assert!(resolve_listed(&settings, &listed).unwrap_err().contains("unknown origin"));
        SettingsEdit::AddFreeformFolder {
            space: settings.active_space().id.clone(),
            folder: root.display().to_string(),
        }
        .apply(&mut settings)
        .unwrap();
        assert_eq!(resolve_listed(&settings, &listed).unwrap().key, listed);
        std::fs::remove_dir_all(&root).ok();
        assert!(resolve_listed(&settings, &listed).unwrap_err().contains("can't open"));
    }
}
