//! What a project scan starts from, and where it writes.
//!
//! The `project-scan` brief says one of four things about a repository's
//! starting point, and which one applies is a fact okena can check: whether a
//! map is there and valid, and if there is none, whether the repository
//! already describes itself. Code decides the case; the words are the
//! template's partials (ADR-0005).

use crate::project_map::{self, manifest_path};
use crate::{KnowledgeError, project};
use okena_core::project_map::ProjectMapState;
use std::path::{Path, PathBuf};

/// What a repository keeps about itself at its root, and a first scan starts
/// from rather than from the code alone. A trailing `/` is a folder.
pub const SELF_DESCRIPTIONS: &[&str] = &["CLAUDE.md", "AGENTS.md", "docs/"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanStart {
    /// A valid map exists: bring it up to date.
    Update,
    /// A manifest exists but cannot be used, for these reasons.
    Repair { problems: Vec<String> },
    /// No map, but the repository describes itself in these, relative to its
    /// root.
    FromDocs { docs: Vec<String> },
    /// No map, and nothing describing the repository.
    FromCode,
}

impl ScanStart {
    /// The partial that words this case in the brief.
    pub const fn partial(&self) -> &'static str {
        match self {
            ScanStart::Update => "scan-update",
            ScanStart::Repair { .. } => "scan-repair",
            ScanStart::FromDocs { .. } => "scan-from-docs",
            ScanStart::FromCode => "scan-from-code",
        }
    }

    /// A stable id for the case, for clients and logs.
    pub const fn id(&self) -> &'static str {
        match self {
            ScanStart::Update => "update",
            ScanStart::Repair { .. } => "repair",
            ScanStart::FromDocs { .. } => "from_docs",
            ScanStart::FromCode => "from_code",
        }
    }
}

/// Where a scan of one repository writes, and what it starts from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanPlan {
    /// The knowledge root the map goes into. It may not exist yet.
    pub map_root: PathBuf,
    pub manifest: PathBuf,
    pub start: ScanStart,
}

/// Plan a scan of the repository at `repo`.
///
/// Refused only when the map's location cannot be known — an unreadable or
/// invalid `.okena/knowledge.yaml`, or a `root:` outside the repository —
/// because an agent told to write "somewhere" picks a place okena never reads.
pub fn plan(repo: &Path) -> Result<ScanPlan, KnowledgeError> {
    let map_root = map_root(repo)?;
    let start = match project_map::load(&map_root) {
        ProjectMapState::Scanned { .. } => ScanStart::Update,
        ProjectMapState::Invalid { problems } => ScanStart::Repair {
            problems: problems.into_iter().map(|d| d.message).collect(),
        },
        ProjectMapState::NotScanned => {
            let docs: Vec<String> = SELF_DESCRIPTIONS
                .iter()
                .filter(|name| exists(repo, name))
                .map(|name| name.to_string())
                .collect();
            if docs.is_empty() {
                ScanStart::FromCode
            } else {
                ScanStart::FromDocs { docs }
            }
        }
    };
    Ok(ScanPlan {
        manifest: manifest_path(&map_root),
        map_root,
        start,
    })
}

/// Where `repo`'s map goes: its knowledge root, whether or not that exists yet.
///
/// The same root discovery lists, so the map shows up in Harness → Knowledge
/// once written.
pub fn map_root(repo: &Path) -> Result<PathBuf, KnowledgeError> {
    let config = project::read_config(repo)?;
    // Checks `root:` stays inside the repository, lexically and — when the
    // folder exists — after symlinks resolve.
    if let Some(root) = project::project_root(repo, config.as_ref())? {
        return Ok(root);
    }
    let rel = config
        .as_ref()
        .and_then(|c| c.root.as_deref())
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .unwrap_or(project::DEFAULT_PROJECT_ROOT);
    Ok(repo.join(rel))
}

fn exists(repo: &Path, name: &str) -> bool {
    match name.strip_suffix('/') {
        Some(dir) => repo.join(dir).is_dir(),
        None => repo.join(name).is_file(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::write;

    const MAP: &str = "version: 1\nproject:\n  name: api\n  description: The API.\n";

    #[test]
    fn a_bare_repository_is_mapped_from_its_code_into_the_default_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(&dir.path().join("src/main.rs"), "fn main() {}\n");
        let plan = plan(dir.path()).expect("plan");
        assert_eq!(plan.start, ScanStart::FromCode);
        assert_eq!(
            plan.map_root,
            dir.path().join(project::DEFAULT_PROJECT_ROOT)
        );
        assert_eq!(
            plan.manifest,
            dir.path().join(".okena/knowledge/project-map.yaml")
        );
    }

    #[test]
    fn a_repository_that_describes_itself_is_mapped_from_those_docs() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(&dir.path().join("CLAUDE.md"), "# Notes\n");
        write(&dir.path().join("docs/architecture.md"), "# Arch\n");
        // A folder named like a file, or a file named like a folder, is not it.
        std::fs::create_dir_all(dir.path().join("AGENTS.md")).expect("mkdir");
        assert_eq!(
            plan(dir.path()).expect("plan").start,
            ScanStart::FromDocs {
                docs: vec!["CLAUDE.md".into(), "docs/".into()]
            }
        );
    }

    #[test]
    fn a_valid_map_is_updated_and_an_invalid_one_repaired() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(&dir.path().join("CLAUDE.md"), "# Notes\n");
        let manifest = dir.path().join(".okena/knowledge/project-map.yaml");

        write(&manifest, MAP);
        assert_eq!(plan(dir.path()).expect("plan").start, ScanStart::Update);

        write(&manifest, "version: 1\nproject: [\n");
        match plan(dir.path()).expect("plan").start {
            ScanStart::Repair { problems } => {
                assert_eq!(problems.len(), 1);
                assert!(
                    problems[0].contains("not a valid project map"),
                    "{problems:?}"
                );
            }
            other => panic!("expected a repair, got {other:?}"),
        }
    }

    #[test]
    fn a_configured_root_is_where_the_map_goes_even_before_it_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            &dir.path().join(project::PROJECT_CONFIG),
            "root: docs/knowledge\n",
        );
        let plan = plan(dir.path()).expect("plan");
        assert_eq!(plan.map_root, dir.path().join("docs/knowledge"));
        assert_eq!(plan.start, ScanStart::FromCode);
    }

    #[test]
    fn a_scan_is_refused_when_the_map_has_nowhere_known_to_go() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(&dir.path().join(project::PROJECT_CONFIG), "stores: acme\n");
        assert_eq!(
            plan(dir.path()).map_err(|e| e.code),
            Err("project_config_invalid")
        );
        write(
            &dir.path().join(project::PROJECT_CONFIG),
            "root: ../elsewhere\n",
        );
        assert_eq!(
            plan(dir.path()).map_err(|e| e.code),
            Err("project_root_outside")
        );
    }

    #[test]
    fn every_case_is_worded_by_a_builtin_partial() {
        for start in [
            ScanStart::Update,
            ScanStart::Repair {
                problems: Vec::new(),
            },
            ScanStart::FromDocs { docs: Vec::new() },
            ScanStart::FromCode,
        ] {
            assert!(
                crate::prompts::defaults::partial_body(start.partial()).is_some(),
                "no partial {}",
                start.partial()
            );
        }
    }
}
