//! Engineering-harness knowledge actions, over `okena-knowledge` (ADR-0003).
//!
//! Reads are scoped to roots discovery found and path-checked against them:
//! these actions are reachable by any paired client and by agents through
//! okena's MCP server, so they must not become a way to read arbitrary files.
//!
//! None of them touches the workspace, and all of them may run git, so the
//! daemon runs them off its lock with the project list copied out by
//! [`knowledge_project_sources`].

use super::ActionResult;
use super::briefs::{self, PromptRoot};
use crate::workspace::persistence::{AppSettings, get_config_dir};
use crate::workspace::state::ProjectData;
use okena_core::api::ActionRequest;
use okena_core::knowledge::{KnowledgeDocument, KnowledgeRoot, KnowledgeRootKind, KnowledgeStores};
use okena_knowledge::discover::{self, ProjectSource, Sources};
use okena_knowledge::prompts::{self, Flow, Vars};
use okena_knowledge::registry::{self, RegisterOutcome};
use okena_knowledge::setup::{self, SetupRequest};
use okena_knowledge::{KnowledgeError, git, tree};
use std::path::Path;

/// Largest file `KnowledgeRead` returns. Knowledge is prose; past this it is
/// not, and streaming it through a JSON reply would stall the client.
const MAX_DOC_BYTES: u64 = 2 * 1024 * 1024;

/// The projects to look for knowledge in.
///
/// None when project discovery is off, and never a worktree (a second checkout
/// of a repo already listed) or an agent session (rooted above repos, not in
/// one).
pub fn knowledge_project_sources(
    projects: &[ProjectData],
    settings: &AppSettings,
) -> Vec<ProjectSource> {
    if !settings.harness.knowledge.projects {
        return Vec::new();
    }
    projects
        .iter()
        .filter(|p| p.worktree_info.is_none() && !p.is_any_agent_session())
        .map(|p| ProjectSource {
            name: p.name.clone(),
            path: okena_core::fs::expand_home(&p.path),
        })
        .collect()
}

/// Run a knowledge action; `None` for any other action.
pub fn execute_knowledge_action(
    action: &ActionRequest,
    projects: &[ProjectSource],
    settings: &AppSettings,
) -> Option<ActionResult> {
    let registry = registry::registry_path(&get_config_dir());
    ensure_defaults(&registry);
    execute_at(&registry, action, projects, settings)
}

/// Put okena's own briefs on disk and in the registry, once per run.
///
/// Here rather than at startup because this is the first moment anything cares
/// that knowledge exists, and a user who never opens the Knowledge view should
/// not have folders appear for it. Failures are swallowed on purpose: a
/// read-only config directory should cost you the ability to *read* the
/// defaults, not the ability to use knowledge at all — launches still fall
/// back to the same templates compiled in.
fn ensure_defaults(registry: &Path) {
    use std::sync::OnceLock;
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        let dir = get_config_dir()
            .join("knowledge")
            .join(prompts::defaults::DEFAULT_STORE_DIR);
        if prompts::defaults::ensure_store(&dir).is_err() {
            return;
        }
        // Registering is what makes it appear in the Knowledge view. Already
        // registered is the ordinary case and not an error worth reporting.
        let _ = registry::register(registry, &dir.to_string_lossy(), None);
    });
}

fn execute_at(
    registry: &Path,
    action: &ActionRequest,
    projects: &[ProjectSource],
    settings: &AppSettings,
) -> Option<ActionResult> {
    Some(match action {
        ActionRequest::KnowledgeStores => to_result(
            serde_json::to_value(stores(registry, projects)),
            "knowledge stores",
        ),
        ActionRequest::KnowledgeTree { root } => tree_of(registry, projects, root.as_deref()),
        ActionRequest::KnowledgeRead { root, path } => {
            read(registry, projects, root.as_deref(), path)
        }
        ActionRequest::KnowledgeWrite {
            root,
            path,
            content,
            revision,
        } => write(registry, projects, root.as_deref(), path, content, revision),
        ActionRequest::KnowledgeFileCreate {
            root,
            path,
            content,
        } => in_root(registry, projects, root.as_deref(), |key, dir| {
            super::document_files::create_file(key, dir, path, content, MAX_DOC_BYTES)
        }),
        ActionRequest::KnowledgeFolderCreate { root, path } => {
            in_root(registry, projects, root.as_deref(), |key, dir| {
                super::document_files::create_folder(key, dir, path)
            })
        }
        ActionRequest::KnowledgeFileRename { root, from, to } => {
            in_root(registry, projects, root.as_deref(), |key, dir| {
                super::document_files::rename(key, dir, tree::resolve_document, from, to)
            })
        }
        ActionRequest::KnowledgeFileDelete { root, path } => {
            in_root(registry, projects, root.as_deref(), |key, dir| {
                super::document_files::delete(key, dir, tree::resolve_document, path)
            })
        }
        ActionRequest::KnowledgeStoreClone { url, path } => match git::clone_store(
            registry,
            url,
            path.as_deref(),
            &settings.harness.knowledge.clone_dir(),
        ) {
            Ok(out) => registered(out),
            Err(e) => failed(e),
        },
        ActionRequest::KnowledgeStoreRegister { path } => register(registry, path),
        ActionRequest::KnowledgeStoreUnregister { id } => {
            match registry::unregister(registry, id) {
                Ok(left) => ActionResult::Ok(Some(serde_json::json!({
                    "id": id,
                    "left_on_disk": left.to_string_lossy(),
                }))),
                Err(e) => failed(e),
            }
        }
        ActionRequest::KnowledgeStoreSetup {
            id,
            path,
            name,
            description,
            remote,
            init_git,
        } => {
            let request = SetupRequest {
                id: id.clone(),
                path: path.clone(),
                name: name.clone(),
                description: description.clone(),
                remote: remote.clone(),
                init_git: *init_git,
            };
            match setup::setup_store(registry, &request) {
                Ok(out) => ActionResult::Ok(Some(serde_json::json!({
                    "id": out.id,
                    "root": out.root.to_string_lossy(),
                    "git_initialized": out.git_initialized,
                    "committed": out.committed,
                }))),
                Err(e) => failed(e),
            }
        }
        ActionRequest::KnowledgeStoreFetch { root } => sync(registry, projects, root, |path| {
            git::fetch(path).map(|()| git::status(path).unwrap_or_default())
        }),
        ActionRequest::KnowledgeStorePull { root } => sync(registry, projects, root, git::pull),
        ActionRequest::KnowledgeStoreCommit {
            root,
            paths,
            message,
        } => sync(registry, projects, root, |path| {
            git::commit(path, paths, message)
        }),
        ActionRequest::KnowledgeStorePush { root } => sync(registry, projects, root, git::push),
        _ => return None,
    })
}

fn to_result(value: serde_json::Result<serde_json::Value>, what: &str) -> ActionResult {
    match value {
        Ok(v) => ActionResult::Ok(Some(v)),
        Err(e) => ActionResult::Err(format!("could not serialize {what}: {e}")),
    }
}

fn failed(e: KnowledgeError) -> ActionResult {
    ActionResult::Err(e.to_string())
}

fn registered(out: RegisterOutcome) -> ActionResult {
    ActionResult::Ok(Some(serde_json::json!({
        "id": out.id,
        "root": out.root.to_string_lossy(),
        "already_registered": out.already_registered,
        "identity_missing": out.identity_missing,
    })))
}

fn discovered(registry: &Path, projects: &[ProjectSource]) -> KnowledgeStores {
    discover::discover(&Sources {
        registry_path: registry.to_path_buf(),
        projects: projects.to_vec(),
    })
}

/// Every root, with sync state on each store.
fn stores(registry: &Path, projects: &[ProjectSource]) -> KnowledgeStores {
    let mut stores = discovered(registry, projects);
    git::attach_status(&mut stores);
    stores
}

/// A usable root the client named, checked against what discovery found —
/// never a path taken on trust.
fn resolve_root(
    registry: &Path,
    projects: &[ProjectSource],
    key: Option<&str>,
) -> Result<KnowledgeRoot, String> {
    let stores = discovered(registry, projects);
    let root = match key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(key) => stores.root(key).cloned().ok_or_else(|| {
            format!(
                "unknown knowledge root `{key}` — it is no longer discovered; refresh the Knowledge view"
            )
        })?,
        None => stores.default_root().cloned().ok_or_else(|| {
            "no knowledge stores yet — clone or add one in Settings → Knowledge".to_string()
        })?,
    };
    if !root.healthy {
        let why = root
            .status
            .first()
            .map(|d| d.message.clone())
            .unwrap_or_else(|| "it is not a usable knowledge root".into());
        return Err(format!("can't open `{}`: {why}", root.name));
    }
    Ok(root)
}

fn tree_of(registry: &Path, projects: &[ProjectSource], key: Option<&str>) -> ActionResult {
    let root = match resolve_root(registry, projects, key) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    let mut t = tree::read_tree(Path::new(&root.path));
    t.root_key = root.key;
    t.store_id = root.store_id;
    to_result(serde_json::to_value(&t), "knowledge tree")
}

fn read(
    registry: &Path,
    projects: &[ProjectSource],
    key: Option<&str>,
    path: &str,
) -> ActionResult {
    let root = match resolve_root(registry, projects, key) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    let real = match tree::resolve_document(Path::new(&root.path), path) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    if let Ok(meta) = real.metadata()
        && meta.len() > MAX_DOC_BYTES
    {
        return ActionResult::Err(format!(
            "{path} is too large to display ({} KB)",
            meta.len() / 1024
        ));
    }
    match std::fs::read_to_string(&real) {
        Ok(content) => to_result(
            serde_json::to_value(KnowledgeDocument {
                root_key: root.key,
                path: path.to_string(),
                revision: okena_core::fs::content_revision(&content),
                content,
            }),
            "knowledge document",
        ),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            ActionResult::Err(format!("{path} is not a text file"))
        }
        Err(e) => ActionResult::Err(format!("could not read {path}: {e}")),
    }
}

/// Replace an existing file, through the same root and path checks as
/// [`read`]: a write must not be a way out of a root that a read is not.
fn write(
    registry: &Path,
    projects: &[ProjectSource],
    key: Option<&str>,
    path: &str,
    content: &str,
    revision: &str,
) -> ActionResult {
    let root = match resolve_root(registry, projects, key) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    let real = match tree::resolve_document(Path::new(&root.path), path) {
        Ok(p) => p,
        Err(e) => return ActionResult::Err(e),
    };
    // What could not be opened must not be written either.
    if content.len() as u64 > MAX_DOC_BYTES {
        return ActionResult::Err(format!(
            "{path} is too large to save ({} KB)",
            content.len() / 1024
        ));
    }
    match okena_core::fs::replace_if_unchanged(&real, content, revision) {
        Ok(revision) => ActionResult::Ok(Some(serde_json::json!({
            "root": root.key,
            "path": path,
            "revision": revision,
        }))),
        Err(e) => ActionResult::Err(e.describe(path)),
    }
}

/// Run `op` with the key and path of the usable root the client named, found
/// exactly as a read finds it. Creating, renaming and deleting files all go
/// through here, and through the path checks in `document_files`.
///
/// The tree lists entries, not folders, so a folder emptied by a delete or a
/// rename simply stops showing.
fn in_root(
    registry: &Path,
    projects: &[ProjectSource],
    key: Option<&str>,
    op: impl FnOnce(&str, &Path) -> ActionResult,
) -> ActionResult {
    match resolve_root(registry, projects, key) {
        Ok(root) => op(&root.key, Path::new(&root.path)),
        Err(e) => ActionResult::Err(e),
    }
}

fn register(registry: &Path, path: &str) -> ActionResult {
    // Record the checkout's origin, when it is a checkout, for the settings list.
    let remote = registry::resolve_checkout(path)
        .ok()
        .filter(|p| okena_git::repository::is_repository_at_root(p))
        .and_then(|p| okena_git::repository::origin_url(&p));
    match registry::register(registry, path, remote) {
        Ok(out) => registered(out),
        Err(e) => failed(e),
    }
}

/// Run `op` — fetch, pull, commit or push — in a store's checkout. Replies
/// with its sync state after.
fn sync(
    registry: &Path,
    projects: &[ProjectSource],
    key: &str,
    op: impl FnOnce(&Path) -> Result<okena_core::knowledge::KnowledgeGitStatus, KnowledgeError>,
) -> ActionResult {
    let root = match resolve_root(registry, projects, Some(key)) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    if root.kind != KnowledgeRootKind::Store {
        return ActionResult::Err(format!(
            "`{}` is part of a project, not a store; sync it with the project's own git",
            root.name
        ));
    }
    match op(Path::new(&root.path)) {
        Ok(status) => to_result(serde_json::to_value(status), "sync state"),
        Err(e) => failed(e),
    }
}

// ─── Drafting with an agent ─────────────────────────────────────────────────

/// Brief an agent to add to or update a knowledge root.
///
/// The prose is a template now (`knowledge-draft`). What stays here is what a
/// template cannot decide: what kind of root this is, and therefore who is
/// expected to commit.
fn draft_brief(request: &str, root: &KnowledgeRoot, prompts: PromptRoot) -> String {
    let mut vars = Vars::new();
    vars.insert("request", request.to_string());
    vars.insert("path", root.path.clone());
    vars.insert(
        "what",
        match root.kind {
            KnowledgeRootKind::Store => "knowledge store",
            KnowledgeRootKind::Project => "project's knowledge folder",
        }
        .to_string(),
    );
    vars.insert("commit_note", commit_note(root.kind, &prompts));
    briefs::build(Flow::KnowledgeDraft, prompts.as_ref(), &vars)
        .rendered
        .text
}

/// Who commits, which depends on whose repository this is. The decision is
/// here; the words are the `knowledge-commit-*` partials.
fn commit_note(kind: KnowledgeRootKind, prompts: &PromptRoot) -> String {
    let name = match kind {
        KnowledgeRootKind::Store => "knowledge-commit-store",
        KnowledgeRootKind::Project => "knowledge-commit-project",
    };
    briefs::block(&briefs::fragment(name, prompts.as_ref(), &Vars::new()))
}

/// The agent a draft session runs. Unlike a spec draft there is nothing to
/// scaffold, so a session without an agent would have no purpose.
fn draft_shell(
    settings: &AppSettings,
    agent_command: Option<&str>,
    prompt: &str,
) -> Result<okena_terminal::shell_config::ShellType, String> {
    super::specs::spec_agent_shell(settings, agent_command, prompt).ok_or_else(|| {
        "no agent to start — pick one, or set the agent command in Settings → Harness".to_string()
    })
}

/// Open an agent session in a knowledge root, briefed to write there.
///
/// Runs on the workspace path, unlike every other knowledge action: it
/// creates a session project.
#[allow(clippy::too_many_arguments)]
pub(super) fn draft(
    ws: &mut crate::workspace::state::Workspace,
    window_id: crate::workspace::state::WindowId,
    root: Option<String>,
    request: String,
    agent_command: Option<String>,
    backend: &dyn okena_terminal::backend::TerminalBackend,
    terminals: &okena_terminal::TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl okena_workspace::context::WorkspaceCx,
) -> ActionResult {
    let request = request.trim().to_string();
    if request.is_empty() {
        return ActionResult::Err("say what to write first".into());
    }
    let projects = knowledge_project_sources(&ws.data.projects, settings);
    let registry = registry::registry_path(&get_config_dir());
    let root = match resolve_root(&registry, &projects, root.as_deref()) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    let shell = match draft_shell(
        settings,
        agent_command.as_deref(),
        &draft_brief(
            &request,
            &root,
            briefs::prompt_root(&ws.data.projects, settings),
        ),
    ) {
        Ok(s) => s,
        Err(e) => return ActionResult::Err(e),
    };
    let name = format!(
        "{} (knowledge)",
        root.store_id.as_deref().unwrap_or(&root.name)
    );
    let project_id = match ws.add_project(
        name.clone(),
        root.path.clone(),
        true,
        &settings.hooks,
        window_id,
        cx,
    ) {
        Ok(id) => id,
        Err(e) => {
            return ActionResult::Err(format!("could not open a session in `{}`: {e}", root.name));
        }
    };
    // Marked and given its agent before the terminal spawns: the terminal
    // reads the project's shell as it starts, and the marker keeps the session
    // out of knowledge discovery from the first snapshot.
    if let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == project_id) {
        p.custom_session = Some(format!("Knowledge: {request}"));
        // The root it writes into, so the Knowledge view can list it beside
        // the entries rather than matching on the goal text.
        p.knowledge_root = Some(root.key.clone());
        p.default_shell = Some(shell);
    }
    if let ActionResult::Err(e) = super::spawn_uninitialized_terminals(
        ws,
        &project_id,
        backend,
        terminals,
        settings,
        None,
        cx,
    ) {
        log::warn!("[knowledge] draft session terminal failed to spawn: {e}");
    }
    ws.notify_data(cx);
    ActionResult::Ok(Some(serde_json::json!({
        "root": root.key,
        "project_id": project_id,
        "name": name,
    })))
}

#[cfg(test)]
mod draft_tests {
    use super::{draft_brief, draft_shell};
    use crate::workspace::persistence::AppSettings;
    use okena_core::knowledge::{KnowledgeRoot, KnowledgeRootKind};

    fn root(kind: KnowledgeRootKind) -> KnowledgeRoot {
        KnowledgeRoot {
            key: "store:acme-eng".into(),
            kind,
            name: "acme-eng".into(),
            path: "/k/eng".into(),
            store_id: Some("acme-eng".into()),
            description: None,
            remote: None,
            healthy: true,
            git: None,
            counts: Default::default(),
            used_by: Vec::new(),
            status: Vec::new(),
        }
    }

    #[test]
    fn the_brief_carries_the_request_the_layout_and_the_branch_rule_for_stores() {
        let brief = draft_brief(
            "document how CI caches work",
            &root(KnowledgeRootKind::Store),
            None,
        );
        for needle in [
            "document how CI caches work",
            "`/k/eng`",
            "docs/**/*.md",
            "skills/<name>/SKILL.md",
            "agents/<name>.md",
            "templates/**/*.md",
            "`{placeholder}`",
            "`description`",
            "knowledge/<short-topic>",
            "do not push",
        ] {
            assert!(
                brief.contains(needle),
                "brief is missing {needle:?}:\n{brief}"
            );
        }

        let project = draft_brief("x", &root(KnowledgeRootKind::Project), None);
        assert!(!project.contains("knowledge/<short-topic>"));
        assert!(project.contains("Leave committing to me"));
    }

    #[test]
    fn a_draft_needs_an_agent_to_start() {
        let settings = AppSettings::default();
        assert!(draft_shell(&settings, None, "p").is_err_and(|e| e.contains("Settings → Harness")));
        assert!(draft_shell(&settings, Some("  "), "p").is_err());
        assert!(draft_shell(&settings, Some("claude"), "p").is_ok());
    }
}

#[cfg(test)]
mod tests {
    use super::{ActionResult, execute_at, knowledge_project_sources};
    use crate::workspace::persistence::AppSettings;
    use crate::workspace::state::ProjectData;
    use okena_core::api::ActionRequest;
    use okena_core::knowledge::{KnowledgeDocument, KnowledgeStores, KnowledgeTree};
    use okena_knowledge::discover::ProjectSource;
    use std::path::{Path, PathBuf};

    fn tmpdir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "okena-knowledge-{tag}-{}-{}",
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

    /// A registry inside `sandbox`, so no test touches the real profile.
    fn registry(sandbox: &Path) -> PathBuf {
        okena_knowledge::registry::registry_path(&sandbox.join("config"))
    }

    fn store(root: &Path, id: &str) {
        write(
            &root.join(".okena-knowledge/store.yaml"),
            &format!("version: 1\nid: {id}\n"),
        );
        write(&root.join("docs/readme.md"), "# Readme\n");
    }

    fn run(sandbox: &Path, projects: &[ProjectSource], action: ActionRequest) -> ActionResult {
        execute_at(
            &registry(sandbox),
            &action,
            projects,
            &AppSettings::default(),
        )
        .expect("a knowledge action")
    }

    /// Decode a successful action's payload into whatever the binding asks for.
    /// A macro so the target type comes from the `let`: this crate has no
    /// direct `serde` dependency to name a deserialize bound with.
    macro_rules! ok {
        ($result:expr) => {
            match $result {
                ActionResult::Ok(Some(v)) => serde_json::from_value(v).unwrap(),
                ActionResult::Ok(None) => panic!("expected a payload"),
                ActionResult::Err(e) => panic!("expected success, got: {e}"),
            }
        };
    }

    fn err(result: ActionResult) -> String {
        match result {
            ActionResult::Err(e) => e,
            ActionResult::Ok(_) => panic!("expected an error"),
        }
    }

    fn project(json: serde_json::Value) -> ProjectData {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn project_sources_skip_worktrees_and_sessions_and_follow_the_setting() {
        let plain = project(serde_json::json!({"id": "p1", "name": "app", "path": "/r/app"}));
        let projects = [
            plain.clone(),
            project(serde_json::json!({
                "id": "p2", "name": "app-wt", "path": "/r/wt",
                "worktree_info": {"parent_project_id": "p1"},
            })),
            project(serde_json::json!({
                "id": "p3", "name": "spec", "path": "/r/s", "spec_change": "add-login",
            })),
            project(serde_json::json!({
                "id": "p4", "name": "agent", "path": "/r", "custom_session": "breakdown",
            })),
        ];
        let mut settings = AppSettings::default();
        let names: Vec<_> = knowledge_project_sources(&projects, &settings)
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["app"]);

        settings.harness.knowledge.projects = false;
        assert!(knowledge_project_sources(&[plain], &settings).is_empty());
    }

    #[test]
    fn register_list_open_and_unregister_through_the_actions() {
        let sandbox = tmpdir("lifecycle");
        let checkout = sandbox.join("eng");
        store(&checkout, "acme-eng");

        let registered: serde_json::Value = ok!(run(
            &sandbox,
            &[],
            ActionRequest::KnowledgeStoreRegister {
                path: checkout.to_string_lossy().into_owned(),
            },
        ));
        assert_eq!(registered["id"], "acme-eng");

        let stores: KnowledgeStores = ok!(run(&sandbox, &[], ActionRequest::KnowledgeStores));
        let root = stores.root("store:acme-eng").expect("listed");
        assert!(root.healthy);
        assert!(root.git.is_none(), "not a git checkout");

        let tree: KnowledgeTree = ok!(run(
            &sandbox,
            &[],
            ActionRequest::KnowledgeTree { root: None },
        ));
        assert_eq!(tree.root_key, "store:acme-eng");
        assert_eq!(tree.store_id.as_deref(), Some("acme-eng"));
        assert_eq!(tree.entries[0].title, "Readme");

        let doc: KnowledgeDocument = ok!(run(
            &sandbox,
            &[],
            ActionRequest::KnowledgeRead {
                root: Some("store:acme-eng".into()),
                path: "docs/readme.md".into(),
            },
        ));
        assert_eq!(doc.content, "# Readme\n");

        let _: serde_json::Value = ok!(run(
            &sandbox,
            &[],
            ActionRequest::KnowledgeStoreUnregister {
                id: "acme-eng".into(),
            },
        ));
        let stores: KnowledgeStores = ok!(run(&sandbox, &[], ActionRequest::KnowledgeStores));
        assert!(stores.roots.is_empty());
        assert!(checkout.join("docs/readme.md").is_file(), "left on disk");
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn undiscovered_keys_escaping_paths_and_huge_files_are_refused() {
        let sandbox = tmpdir("guards");
        let checkout = sandbox.join("eng");
        store(&checkout, "acme-eng");
        write(&sandbox.join("outside.md"), "SECRET");
        write(
            &checkout.join("docs/huge.md"),
            &"x".repeat(2 * 1024 * 1024 + 1),
        );
        okena_knowledge::registry::register(&registry(&sandbox), &checkout.to_string_lossy(), None)
            .unwrap();

        let key = format!("path:{}", sandbox.to_string_lossy());
        assert!(
            err(run(
                &sandbox,
                &[],
                ActionRequest::KnowledgeTree {
                    root: Some(key.clone())
                }
            ))
            .contains("unknown knowledge root")
        );
        assert!(
            err(run(
                &sandbox,
                &[],
                ActionRequest::KnowledgeRead {
                    root: Some(key),
                    path: "outside.md".into(),
                },
            ))
            .contains("unknown knowledge root")
        );
        assert!(
            !err(run(
                &sandbox,
                &[],
                ActionRequest::KnowledgeRead {
                    root: None,
                    path: "docs/../../outside.md".into(),
                },
            ))
            .is_empty()
        );
        assert!(
            err(run(
                &sandbox,
                &[],
                ActionRequest::KnowledgeRead {
                    root: None,
                    path: "docs/huge.md".into(),
                },
            ))
            .contains("too large")
        );
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn a_write_replaces_what_was_read_and_refuses_stale_revisions_and_escapes() {
        let sandbox = tmpdir("write");
        let checkout = sandbox.join("eng");
        store(&checkout, "acme-eng");
        write(&sandbox.join("outside.md"), "SECRET");
        okena_knowledge::registry::register(&registry(&sandbox), &checkout.to_string_lossy(), None)
            .unwrap();
        let save = |path: &str, content: &str, revision: &str| {
            run(
                &sandbox,
                &[],
                ActionRequest::KnowledgeWrite {
                    root: None,
                    path: path.into(),
                    content: content.into(),
                    revision: revision.into(),
                },
            )
        };

        let doc: KnowledgeDocument = ok!(run(
            &sandbox,
            &[],
            ActionRequest::KnowledgeRead {
                root: None,
                path: "docs/readme.md".into(),
            },
        ));
        let saved: serde_json::Value = ok!(save("docs/readme.md", "# Edited\n", &doc.revision));
        assert_eq!(
            std::fs::read_to_string(checkout.join("docs/readme.md")).unwrap(),
            "# Edited\n"
        );
        let reopened: KnowledgeDocument = ok!(run(
            &sandbox,
            &[],
            ActionRequest::KnowledgeRead {
                root: None,
                path: "docs/readme.md".into(),
            },
        ));
        assert_eq!(reopened.content, "# Edited\n");
        assert_eq!(saved["revision"], reopened.revision);

        // The revision from before the first save is stale now.
        assert!(
            err(save("docs/readme.md", "# Clobber\n", &doc.revision)).contains("changed on disk")
        );
        assert_eq!(reopened.content, "# Edited\n");

        let outside = okena_core::fs::content_revision("SECRET");
        assert!(!err(save("docs/../../outside.md", "pwned", &outside)).is_empty());
        assert_eq!(
            std::fs::read_to_string(sandbox.join("outside.md")).unwrap(),
            "SECRET"
        );
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn entries_are_created_renamed_and_deleted_through_the_actions_and_never_outside() {
        let sandbox = tmpdir("files");
        let checkout = sandbox.join("eng");
        store(&checkout, "acme-eng");
        write(&sandbox.join("outside.md"), "SECRET");
        okena_knowledge::registry::register(&registry(&sandbox), &checkout.to_string_lossy(), None)
            .unwrap();
        let create = |path: &str, content: &str| {
            run(
                &sandbox,
                &[],
                ActionRequest::KnowledgeFileCreate {
                    root: None,
                    path: path.into(),
                    content: content.into(),
                },
            )
        };
        let rename = |from: &str, to: &str| {
            run(
                &sandbox,
                &[],
                ActionRequest::KnowledgeFileRename {
                    root: None,
                    from: from.into(),
                    to: to.into(),
                },
            )
        };
        let delete = |path: &str| {
            run(
                &sandbox,
                &[],
                ActionRequest::KnowledgeFileDelete {
                    root: None,
                    path: path.into(),
                },
            )
        };

        // A new skill is on disk where the tree says, and listed.
        let created: serde_json::Value = ok!(create(
            "skills/release/SKILL.md",
            "---\nname: release\n---\n"
        ));
        assert_eq!(created["path"], "skills/release/SKILL.md");
        let tree: KnowledgeTree = ok!(run(
            &sandbox,
            &[],
            ActionRequest::KnowledgeTree { root: None },
        ));
        assert!(tree.entry("skills/release/SKILL.md").is_some());

        // Collisions, hidden names and escapes are refused with a reason.
        assert!(err(create("docs/readme.md", "clobber")).contains("already exists"));
        assert_eq!(
            std::fs::read_to_string(checkout.join("docs/readme.md")).unwrap(),
            "# Readme\n"
        );
        for bad in [
            "docs/../../outside-new.md",
            ".okena-knowledge/store.yaml",
            "  ",
        ] {
            assert!(!err(create(bad, "x")).is_empty(), "accepted {bad:?}");
        }
        assert!(!sandbox.join("outside-new.md").exists());

        let renamed: serde_json::Value = ok!(rename("docs/readme.md", "docs/guides/readme.md"));
        assert_eq!(renamed["path"], "docs/guides/readme.md");
        assert!(checkout.join("docs/guides/readme.md").is_file());
        assert!(!checkout.join("docs/readme.md").exists());
        assert!(!err(rename("docs/../../outside.md", "docs/x.md")).is_empty());
        assert!(!err(rename("docs/guides/readme.md", "../escaped.md")).is_empty());
        assert!(!sandbox.join("escaped.md").exists());

        let _: serde_json::Value = ok!(run(
            &sandbox,
            &[],
            ActionRequest::KnowledgeFolderCreate {
                root: None,
                path: "templates/flows".into(),
            },
        ));
        assert!(checkout.join("templates/flows").is_dir());

        assert!(!err(delete("docs/guides")).is_empty(), "a folder");
        assert!(!err(delete("docs/../../outside.md")).is_empty());
        let _: serde_json::Value = ok!(delete("docs/guides/readme.md"));
        assert!(!checkout.join("docs/guides/readme.md").exists());
        assert_eq!(
            std::fs::read_to_string(sandbox.join("outside.md")).unwrap(),
            "SECRET"
        );
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn nothing_configured_says_where_to_configure_it() {
        let sandbox = tmpdir("empty");
        let e = err(run(
            &sandbox,
            &[],
            ActionRequest::KnowledgeTree { root: None },
        ));
        assert!(e.contains("Settings → Knowledge"), "unhelpful: {e}");
        std::fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn project_roots_open_but_do_not_sync() {
        let sandbox = tmpdir("project");
        let repo = sandbox.join("app");
        write(&repo.join(".okena/knowledge/docs/a.md"), "# A\n");
        let projects = [ProjectSource {
            name: "app".into(),
            path: repo.clone(),
        }];
        let stores: KnowledgeStores = ok!(run(&sandbox, &projects, ActionRequest::KnowledgeStores));
        let key = stores.roots[0].key.clone();

        let tree: KnowledgeTree = ok!(run(
            &sandbox,
            &projects,
            ActionRequest::KnowledgeTree {
                root: Some(key.clone()),
            },
        ));
        assert_eq!(tree.entries.len(), 1);
        assert!(tree.store_id.is_none());
        assert!(
            err(run(
                &sandbox,
                &projects,
                ActionRequest::KnowledgeStoreFetch { root: key },
            ))
            .contains("not a store")
        );
        std::fs::remove_dir_all(&sandbox).ok();
    }
}
