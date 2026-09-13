//! Project map wire types.
//!
//! A project map says what one repository *is*: the areas its code is divided
//! into, the concepts those areas implement, what it exposes to other projects
//! and consumes from them, and how it is built and run. An agent writes it into
//! the repo's knowledge root and the repo commits it; okena only reads it
//! (ADR-0005, `docs/reference/project-map.md`):
//!
//! ```text
//! <knowledge root>/
//! ├── project-map.yaml       the manifest these types are parsed from
//! └── docs/project/**/*.md   the same map as prose, ordinary knowledge docs
//! ```
//!
//! Reading and validation live in `okena-knowledge`. These are only the shapes
//! that cross the wire.

use serde::{Deserialize, Serialize};

pub use crate::diagnostic::{Diagnostic, Severity};

/// One repository's `project-map.yaml`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectMap {
    pub version: u32,
    pub project: ProjectSummary,
    /// The commit the map describes. Absent means nobody recorded it, so
    /// whether the map is stale is unknown — not that the map is invalid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanned: Option<ScanStamp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub areas: Vec<Area>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub concepts: Vec<Concept>,
    /// What other projects can use from this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exposes: Vec<Interface>,
    /// What this project uses from elsewhere.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumes: Vec<Interface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ci: Vec<Pipeline>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub infrastructure: Vec<InfraResource>,
    /// Links to other projects, written on both sides of each (ADR-0006).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
}

impl ProjectMap {
    pub fn area(&self, id: &str) -> Option<&Area> {
        self.areas.iter().find(|a| a.id == id)
    }

    pub fn concept(&self, id: &str) -> Option<&Concept> {
        self.concepts.iter().find(|c| c.id == id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub name: String,
    /// What the project is for, in a line.
    pub description: String,
    /// The overview doc, relative to the knowledge root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

/// Which commit a map was written against, and when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanStamp {
    /// A full or abbreviated commit hash.
    pub commit: String,
    /// When, as the agent wrote it (ISO 8601).
    pub at: String,
}

/// A boundary in the code: a module, package, service or app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Area {
    /// Kebab-case, unique among areas. What concepts and interfaces name it by.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub description: String,
    /// Paths or globs relative to the repository root.
    pub paths: Vec<String>,
    /// The area's own doc, relative to the knowledge root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

impl Area {
    /// What to show: the name, else the id.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

/// A domain idea or feature, and the areas that implement it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Concept {
    /// Kebab-case, unique among concepts.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub description: String,
    /// Ids of the areas that implement it.
    pub areas: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

impl Concept {
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

/// What kind of thing crosses a project boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterfaceKind {
    Http,
    Grpc,
    Graphql,
    Package,
    Queue,
    Topic,
    Database,
    Infra,
}

impl InterfaceKind {
    pub const fn all() -> [InterfaceKind; 8] {
        [
            InterfaceKind::Http,
            InterfaceKind::Grpc,
            InterfaceKind::Graphql,
            InterfaceKind::Package,
            InterfaceKind::Queue,
            InterfaceKind::Topic,
            InterfaceKind::Database,
            InterfaceKind::Infra,
        ]
    }

    /// The `type:` value in a manifest.
    pub const fn id(self) -> &'static str {
        match self {
            InterfaceKind::Http => "http",
            InterfaceKind::Grpc => "grpc",
            InterfaceKind::Graphql => "graphql",
            InterfaceKind::Package => "package",
            InterfaceKind::Queue => "queue",
            InterfaceKind::Topic => "topic",
            InterfaceKind::Database => "database",
            InterfaceKind::Infra => "infra",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            InterfaceKind::Http => "HTTP API",
            InterfaceKind::Grpc => "gRPC service",
            InterfaceKind::Graphql => "GraphQL API",
            InterfaceKind::Package => "Package",
            InterfaceKind::Queue => "Queue",
            InterfaceKind::Topic => "Topic",
            InterfaceKind::Database => "Database",
            InterfaceKind::Infra => "Infrastructure",
        }
    }
}

/// One thing a project exposes or consumes.
///
/// Two projects are linked when one's `consumes` and the other's `exposes`
/// carry the same `kind` and `name`, so `name` is the identifier the provider
/// publishes — a host and base path, a proto service, a package name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interface {
    #[serde(rename = "type")]
    pub kind: InterfaceKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Ids of the areas that serve or use it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub areas: Vec<String>,
}

/// A CI/CD pipeline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pipeline {
    pub name: String,
    /// e.g. `github-actions`, `gitlab-ci`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The files that define it, relative to the repository root.
    pub files: Vec<String>,
}

/// A resource the project needs to run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InfraResource {
    pub name: String,
    /// e.g. `database`, `cache`, `bucket`, `cluster`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The files that define or configure it, relative to the repository root.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

/// Which way a link points, seen from the manifest it is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkDirection {
    /// This project uses the other one.
    Uses,
    /// The other project uses this one.
    UsedBy,
}

impl LinkDirection {
    /// The `direction:` value in a manifest.
    pub const fn id(self) -> &'static str {
        match self {
            LinkDirection::Uses => "uses",
            LinkDirection::UsedBy => "used_by",
        }
    }
}

/// A link to another project, as one of its two manifests records it.
///
/// Written on both sides (ADR-0006): `uses` in the map of the project that
/// uses the other, `used_by` in the other's, with the same `type` and `name`,
/// so each project's map says on its own what it is connected to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    /// The other project, by the `project.name` of its map.
    pub project: String,
    pub direction: LinkDirection,
    #[serde(rename = "type")]
    pub kind: InterfaceKind,
    /// The interface the link runs through, named as its provider publishes it.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Where okena learned of a link between two projects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkSource {
    /// One project's `consumes` matches another's `exposes`, and neither map
    /// lists the link.
    Matched,
    /// Matched, and listed under `links`.
    Confirmed,
    /// Listed under `links` with no matching `exposes` and `consumes` —
    /// typically written by a multi-project scan.
    FoundByScan,
}

impl LinkSource {
    pub const fn label(self) -> &'static str {
        match self {
            LinkSource::Matched => "matched",
            LinkSource::Confirmed => "confirmed",
            LinkSource::FoundByScan => "found by scan",
        }
    }
}

/// Whether a project has a map to read links from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapStatus {
    NotScanned,
    Scanned,
    Invalid,
}

impl From<&ProjectMapState> for MapStatus {
    fn from(state: &ProjectMapState) -> Self {
        match state {
            ProjectMapState::NotScanned => MapStatus::NotScanned,
            ProjectMapState::Scanned { .. } => MapStatus::Scanned,
            ProjectMapState::Invalid { .. } => MapStatus::Invalid,
        }
    }
}

/// One okena project, as the link matcher saw it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedProject {
    pub project_id: String,
    /// The okena project's name.
    pub name: String,
    pub status: MapStatus,
    /// Its map's `project.name`, which other maps' links name it by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_name: Option<String>,
}

/// `consumer` uses `provider` through one interface. Both are okena project
/// ids.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectLink {
    pub consumer: String,
    pub provider: String,
    #[serde(rename = "type")]
    pub kind: InterfaceKind,
    pub name: String,
    pub source: LinkSource,
    /// The one project whose map lists the link, when only one does. A link
    /// belongs in both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed_only_by: Option<String>,
}

/// Something a project consumes that no scanned project exposes — often a
/// project not scanned yet rather than a mistake.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnmatchedConsume {
    pub project: String,
    pub interface: Interface,
}

/// A `links` entry naming no scanned project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnresolvedLink {
    pub project: String,
    pub link: Link,
}

/// The links between every scanned project okena knows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectLinks {
    #[serde(default)]
    pub projects: Vec<LinkedProject>,
    #[serde(default)]
    pub links: Vec<ProjectLink>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unmatched: Vec<UnmatchedConsume>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved: Vec<UnresolvedLink>,
}

impl ProjectLinks {
    pub fn project(&self, id: &str) -> Option<&LinkedProject> {
        self.projects.iter().find(|p| p.project_id == id)
    }

    /// Links on which project `id` uses another.
    pub fn uses<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a ProjectLink> {
        self.links.iter().filter(move |l| l.consumer == id)
    }

    /// Links on which another project uses project `id`.
    pub fn used_by<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a ProjectLink> {
        self.links.iter().filter(move |l| l.provider == id)
    }
}

/// What reading a project's map found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProjectMapState {
    /// There is no `project-map.yaml`.
    NotScanned,
    Scanned {
        map: Box<ProjectMap>,
    },
    /// A manifest exists and could not be used; every problem found, in file
    /// order where there is one.
    Invalid {
        problems: Vec<Diagnostic>,
    },
}

/// What a map read replies: the map's state, and the knowledge root it lives
/// in, so a client can open the map's docs there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectMapReport {
    /// The knowledge root's key as discovery gives it (`path:<root>`), for
    /// reading the map's docs. `None` when the project has no knowledge root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_key: Option<String>,
    #[serde(flatten)]
    pub state: ProjectMapState,
}

impl ProjectMapState {
    pub fn map(&self) -> Option<&ProjectMap> {
        match self {
            ProjectMapState::Scanned { map } => Some(map),
            _ => None,
        }
    }

    /// The problem to lead with when the map is invalid.
    pub fn problem(&self) -> Option<&Diagnostic> {
        match self {
            ProjectMapState::Invalid { problems } => problems.first(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> ProjectMap {
        ProjectMap {
            version: 1,
            project: ProjectSummary {
                name: "api".into(),
                description: "The public API".into(),
                doc: Some("docs/project/overview.md".into()),
            },
            scanned: Some(ScanStamp {
                commit: "3f9a2c1".into(),
                at: "2026-09-13T10:00:00Z".into(),
            }),
            areas: vec![Area {
                id: "billing".into(),
                name: Some("Billing".into()),
                description: "Invoices".into(),
                paths: vec!["src/billing/**".into()],
                doc: None,
            }],
            concepts: vec![Concept {
                id: "invoice".into(),
                name: None,
                description: "A bill".into(),
                areas: vec!["billing".into()],
                doc: None,
            }],
            exposes: vec![Interface {
                kind: InterfaceKind::Http,
                name: "api.acme.com/v1".into(),
                description: None,
                areas: vec!["billing".into()],
            }],
            consumes: vec![Interface {
                kind: InterfaceKind::Package,
                name: "@acme/money".into(),
                description: Some("Money maths".into()),
                areas: Vec::new(),
            }],
            ci: vec![Pipeline {
                name: "test".into(),
                provider: Some("github-actions".into()),
                description: None,
                files: vec![".github/workflows/test.yml".into()],
            }],
            infrastructure: vec![InfraResource {
                name: "postgres".into(),
                kind: Some("database".into()),
                description: None,
                files: Vec::new(),
            }],
            links: vec![Link {
                project: "accounts".into(),
                direction: LinkDirection::Uses,
                kind: InterfaceKind::Grpc,
                name: "acme.accounts.v1.AccountService".into(),
                description: None,
            }],
        }
    }

    #[test]
    fn links_round_trip_and_answer_by_side() {
        let links = ProjectLinks {
            projects: vec![LinkedProject {
                project_id: "p1".into(),
                name: "api".into(),
                status: MapStatus::Scanned,
                map_name: Some("api".into()),
            }],
            links: vec![ProjectLink {
                consumer: "p1".into(),
                provider: "p2".into(),
                kind: InterfaceKind::Topic,
                name: "billing.invoice-issued".into(),
                source: LinkSource::FoundByScan,
                listed_only_by: Some("p1".into()),
            }],
            unmatched: Vec::new(),
            unresolved: Vec::new(),
        };
        let json = serde_json::to_value(&links).expect("json");
        assert_eq!(json["links"][0]["type"], "topic");
        assert_eq!(json["links"][0]["source"], "found_by_scan");
        assert_eq!(
            serde_json::from_value::<ProjectLinks>(json).expect("decode"),
            links
        );
        assert_eq!(links.uses("p1").count(), 1);
        assert_eq!(links.used_by("p2").count(), 1);
        assert_eq!(links.used_by("p1").count(), 0);
        assert_eq!(
            serde_json::to_value(LinkDirection::UsedBy).expect("json"),
            "used_by"
        );
    }

    #[test]
    fn states_round_trip() {
        for state in [
            ProjectMapState::NotScanned,
            ProjectMapState::Scanned {
                map: Box::new(map()),
            },
            ProjectMapState::Invalid {
                problems: vec![Diagnostic::error("project_map_invalid", "m").with_fix("f")],
            },
        ] {
            let json = serde_json::to_string(&state).expect("encode");
            assert_eq!(
                serde_json::from_str::<ProjectMapState>(&json).expect("decode"),
                state
            );
        }
    }

    #[test]
    fn a_state_is_tagged_so_a_client_can_switch_on_it() {
        assert_eq!(
            serde_json::to_value(ProjectMapState::NotScanned).expect("json"),
            serde_json::json!({ "state": "not_scanned" })
        );
        let invalid = ProjectMapState::Invalid {
            problems: vec![Diagnostic::error("project_map_invalid", "m")],
        };
        assert_eq!(
            invalid.problem().map(|d| d.code.as_str()),
            Some("project_map_invalid")
        );
        assert!(invalid.map().is_none());
    }

    #[test]
    fn a_report_keeps_the_state_tag_at_the_top_beside_its_root() {
        let report = ProjectMapReport {
            root_key: Some("path:/p/api/.okena/knowledge".into()),
            state: ProjectMapState::Scanned {
                map: Box::new(map()),
            },
        };
        let json = serde_json::to_value(&report).expect("json");
        assert_eq!(json["state"], "scanned");
        assert_eq!(json["root_key"], "path:/p/api/.okena/knowledge");
        assert_eq!(
            serde_json::from_value::<ProjectMapReport>(json).expect("decode"),
            report
        );
        // A bare state, as a reply without a root, still decodes.
        let bare: ProjectMapReport =
            serde_json::from_value(serde_json::json!({ "state": "not_scanned" })).expect("decode");
        assert_eq!(bare.root_key, None);
        assert_eq!(bare.state, ProjectMapState::NotScanned);
    }

    #[test]
    fn an_interface_kind_serializes_as_its_manifest_id() {
        // `id()` is what the docs and the skill tell an agent to write.
        for kind in InterfaceKind::all() {
            assert_eq!(
                serde_json::to_value(kind).expect("json"),
                serde_json::json!(kind.id())
            );
        }
    }

    #[test]
    fn lookups_and_labels() {
        let map = map();
        assert_eq!(map.area("billing").map(Area::label), Some("Billing"));
        assert_eq!(map.concept("invoice").map(Concept::label), Some("invoice"));
        assert!(map.area("missing").is_none());
    }
}
