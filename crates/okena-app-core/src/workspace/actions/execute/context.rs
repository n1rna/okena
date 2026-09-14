//! Launch context (QBL-406) on the workspace side.
//!
//! What the catalog of roots is for this workspace, the items a launch's refs
//! resolve to, and what a running agent's own lookups are scoped to. The live
//! index that searches a catalog is the daemon's (`okena-context`'s `index`);
//! nothing here needs it, so the desktop links no fff.

use super::ActionResult;
use crate::workspace::persistence::{AppSettings, get_config_dir};
use okena_context::catalog::{Catalog, CatalogProject};
use okena_core::context::{ContextDocument, ContextItem, ContextRef};
use okena_workspace::state::ProjectData;
use std::io::Read;
use std::path::Path;

/// Largest file an agent reads through `okena_context_read`, like a knowledge
/// read's cap.
const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;

/// Every root context can come from in this workspace: each repository's map,
/// and the knowledge and OpenSpec roots discovery finds — the same ones the
/// Knowledge and Specs sections list.
pub fn context_catalog(projects: &[ProjectData], settings: &AppSettings) -> Catalog {
    let knowledge = okena_knowledge::discover::discover(&okena_knowledge::discover::Sources {
        registry_path: okena_knowledge::registry::registry_path(&get_config_dir()),
        projects: super::knowledge::knowledge_project_sources(projects, settings),
    });
    let specs = okena_openspec::discover::discover(
        &super::specs::dirs(settings),
        &super::specs::spec_sources(projects, settings),
    );
    Catalog::build(catalog_projects(projects), &knowledge, &specs)
}

/// The projects context can come from: repositories, never a worktree (a
/// second checkout of one) or an agent session.
pub(super) fn catalog_projects(projects: &[ProjectData]) -> Vec<CatalogProject> {
    projects
        .iter()
        .filter(|p| p.worktree_info.is_none() && !p.is_any_agent_session())
        .map(|p| CatalogProject {
            id: p.id.clone(),
            name: p.name.clone(),
            path: okena_core::fs::expand_home(&p.path),
        })
        .collect()
}

/// The items a launch was handed, resolved again on this side.
///
/// No refs, no discovery: a launch without context costs what it did before.
pub(super) fn resolve_for_launch(
    projects: &[ProjectData],
    settings: &AppSettings,
    refs: &[ContextRef],
) -> Vec<ContextItem> {
    if refs.is_empty() {
        return Vec::new();
    }
    context_catalog(projects, settings).resolve(refs)
}

/// What a session's lookups are scoped to: the projects it was pointed at, in
/// order, then the projects owning context it was handed.
pub(super) fn scope_projects(chosen: &[String], items: &[ContextItem]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let owners = items
        .iter()
        .filter_map(|i| i.reference.owner.project_id().map(str::to_string));
    for id in chosen.iter().cloned().chain(owners) {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// The projects a terminal's lookups are scoped to.
///
/// A session's own `context_projects`; for a terminal in an ordinary
/// repository, that repository. A session started without any projects has
/// nothing to look up in, which is said rather than widened to everything.
pub fn session_scope(projects: &[ProjectData], terminal_id: &str) -> Result<Vec<String>, String> {
    let project = projects
        .iter()
        .find(|p| {
            p.layout
                .as_ref()
                .is_some_and(|l| l.collect_terminal_ids().iter().any(|t| t == terminal_id))
        })
        .ok_or_else(|| format!("no okena project has terminal `{terminal_id}`"))?;
    if !project.context_projects.is_empty() {
        return Ok(project.context_projects.clone());
    }
    if project.worktree_info.is_none() && !project.is_any_agent_session() {
        return Ok(vec![project.id.clone()]);
    }
    Err("this session was started without projects, so it has no context to look up".into())
}

/// Read `path` for an agent scoped to `scope`, refusing anything outside the
/// roots those projects own or follow.
pub fn read_in_scope(catalog: &Catalog, scope: &[String], path: &str) -> ActionResult {
    let Some(real) = catalog.in_scope(scope, Path::new(path)) else {
        return ActionResult::Err(format!(
            "`{path}` is not in this session's projects or the stores they follow"
        ));
    };
    let file = match std::fs::File::open(&real) {
        Ok(f) => f,
        Err(e) => return ActionResult::Err(format!("could not read `{path}`: {e}")),
    };
    if file.metadata().map(|m| m.len()).unwrap_or(0) > MAX_READ_BYTES {
        return ActionResult::Err(format!("`{path}` is larger than 2 MiB"));
    }
    let mut content = String::new();
    if let Err(e) = file.take(MAX_READ_BYTES).read_to_string(&mut content) {
        return ActionResult::Err(format!("`{path}` is not readable text: {e}"));
    }
    let document = ContextDocument {
        path: real.to_string_lossy().into_owned(),
        content,
    };
    match serde_json::to_value(document) {
        Ok(v) => ActionResult::Ok(Some(v)),
        Err(e) => ActionResult::Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_core::context::{ContextKind, ContextOwner};

    fn project(json: serde_json::Value) -> ProjectData {
        serde_json::from_value(json).unwrap()
    }

    fn with_terminal(mut json: serde_json::Value, terminal: &str) -> ProjectData {
        json["layout"] = serde_json::json!({ "type": "terminal", "terminal_id": terminal });
        project(json)
    }

    fn item(owner: ContextOwner) -> ContextItem {
        ContextItem {
            reference: ContextRef {
                kind: ContextKind::Doc,
                owner,
                locator: "docs/x.md".into(),
            },
            title: "x".into(),
            description: String::new(),
            owner_name: "o".into(),
            path: "/x".into(),
            map_id: None,
            chosen: false,
        }
    }

    #[test]
    fn a_sessions_scope_is_its_chosen_projects_then_the_owners_of_its_context() {
        let items = [
            item(ContextOwner::store("store:acme")),
            item(ContextOwner::project("b")),
            item(ContextOwner::project("a")),
            item(ContextOwner::project("c")),
        ];
        assert_eq!(
            scope_projects(&["a".into(), "b".into()], &items),
            ["a", "b", "c"]
        );
        assert!(scope_projects(&[], &[]).is_empty());
    }

    #[test]
    fn a_terminal_is_scoped_by_the_session_it_runs_in() {
        let projects = [
            with_terminal(
                serde_json::json!({
                    "id": "s1", "name": "session", "path": "/p",
                    "custom_session": "do it",
                    "context_projects": ["repo-a", "repo-b"],
                }),
                "t-session",
            ),
            with_terminal(
                serde_json::json!({ "id": "repo-a", "name": "a", "path": "/p/a" }),
                "t-repo",
            ),
            with_terminal(
                serde_json::json!({
                    "id": "s2", "name": "old session", "path": "/p",
                    "custom_session": "started before scopes were recorded",
                }),
                "t-bare",
            ),
        ];
        assert_eq!(
            session_scope(&projects, "t-session").unwrap(),
            ["repo-a", "repo-b"]
        );
        // A shell in an ordinary repository looks up that repository.
        assert_eq!(session_scope(&projects, "t-repo").unwrap(), ["repo-a"]);
        // Nothing recorded: refused, never widened to every project.
        assert!(session_scope(&projects, "t-bare").is_err());
        assert!(session_scope(&projects, "t-nowhere").is_err());
    }

    /// `shop` follows the store `acme`; `billing` follows nothing.
    fn scoped_catalog(base: &Path) -> Catalog {
        use okena_core::knowledge::{KnowledgeRoot, KnowledgeRootKind, KnowledgeStores};
        let write = |rel: &str, body: &str| {
            let path = base.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        };
        write("acme/docs/principles.md", "# Principles\n");
        write(
            "shop/.okena/knowledge/docs/project/checkout.md",
            "# Checkout\n",
        );
        write("shop/secret.env", "TOKEN=1\n");
        write("billing/README.md", "# Billing\n");
        let projects = ["shop", "billing"]
            .map(|name| CatalogProject {
                id: format!("p-{name}"),
                name: name.into(),
                path: base.join(name),
            })
            .to_vec();
        let acme = KnowledgeRoot {
            key: "store:acme".into(),
            kind: KnowledgeRootKind::Store,
            name: "acme".into(),
            path: base.join("acme").to_string_lossy().into_owned(),
            store_id: Some("acme".into()),
            description: None,
            remote: None,
            healthy: true,
            git: None,
            counts: Default::default(),
            used_by: vec!["shop".into()],
            status: Vec::new(),
        };
        Catalog::build(
            projects,
            &KnowledgeStores {
                roots: vec![acme],
                ..Default::default()
            },
            &Default::default(),
        )
    }

    fn read(catalog: &Catalog, scope: &[&str], path: &Path) -> ActionResult {
        let scope: Vec<String> = scope.iter().map(|s| s.to_string()).collect();
        read_in_scope(catalog, &scope, &path.to_string_lossy())
    }

    #[test]
    fn an_agent_reads_its_followed_stores_and_nothing_outside_them() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let catalog = scoped_catalog(&base);
        let principles = base.join("acme/docs/principles.md");

        // Its project follows the store: readable.
        match read(&catalog, &["p-shop"], &principles) {
            ActionResult::Ok(Some(v)) => {
                let doc: ContextDocument = serde_json::from_value(v).unwrap();
                assert_eq!(doc.content, "# Principles\n");
            }
            _ => panic!("a followed store's doc must be readable"),
        }
        // Its own project's map doc: readable.
        assert!(matches!(
            read(
                &catalog,
                &["p-shop"],
                &base.join("shop/.okena/knowledge/docs/project/checkout.md")
            ),
            ActionResult::Ok(_)
        ));
        // A session on billing does not follow acme: refused.
        match read(&catalog, &["p-billing"], &principles) {
            ActionResult::Err(e) => assert!(e.contains("not in this session's projects"), "{e}"),
            ActionResult::Ok(_) => panic!("a store billing does not follow was readable"),
        }
        // A file of the repository that is under no root: refused.
        assert!(matches!(
            read(&catalog, &["p-shop"], &base.join("shop/secret.env")),
            ActionResult::Err(_)
        ));
        // `..` out of a followed store: refused once resolved.
        assert!(matches!(
            read(
                &catalog,
                &["p-shop"],
                &base.join("acme/docs/../../shop/secret.env")
            ),
            ActionResult::Err(_)
        ));
        // Anything at all, with no scope.
        assert!(matches!(
            read(&catalog, &[], &principles),
            ActionResult::Err(_)
        ));
    }

    #[test]
    fn a_launch_without_context_resolves_nothing() {
        let settings = AppSettings::default();
        assert!(resolve_for_launch(&[], &settings, &[]).is_empty());
    }
}
