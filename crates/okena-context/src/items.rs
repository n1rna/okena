//! The items inside one root.
//!
//! Every item is addressed by its owner and a locator. A map entry's locator
//! is its map id (`area:core`); anything else's is its path relative to the
//! owner's *base* — the repository for a project, the store root for a store —
//! so a project's knowledge root and its OpenSpec root, which are different
//! directories, never produce the same locator.

use okena_core::context::{ContextItem, ContextKind, ContextOwner, ContextRef};
use okena_core::knowledge::KnowledgeKind;
use okena_core::project_map::{MapStatus, ProjectMapState};
use std::io::Read;
use std::path::{Path, PathBuf};

/// How much of a spec file is read for its description.
const HEAD_BYTES: u64 = 16 * 1024;

/// Who owns the items of a root, and where their locators are relative to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub owner: ContextOwner,
    /// The project's or store's name, shown beside every item.
    pub name: String,
    /// Locators are relative to this.
    pub base: PathBuf,
}

impl Owner {
    fn item(
        &self,
        kind: ContextKind,
        locator: String,
        title: String,
        description: String,
        path: &Path,
    ) -> ContextItem {
        ContextItem {
            reference: ContextRef {
                kind,
                owner: self.owner.clone(),
                locator,
            },
            title,
            description: one_line(&description),
            owner_name: self.name.clone(),
            path: display(path),
            map_id: None,
            chosen: false,
        }
    }

    fn locator(&self, path: &Path) -> String {
        rel(&self.base, path)
    }
}

/// A project map's entries, and whether the project has a map at all.
#[derive(Clone, Debug, PartialEq)]
pub struct MapItems {
    pub status: MapStatus,
    pub items: Vec<ContextItem>,
}

/// The entries of the map in `knowledge_root`: every area, concept, exposed
/// and consumed interface, pipeline and infrastructure resource. An invalid map
/// contributes nothing — its entries may be exactly what is wrong with it.
pub fn map_items(owner: &Owner, knowledge_root: &Path) -> MapItems {
    let state = okena_knowledge::project_map::load(knowledge_root);
    let status = MapStatus::from(&state);
    let ProjectMapState::Scanned { map } = state else {
        return MapItems {
            status,
            items: Vec::new(),
        };
    };
    let manifest = okena_knowledge::project_map::manifest_path(knowledge_root);
    // An entry's doc is where the prose about it is; without one, the
    // manifest itself is the thing to read.
    let path_of = |doc: Option<&str>| -> PathBuf {
        doc.map(|d| knowledge_root.join(d))
            .filter(|p| p.is_file())
            .unwrap_or_else(|| manifest.clone())
    };
    let mut items = Vec::new();
    let mut push = |id: String, title: &str, description: &str, path: PathBuf| {
        let mut item = owner.item(
            ContextKind::MapEntry,
            id.clone(),
            title.to_string(),
            description.to_string(),
            &path,
        );
        item.map_id = Some(id);
        items.push(item);
    };
    for area in &map.areas {
        push(
            format!("area:{}", area.id),
            area.label(),
            &area.description,
            path_of(area.doc.as_deref()),
        );
    }
    for concept in &map.concepts {
        push(
            format!("concept:{}", concept.id),
            concept.label(),
            &concept.description,
            path_of(concept.doc.as_deref()),
        );
    }
    for (section, list) in [("exposes", &map.exposes), ("consumes", &map.consumes)] {
        for interface in list {
            let description = interface.description.clone().unwrap_or_default();
            let description = if description.is_empty() {
                interface.kind.label().to_string()
            } else {
                format!("{} · {description}", interface.kind.label())
            };
            push(
                format!("{section}:{}", interface.name),
                &interface.name,
                &description,
                manifest.clone(),
            );
        }
    }
    for pipeline in &map.ci {
        push(
            format!("ci:{}", pipeline.name),
            &pipeline.name,
            pipeline.description.as_deref().unwrap_or_default(),
            manifest.clone(),
        );
    }
    for resource in &map.infrastructure {
        push(
            format!("infrastructure:{}", resource.name),
            &resource.name,
            resource.description.as_deref().unwrap_or_default(),
            manifest.clone(),
        );
    }
    MapItems { status, items }
}

/// The specs and active changes of the OpenSpec root at `root`. Archived
/// changes are history, not context.
pub fn spec_items(owner: &Owner, root: &Path) -> Vec<ContextItem> {
    let tree = okena_openspec::tree::read_tree(root);
    let mut items = Vec::new();
    for spec in &tree.specs {
        let path = root.join(&spec.path);
        items.push(owner.item(
            ContextKind::Spec,
            owner.locator(&path),
            spec.name.clone(),
            first_prose_line(&path),
            &path,
        ));
    }
    for change in &tree.changes {
        let dir = root.join(&change.path);
        let proposal = dir.join("proposal.md");
        items.push(owner.item(
            ContextKind::Spec,
            owner.locator(&dir),
            change.name.clone(),
            first_prose_line(&proposal),
            &dir,
        ));
    }
    items
}

/// The docs, skills and agents of the knowledge root at `root`. Templates are
/// okena's own prompts, not something to hand an agent.
pub fn knowledge_items(owner: &Owner, root: &Path) -> Vec<ContextItem> {
    okena_knowledge::tree::read_tree(root)
        .entries
        .into_iter()
        .filter_map(|entry| {
            let kind = match entry.kind {
                KnowledgeKind::Doc => ContextKind::Doc,
                KnowledgeKind::Skill => ContextKind::Skill,
                KnowledgeKind::Agent => ContextKind::Agent,
                KnowledgeKind::Template => return None,
            };
            let path = root.join(&entry.path);
            Some(owner.item(
                kind,
                owner.locator(&path),
                entry.title,
                entry.description.unwrap_or_default(),
                &path,
            ))
        })
        .collect()
}

/// The first line of prose in a Markdown file: not a heading, not blank, not
/// frontmatter. Empty when there is none or the file cannot be read.
fn first_prose_line(path: &Path) -> String {
    let mut head = String::new();
    let Ok(file) = std::fs::File::open(path) else {
        return String::new();
    };
    if file.take(HEAD_BYTES).read_to_string(&mut head).is_err() {
        // Cut mid-character: keep what decoded.
        head = String::from_utf8_lossy(head.as_bytes()).into_owned();
    }
    let body = okena_knowledge::frontmatter::split(&head)
        .map(|(_, body)| body)
        .unwrap_or(&head);
    body.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("---"))
        .unwrap_or_default()
        .to_string()
}

/// Collapse to one line, so a description never breaks a results row or a
/// brief's list.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Path relative to `base`, forward slashes.
pub fn rel(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

pub fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
pub(crate) mod fixtures {
    use std::path::Path;

    pub fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    pub const MAP: &str = "\
version: 1
project:
  name: shop
  description: The web shop.
areas:
  - id: checkout
    name: Checkout
    description: Takes payment for a basket.
    paths: [src/checkout]
    doc: docs/project/checkout.md
  - id: catalog
    description: Lists products.
    paths: [src/catalog]
concepts:
  - id: basket
    name: Basket
    description: What a customer is about to buy.
    areas: [checkout]
exposes:
  - type: http
    name: shop.example.com/api
    description: The public REST API.
    areas: [checkout]
consumes:
  - type: queue
    name: payments.events
ci:
  - name: build
    provider: github-actions
    description: Builds and tests every push.
    files: [.github/workflows/build.yml]
infrastructure:
  - name: orders-db
    kind: database
    description: Postgres holding orders.
    files: [infra/db.tf]
";

    /// A repository with a scanned map in its default knowledge root.
    pub fn mapped_repo(repo: &Path) {
        write(repo, ".okena/knowledge/project-map.yaml", MAP);
        write(
            repo,
            ".okena/knowledge/docs/project/checkout.md",
            "# Checkout\n\nHow payment works.\n",
        );
    }

    pub fn spec_root(root: &Path) {
        write(
            root,
            "openspec/specs/auth/spec.md",
            "# auth Specification\n\n## Purpose\nSigning users in and out.\n",
        );
        write(
            root,
            "openspec/changes/add-sso/proposal.md",
            "## Why\n\nCustomers want single sign-on.\n",
        );
        write(
            root,
            "openspec/changes/archive/2026-01-01-old/proposal.md",
            "## Why\n\nLong ago.\n",
        );
    }

    pub fn knowledge_store(root: &Path) {
        write(
            root,
            "docs/principles.md",
            "---\ntitle: Engineering principles\ndescription: How we build things.\n---\n# Principles\n",
        );
        write(
            root,
            "skills/release/SKILL.md",
            "---\nname: release\ndescription: Cut a release.\n---\n# Release\n",
        );
        write(
            root,
            "agents/reviewer.md",
            "---\nname: reviewer\ndescription: Reviews a diff.\n---\nYou review.\n",
        );
        write(
            root,
            "templates/task-start.md",
            "---\nfor: [task-start]\n---\nDo {title}\n",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    fn project(repo: &Path) -> Owner {
        Owner {
            owner: ContextOwner::project("p1"),
            name: "shop".into(),
            base: repo.to_path_buf(),
        }
    }

    #[test]
    fn a_scanned_map_yields_every_entry_with_its_id() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        mapped_repo(repo);
        let knowledge = repo.join(".okena/knowledge");
        let got = map_items(&project(repo), &knowledge);
        assert_eq!(got.status, MapStatus::Scanned);

        let ids: Vec<_> = got
            .items
            .iter()
            .map(|i| i.map_id.clone().unwrap())
            .collect();
        assert_eq!(
            ids,
            [
                "area:checkout",
                "area:catalog",
                "concept:basket",
                "exposes:shop.example.com/api",
                "consumes:payments.events",
                "ci:build",
                "infrastructure:orders-db",
            ]
        );
        let checkout = &got.items[0];
        assert_eq!(checkout.reference.kind, ContextKind::MapEntry);
        assert_eq!(checkout.reference.locator, "area:checkout");
        assert_eq!(checkout.title, "Checkout");
        assert_eq!(checkout.description, "Takes payment for a basket.");
        assert_eq!(checkout.owner_name, "shop");
        // Its doc under docs/project is the path handed over.
        assert_eq!(
            checkout.path,
            display(&knowledge.join("docs/project/checkout.md"))
        );
        // No doc: the manifest, and the id stands in for a missing name.
        let catalog = &got.items[1];
        assert_eq!(catalog.title, "catalog");
        assert_eq!(catalog.path, display(&knowledge.join("project-map.yaml")));
        assert_eq!(got.items[3].description, "HTTP API · The public REST API.");
        assert_eq!(got.items[4].description, "Queue");
    }

    #[test]
    fn an_invalid_map_contributes_no_entries() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            ".okena/knowledge/project-map.yaml",
            "version: 1\nareas: nonsense\n",
        );
        let got = map_items(&project(dir.path()), &dir.path().join(".okena/knowledge"));
        assert_eq!(got.status, MapStatus::Invalid);
        assert!(got.items.is_empty());
    }

    #[test]
    fn an_unscanned_project_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let got = map_items(&project(dir.path()), &dir.path().join(".okena/knowledge"));
        assert_eq!(got.status, MapStatus::NotScanned);
        assert!(got.items.is_empty());
    }

    #[test]
    fn a_spec_root_yields_specs_and_active_changes() {
        let dir = tempfile::tempdir().unwrap();
        spec_root(dir.path());
        let got = spec_items(&project(dir.path()), dir.path());
        let summary: Vec<_> = got
            .iter()
            .map(|i| {
                (
                    i.title.as_str(),
                    i.reference.locator.as_str(),
                    i.description.as_str(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "auth",
                    "openspec/specs/auth/spec.md",
                    "Signing users in and out."
                ),
                (
                    "add-sso",
                    "openspec/changes/add-sso",
                    "Customers want single sign-on."
                ),
            ]
        );
        assert!(got.iter().all(|i| i.reference.kind == ContextKind::Spec));
        assert_eq!(
            got[1].path,
            display(&dir.path().join("openspec/changes/add-sso"))
        );
    }

    #[test]
    fn a_knowledge_store_yields_docs_skills_and_agents_but_not_templates() {
        let dir = tempfile::tempdir().unwrap();
        knowledge_store(dir.path());
        let owner = Owner {
            owner: ContextOwner::store("store:acme"),
            name: "acme".into(),
            base: dir.path().to_path_buf(),
        };
        let got = knowledge_items(&owner, dir.path());
        let summary: Vec<_> = got
            .iter()
            .map(|i| {
                (
                    i.reference.kind,
                    i.reference.locator.as_str(),
                    i.title.as_str(),
                    i.description.as_str(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (
                    ContextKind::Doc,
                    "docs/principles.md",
                    "Engineering principles",
                    "How we build things."
                ),
                (
                    ContextKind::Skill,
                    "skills/release/SKILL.md",
                    "Release",
                    "Cut a release."
                ),
                (
                    ContextKind::Agent,
                    "agents/reviewer.md",
                    "reviewer",
                    "Reviews a diff."
                ),
            ]
        );
        assert!(got.iter().all(|i| i.owner_name == "acme"));
    }

    #[test]
    fn a_project_knowledge_root_locates_items_from_the_repository() {
        let dir = tempfile::tempdir().unwrap();
        mapped_repo(dir.path());
        let got = knowledge_items(&project(dir.path()), &dir.path().join(".okena/knowledge"));
        assert_eq!(
            got[0].reference.locator,
            ".okena/knowledge/docs/project/checkout.md"
        );
    }
}
