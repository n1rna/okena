//! Which roots hold context, who owns them and which projects follow them.
//!
//! Built from what okena already discovers — the workspace's projects, the
//! knowledge stores and the OpenSpec roots — so the launcher searches exactly
//! the roots the Specs and Knowledge sections show. Holds no file contents:
//! [`Catalog::items`] reads a root when asked, and the index caches that.

use crate::items::{self, MapItems, Owner};
use okena_core::context::{ContextItem, ContextKind, ContextOwner, ContextRef};
use okena_core::knowledge::{KnowledgeRootKind, KnowledgeStores};
use okena_core::project_map::MapStatus;
use okena_core::specs::{SpecRootKind, SpecStores};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// A workspace project context can come from: a repository, not a worktree or
/// an agent session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogProject {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
}

/// What a root is read as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RootKind {
    /// A project's knowledge root, read for its `project-map.yaml`.
    Map,
    Spec,
    Knowledge,
}

impl RootKind {
    /// Whether an item of `kind` comes from a root of this kind.
    pub fn holds(self, kind: ContextKind) -> bool {
        match self {
            RootKind::Map => kind == ContextKind::MapEntry,
            RootKind::Spec => kind == ContextKind::Spec,
            RootKind::Knowledge => kind.is_installable() || kind == ContextKind::Doc,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogRoot {
    pub kind: RootKind,
    pub owner: Owner,
    /// The directory read. For a map root with no knowledge root, `None`:
    /// the project can have no map.
    pub path: Option<PathBuf>,
    /// Ids of the projects that own or follow it. A chosen one ranks it first.
    pub projects: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catalog {
    pub projects: Vec<CatalogProject>,
    pub roots: Vec<CatalogRoot>,
}

impl Catalog {
    /// Every root: one map root per project, then every healthy knowledge and
    /// spec root discovery found.
    pub fn build(
        projects: Vec<CatalogProject>,
        knowledge: &KnowledgeStores,
        specs: &SpecStores,
    ) -> Self {
        let mut roots = Vec::new();
        for project in &projects {
            roots.push(CatalogRoot {
                kind: RootKind::Map,
                owner: project_owner(project),
                path: map_root(&project.path),
                projects: BTreeSet::from([project.id.clone()]),
            });
        }
        for root in knowledge.roots.iter().filter(|r| r.healthy) {
            let path = PathBuf::from(&root.path);
            let owner = match root.kind {
                KnowledgeRootKind::Project => owning_project(&projects, &path),
                KnowledgeRootKind::Store => None,
            };
            roots.push(catalog_root(
                RootKind::Knowledge,
                owner,
                &root.key,
                &root.name,
                path,
                &root.used_by,
                &projects,
            ));
        }
        for root in specs.roots.iter().filter(|r| r.healthy) {
            let path = PathBuf::from(&root.path);
            let owner = match root.kind {
                SpecRootKind::Project => owning_project(&projects, &path),
                SpecRootKind::Store | SpecRootKind::Folder => None,
            };
            roots.push(catalog_root(
                RootKind::Spec,
                owner,
                &root.key,
                &root.name,
                path,
                &root.used_by,
                &projects,
            ));
        }
        Catalog { projects, roots }
    }

    pub fn project(&self, id: &str) -> Option<&CatalogProject> {
        self.projects.iter().find(|p| p.id == id)
    }

    /// Whether `root` belongs to or is followed by any of `project_ids`.
    pub fn is_chosen(root: &CatalogRoot, project_ids: &[String]) -> bool {
        project_ids.iter().any(|id| root.projects.contains(id))
    }

    /// Whether `path` lies inside a root one of `project_ids` owns or follows,
    /// checked on canonical paths so neither `..` nor a symlink escapes.
    pub fn in_scope(&self, project_ids: &[String], path: &Path) -> Option<PathBuf> {
        let real = path.canonicalize().ok()?;
        if !real.is_file() {
            return None;
        }
        self.roots
            .iter()
            .filter(|root| Self::is_chosen(root, project_ids))
            .filter_map(|root| scope_base(root)?.canonicalize().ok())
            .any(|base| real.starts_with(&base))
            .then_some(real)
    }

    /// Read `root`'s items from disk.
    pub fn items(root: &CatalogRoot) -> (Vec<ContextItem>, Option<MapStatus>) {
        match (root.kind, root.path.as_deref()) {
            (RootKind::Map, None) => (Vec::new(), Some(MapStatus::NotScanned)),
            (RootKind::Map, Some(path)) => {
                let MapItems { status, items } = items::map_items(&root.owner, path);
                (items, Some(status))
            }
            (_, None) => (Vec::new(), None),
            (RootKind::Spec, Some(path)) => (items::spec_items(&root.owner, path), None),
            (RootKind::Knowledge, Some(path)) => (items::knowledge_items(&root.owner, path), None),
        }
    }

    /// Resolve refs a client sent back into items, reading every root again.
    ///
    /// A ref that names no current item — its file deleted, its map entry
    /// renamed, its owner gone — is dropped rather than trusted. Order is the
    /// client's, duplicates collapse.
    pub fn resolve(&self, refs: &[ContextRef]) -> Vec<ContextItem> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for r in refs {
            if !seen.insert(r.clone()) {
                continue;
            }
            let found = self
                .roots
                .iter()
                .filter(|root| root.owner.owner == r.owner && root.kind.holds(r.kind))
                .find_map(|root| {
                    Self::items(root)
                        .0
                        .into_iter()
                        .find(|item| item.reference == *r)
                });
            match found {
                Some(item) => out.push(item),
                None => log::info!("[context] dropping a ref that no longer resolves: {r:?}"),
            }
        }
        out
    }
}

/// The directory a root's readable files lie under.
///
/// An OpenSpec root is often a whole repository, but only its `openspec/`
/// folder is specs: scoping reads to the root itself would hand an agent every
/// file in the checkout.
fn scope_base(root: &CatalogRoot) -> Option<PathBuf> {
    let path = root.path.as_ref()?;
    Some(match root.kind {
        RootKind::Spec => path.join(okena_openspec::root::OPENSPEC_DIR),
        RootKind::Map | RootKind::Knowledge => path.clone(),
    })
}

fn project_owner(project: &CatalogProject) -> Owner {
    Owner {
        owner: ContextOwner::project(&project.id),
        name: project.name.clone(),
        base: project.path.clone(),
    }
}

/// A project's knowledge root, where its map lives. `None` when it has none or
/// its `.okena/knowledge.yaml` is unreadable.
fn map_root(repo: &Path) -> Option<PathBuf> {
    let config = okena_knowledge::project::read_config(repo).ok()?;
    okena_knowledge::project::project_root(repo, config.as_ref())
        .ok()
        .flatten()
}

/// The project whose checkout holds `root`, for a root discovery attributed to
/// a project.
fn owning_project<'a>(projects: &'a [CatalogProject], root: &Path) -> Option<&'a CatalogProject> {
    let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let root = real(root);
    projects
        .iter()
        .filter(|p| root.starts_with(real(&p.path)))
        // The deepest checkout wins, should one repository sit inside another.
        .max_by_key(|p| p.path.components().count())
}

fn catalog_root(
    kind: RootKind,
    project: Option<&CatalogProject>,
    key: &str,
    name: &str,
    path: PathBuf,
    used_by: &[String],
    projects: &[CatalogProject],
) -> CatalogRoot {
    // Discovery names followers by project name.
    let mut followers: BTreeSet<String> = projects
        .iter()
        .filter(|p| used_by.contains(&p.name))
        .map(|p| p.id.clone())
        .collect();
    let owner = match project {
        Some(project) => {
            followers.insert(project.id.clone());
            project_owner(project)
        }
        None => Owner {
            owner: ContextOwner::store(key),
            name: name.to_string(),
            base: path.clone(),
        },
    };
    CatalogRoot {
        kind,
        owner,
        path: Some(path),
        projects: followers,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::items::fixtures::*;
    use okena_core::knowledge::KnowledgeRoot;
    use okena_core::specs::SpecRoot;

    /// Two mapped projects `shop` and `billing`, a knowledge store `acme`
    /// followed by `shop`, and a spec root inside `billing`.
    pub struct World {
        pub dir: tempfile::TempDir,
        pub catalog: Catalog,
    }

    pub fn world() -> World {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let shop = base.join("shop");
        let billing = base.join("billing");
        let store = base.join("acme");
        mapped_repo(&shop);
        mapped_repo(&billing);
        spec_root(&billing);
        knowledge_store(&store);

        let projects = vec![
            CatalogProject {
                id: "p-shop".into(),
                name: "shop".into(),
                path: shop.clone(),
            },
            CatalogProject {
                id: "p-billing".into(),
                name: "billing".into(),
                path: billing.clone(),
            },
            CatalogProject {
                id: "p-bare".into(),
                name: "bare".into(),
                path: base.join("bare"),
            },
        ];
        std::fs::create_dir_all(base.join("bare")).unwrap();

        let acme = KnowledgeRoot {
            key: "store:acme".into(),
            kind: KnowledgeRootKind::Store,
            name: "acme".into(),
            path: store.to_string_lossy().into_owned(),
            store_id: Some("acme".into()),
            description: None,
            remote: None,
            healthy: true,
            git: None,
            counts: Default::default(),
            used_by: vec!["shop".into()],
            status: Vec::new(),
        };
        let knowledge = KnowledgeStores {
            roots: vec![acme],
            ..Default::default()
        };
        let billing_specs = SpecRoot {
            key: format!("path:{}", billing.display()),
            kind: SpecRootKind::Project,
            name: "billing".into(),
            path: billing.to_string_lossy().into_owned(),
            store_id: None,
            remote: None,
            schema: None,
            healthy: true,
            is_default: false,
            git: None,
            references: Vec::new(),
            used_by: Vec::new(),
            status: Vec::new(),
        };
        let specs = SpecStores {
            roots: vec![billing_specs],
            ..Default::default()
        };
        World {
            catalog: Catalog::build(projects, &knowledge, &specs),
            dir,
        }
    }

    #[test]
    fn a_followed_store_counts_as_its_followers_and_a_project_root_as_its_project() {
        let w = world();
        let store = w
            .catalog
            .roots
            .iter()
            .find(|r| r.kind == RootKind::Knowledge)
            .unwrap();
        assert_eq!(store.owner.owner, ContextOwner::store("store:acme"));
        assert_eq!(store.projects, BTreeSet::from(["p-shop".to_string()]));

        let specs = w
            .catalog
            .roots
            .iter()
            .find(|r| r.kind == RootKind::Spec)
            .unwrap();
        assert_eq!(specs.owner.owner, ContextOwner::project("p-billing"));
        assert!(Catalog::is_chosen(specs, &["p-billing".into()]));
        assert!(!Catalog::is_chosen(specs, &["p-shop".into()]));
    }

    #[test]
    fn refs_resolve_again_and_stale_ones_are_dropped() {
        let w = world();
        let area = ContextRef {
            kind: ContextKind::MapEntry,
            owner: ContextOwner::project("p-shop"),
            locator: "area:checkout".into(),
        };
        let doc = ContextRef {
            kind: ContextKind::Doc,
            owner: ContextOwner::store("store:acme"),
            locator: "docs/principles.md".into(),
        };
        let gone = ContextRef {
            kind: ContextKind::Doc,
            owner: ContextOwner::store("store:acme"),
            locator: "docs/deleted.md".into(),
        };
        // A path a client made up: the right shape, pointing out of the root.
        let escape = ContextRef {
            kind: ContextKind::Doc,
            owner: ContextOwner::store("store:acme"),
            locator: "../shop/.okena/knowledge/project-map.yaml".into(),
        };
        let unknown_owner = ContextRef {
            owner: ContextOwner::project("p-nope"),
            ..area.clone()
        };
        let got = w.catalog.resolve(&[
            area.clone(),
            gone,
            doc.clone(),
            escape,
            unknown_owner,
            area.clone(),
        ]);
        let refs: Vec<_> = got.iter().map(|i| i.reference.clone()).collect();
        assert_eq!(refs, [area, doc]);
        assert_eq!(got[0].title, "Checkout");
    }

    #[test]
    fn scope_admits_only_files_under_chosen_roots() {
        let w = world();
        let base = w.dir.path().canonicalize().unwrap();
        let in_store = base.join("acme/docs/principles.md");
        let shop = ["p-shop".to_string()];
        assert!(w.catalog.in_scope(&shop, &in_store).is_some());
        // The same file is outside billing's scope: billing follows no store.
        assert!(
            w.catalog
                .in_scope(&["p-billing".into()], &in_store)
                .is_none()
        );
        // A file of the repository that is not under a root.
        std::fs::write(base.join("shop/secret.txt"), "x").unwrap();
        assert!(
            w.catalog
                .in_scope(&shop, &base.join("shop/secret.txt"))
                .is_none()
        );
        // `..` resolves before the check.
        let sneaky = base.join("acme/docs/../../shop/secret.txt");
        assert!(w.catalog.in_scope(&shop, &sneaky).is_none());
    }

    #[test]
    fn a_repository_that_is_a_spec_root_exposes_only_its_specs() {
        let w = world();
        let base = w.dir.path().canonicalize().unwrap();
        let billing = ["p-billing".to_string()];
        // billing's OpenSpec root is the repository itself.
        let spec = base.join("billing/openspec/specs/auth/spec.md");
        assert!(w.catalog.in_scope(&billing, &spec).is_some());
        std::fs::write(base.join("billing/secret.env"), "TOKEN=1").unwrap();
        assert!(
            w.catalog
                .in_scope(&billing, &base.join("billing/secret.env"))
                .is_none(),
            "a file of the repository outside openspec/ must not be readable"
        );
        // Its own knowledge root still is.
        let map_doc = base.join("billing/.okena/knowledge/docs/project/checkout.md");
        assert!(w.catalog.in_scope(&billing, &map_doc).is_some());
    }
}
