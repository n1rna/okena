//! Changing one open document with an agent: a spec, a change file or a
//! knowledge entry.
//!
//! The same session in both sections — rooted at the document's root, briefed
//! with `doc-refine`, committing nothing — so the two differ only in how the
//! root and the path are checked. Both are checked against what discovery
//! found: these actions are reachable by any paired client, and must not
//! become a way to point an agent at an arbitrary file.

use super::ActionResult;
use super::briefs::{self, PromptRoots};
use crate::workspace::persistence::{AppSettings, get_config_dir};
use crate::workspace::state::{WindowId, Workspace};
use okena_core::harness::AgentPurpose;
use okena_knowledge::prompts::{Flow, Vars};
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::TerminalBackend;
use okena_workspace::context::WorkspaceCx;
use std::path::{Path, PathBuf};

/// The document a session is started on, checked.
struct Target {
    root_path: String,
    /// Relative to the root, as the tree names it.
    path: String,
    /// Where the agent edits it.
    file: PathBuf,
    /// What kind of document it is, in words.
    what: String,
    purpose: AgentPurpose,
    /// Set for a knowledge root, whose sessions are recognized by it.
    knowledge_root: Option<String>,
}

/// Start an agent on one document of a spec root.
#[allow(clippy::too_many_arguments)]
pub(super) fn refine_spec_document(
    ws: &mut Workspace,
    window_id: WindowId,
    root: String,
    path: String,
    request: String,
    agent_command: Option<String>,
    model: Option<String>,
    context_refs: Vec<okena_core::context::ContextRef>,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let root = match super::specs::resolve_root(&ws.data.projects, settings, Some(&root)) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    let file = match existing_file(
        okena_openspec::tree::resolve_document(Path::new(&root.path), &path),
        &path,
    ) {
        Ok(f) => f,
        Err(e) => return ActionResult::Err(e),
    };
    let target = Target {
        root_path: root.path.clone(),
        what: spec_document_kind(&path),
        purpose: AgentPurpose::SpecEdit {
            root: root.key.clone(),
            path: path.clone(),
        },
        path,
        file,
        knowledge_root: None,
    };
    start(
        ws,
        window_id,
        target,
        request,
        agent_command,
        model,
        context_refs,
        backend,
        terminals,
        settings,
        cx,
    )
}

/// Start an agent on one file of a knowledge root.
#[allow(clippy::too_many_arguments)]
pub(super) fn refine_knowledge_document(
    ws: &mut Workspace,
    window_id: WindowId,
    root: String,
    path: String,
    request: String,
    agent_command: Option<String>,
    model: Option<String>,
    context_refs: Vec<okena_core::context::ContextRef>,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let projects = super::knowledge::knowledge_project_sources(&ws.data.projects, settings);
    let registry = okena_knowledge::registry::registry_path(&get_config_dir());
    // Refining rewrites the file, so okena's own store is refused here for the
    // same reason a save is: the next start would undo whatever the agent did.
    let sources = super::knowledge::knowledge_sources(&registry, &projects, settings);
    let root = match super::knowledge::resolve_writable_root(&sources, Some(&root)) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    let file = match existing_file(
        okena_knowledge::tree::resolve_document(Path::new(&root.path), &path),
        &path,
    ) {
        Ok(f) => f,
        Err(e) => return ActionResult::Err(e),
    };
    let target = Target {
        root_path: root.path.clone(),
        what: "a knowledge file".to_string(),
        purpose: AgentPurpose::KnowledgeEdit {
            root: root.key.clone(),
            path: path.clone(),
        },
        path,
        file,
        knowledge_root: Some(root.key.clone()),
    };
    start(
        ws,
        window_id,
        target,
        request,
        agent_command,
        model,
        context_refs,
        backend,
        terminals,
        settings,
        cx,
    )
}

/// A resolved path, refused unless it is a file that exists: an agent told
/// to change a document that is not there would write a new one.
fn existing_file(resolved: Result<PathBuf, String>, path: &str) -> Result<PathBuf, String> {
    let file = resolved?;
    if !file.is_file() {
        return Err(format!(
            "`{path}` is not there any more — refresh and open it again"
        ));
    }
    Ok(file)
}

/// What a spec root's document is, as the brief words it.
fn spec_document_kind(path: &str) -> String {
    match path
        .strip_prefix("openspec/changes/")
        .and_then(|rest| rest.split('/').next())
        .filter(|name| !name.is_empty() && *name != "archive")
    {
        Some(change) => format!("a file of the OpenSpec change `{change}`"),
        None => "an OpenSpec document".to_string(),
    }
}

fn document_brief(
    request: &str,
    target: &Target,
    context: &[okena_core::context::ContextItem],
    loaded: bool,
    prompts: &PromptRoots,
) -> String {
    let mut vars = Vars::new();
    vars.insert(
        "context",
        briefs::context_block(context, loaded, prompts),
    );
    vars.insert("request", request.to_string());
    vars.insert("file", target.path.clone());
    vars.insert("path", target.file.display().to_string());
    vars.insert("root_path", target.root_path.clone());
    vars.insert("what", target.what.clone());
    briefs::build(Flow::DocumentRefine, prompts, &vars)
        .rendered
        .text
}

#[allow(clippy::too_many_arguments)]
fn start(
    ws: &mut Workspace,
    window_id: WindowId,
    target: Target,
    request: String,
    agent_command: Option<String>,
    model: Option<String>,
    context_refs: Vec<okena_core::context::ContextRef>,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let request = request.trim().to_string();
    if request.is_empty() {
        return ActionResult::Err("say what to change first".into());
    }
    let context_items =
        super::context::resolve_for_launch(&ws.data.projects, settings, &context_refs);
    let command = super::agent_context::launch_command(settings, agent_command.as_deref());
    let install = super::agent_context::install(&command, &context_items);
    let prompts = briefs::prompt_roots(&ws.data.projects, settings);
    let brief = document_brief(
        &request,
        &target,
        &context_items,
        install.loaded(),
        &prompts,
    );
    // Nothing is scaffolded, so a session without an agent would do nothing.
    let Some(shell) = super::specs::spec_agent_shell(
        settings,
        agent_command.as_deref(),
        &brief,
        &install,
        &briefs::launch_model(Flow::DocumentRefine, &prompts, model),
    ) else {
        return ActionResult::Err(
            "no agent to start — pick one, or set the agent command in Settings → Harness".into(),
        );
    };
    let file_name = Path::new(&target.path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| target.path.clone());
    let name = format!("Refine {file_name}");
    let project_id = match ws.add_project(
        name.clone(),
        target.root_path.clone(),
        true,
        &settings.hooks,
        window_id,
        cx,
    ) {
        Ok(id) => id,
        Err(e) => return ActionResult::Err(format!("could not open a session: {e}")),
    };
    // Marked and given its agent before the terminal spawns: the terminal
    // reads the project's shell as it starts, and the markers keep the session
    // out of discovery from the first snapshot.
    if let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == project_id) {
        p.custom_session = Some(format!("{}: {request}", target.path));
        p.knowledge_root = target.knowledge_root.clone();
        p.agent_purpose = Some(target.purpose.clone());
        p.context_projects = super::context::scope_projects(&[], &context_items);
        p.default_shell = Some(shell);
    }
    if let ActionResult::Err(e) =
        super::spawn_session_terminals(ws, &project_id, backend, terminals, settings, cx)
    {
        log::warn!("[harness] document session terminal failed to spawn: {e}");
    }
    ws.notify_data(cx);
    ActionResult::Ok(Some(serde_json::json!({
        "project_id": project_id,
        "name": name,
        "path": target.path,
    })))
}

#[cfg(test)]
mod tests {
    use super::{Target, document_brief, spec_document_kind};
    use okena_core::harness::AgentPurpose;
    use std::path::PathBuf;

    #[test]
    fn a_change_file_is_named_for_its_change() {
        assert_eq!(
            spec_document_kind("openspec/changes/add-login/design.md"),
            "a file of the OpenSpec change `add-login`"
        );
        assert_eq!(
            spec_document_kind("openspec/specs/auth/spec.md"),
            "an OpenSpec document"
        );
        // An archived change is history, not a change in flight.
        assert_eq!(
            spec_document_kind("openspec/changes/archive/2026-01-01-x/proposal.md"),
            "an OpenSpec document"
        );
    }

    #[test]
    fn the_brief_names_the_file_the_request_and_that_nothing_is_committed() {
        let target = Target {
            root_path: "/k/eng".into(),
            path: "docs/ci.md".into(),
            file: PathBuf::from("/k/eng/docs/ci.md"),
            what: "a knowledge file".into(),
            purpose: AgentPurpose::KnowledgeEdit {
                root: "store:eng".into(),
                path: "docs/ci.md".into(),
            },
            knowledge_root: Some("store:eng".into()),
        };
        let brief = document_brief("explain cache busting", &target, &[], false, &Vec::new());
        for needle in [
            "explain cache busting",
            "`docs/ci.md`",
            "`/k/eng/docs/ci.md`",
            "a knowledge file",
            "edit only this file",
            "Do not commit",
            "okena_report_status",
        ] {
            assert!(brief.contains(needle), "missing {needle:?}:\n{brief}");
        }
        // Not "no braces": the reporting partial shows a JSON example.
        for var in ["{request}", "{file}", "{path}", "{root_path}", "{what}"] {
            assert!(!brief.contains(var), "unfilled {var}:\n{brief}");
        }
    }
}
