//! okena's own templates and partials, compiled in.
//!
//! These are ordinary knowledge-store files, not Rust strings, and they are
//! the same bytes [`materialize`] writes to disk. That is the point: "show me
//! the default so I can edit it" hands you the exact text that would otherwise
//! have been sent, rather than a documented approximation of it that drifts.

use super::{Flow, body_of};
use std::path::Path;

/// The raw file for `flow`, frontmatter and all.
pub const fn file(flow: Flow) -> &'static str {
    match flow {
        Flow::TaskStart => include_str!("templates/briefs/task-start.md"),
        Flow::TasksStart => include_str!("templates/briefs/tasks-start.md"),
        Flow::TaskVerify => include_str!("templates/briefs/task-verify.md"),
        Flow::TaskBreakDown => include_str!("templates/briefs/break-down.md"),
        Flow::TaskCoordinate => include_str!("templates/briefs/task-coordinate.md"),
        Flow::TasksCoordinate => include_str!("templates/briefs/tasks-coordinate.md"),
        Flow::TaskCreate => include_str!("templates/briefs/task-create.md"),
        Flow::TaskRefine => include_str!("templates/briefs/task-refine.md"),
        Flow::SpecDraft => include_str!("templates/briefs/spec-draft.md"),
        Flow::KnowledgeDraft => include_str!("templates/briefs/knowledge-draft.md"),
        Flow::DocumentRefine => include_str!("templates/briefs/doc-refine.md"),
        Flow::AgentSession => include_str!("templates/briefs/agent-session.md"),
        Flow::ExtensionBuild => include_str!("templates/briefs/extension-build.md"),
        Flow::ProjectScan => include_str!("templates/briefs/project-scan.md"),
        Flow::ProjectsScan => include_str!("templates/briefs/projects-scan.md"),
    }
}

/// okena's partials: text shared between briefs, or chosen between by code.
///
/// Every sentence okena says to an agent is in one of these or in a flow
/// template. Code decides *which* partial applies — a store or a folder, a
/// description or none — and never how it is worded.
pub const PARTIALS: &[(&str, &str)] = &[
    ("reporting", include_str!("templates/partials/reporting.md")),
    (
        "no-description",
        include_str!("templates/partials/no-description.md"),
    ),
    (
        "no-description-yet",
        include_str!("templates/partials/no-description-yet.md"),
    ),
    (
        "given-projects",
        include_str!("templates/partials/given-projects.md"),
    ),
    (
        "given-worktrees",
        include_str!("templates/partials/given-worktrees.md"),
    ),
    (
        "spec-store-note",
        include_str!("templates/partials/spec-store-note.md"),
    ),
    (
        "spec-folder-note",
        include_str!("templates/partials/spec-folder-note.md"),
    ),
    (
        "spec-references",
        include_str!("templates/partials/spec-references.md"),
    ),
    (
        "spec-reference",
        include_str!("templates/partials/spec-reference.md"),
    ),
    (
        "knowledge-commit-store",
        include_str!("templates/partials/knowledge-commit-store.md"),
    ),
    (
        "knowledge-commit-project",
        include_str!("templates/partials/knowledge-commit-project.md"),
    ),
    (
        "fan-out-note",
        include_str!("templates/partials/fan-out-note.md"),
    ),
    (
        "group-note",
        include_str!("templates/partials/group-note.md"),
    ),
    (
        "picked-fan-out-note",
        include_str!("templates/partials/picked-fan-out-note.md"),
    ),
    (
        "picked-group-note",
        include_str!("templates/partials/picked-group-note.md"),
    ),
    (
        "picked-sibling",
        include_str!("templates/partials/picked-sibling.md"),
    ),
    (
        "task-in-group",
        include_str!("templates/partials/task-in-group.md"),
    ),
    (
        "coordinate-child",
        include_str!("templates/partials/coordinate-child.md"),
    ),
    (
        "scan-update",
        include_str!("templates/partials/scan-update.md"),
    ),
    (
        "scan-repair",
        include_str!("templates/partials/scan-repair.md"),
    ),
    (
        "scan-from-docs",
        include_str!("templates/partials/scan-from-docs.md"),
    ),
    (
        "scan-from-code",
        include_str!("templates/partials/scan-from-code.md"),
    ),
    // Launch context (QBL-406): the heading over what was picked, and the
    // line naming what was loaded into the session instead of listed.
    ("context", include_str!("templates/partials/context.md")),
    (
        "context-installed",
        include_str!("templates/partials/context-installed.md"),
    ),
    (
        "context-lookup",
        include_str!("templates/partials/context-lookup.md"),
    ),
    // QBL-410: the picked items past the brief's context budget, by title.
    (
        "context-more",
        include_str!("templates/partials/context-more.md"),
    ),
];

/// The skill that tells an agent how to map a repository (ADR-0005).
pub const PROJECT_MAP_SKILL: &str = "project-map";

/// okena's skills: whole Agent Skills handed to an agent as they are, never
/// rendered into a brief. What to put in a skill is prose a team should be
/// able to change without a release, exactly like a template.
pub const SKILLS: &[(&str, &str)] = &[(
    PROJECT_MAP_SKILL,
    include_str!("skills/project-map/SKILL.md"),
)];

/// Where a store keeps skill `name`, relative to its root.
pub fn skill_path(name: &str) -> String {
    format!("skills/{name}/SKILL.md")
}

/// okena's built-in `SKILL.md` for `name`, frontmatter and all.
pub fn skill_file(name: &str) -> Option<&'static str> {
    SKILLS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, file)| *file)
}

/// The folder a root keeps its partials in, relative to the root.
///
/// Beside [`super::flows::BRIEFS_DIR`], and shown as a directory of its own in
/// the Knowledge view for the same reason (QBL-427).
pub const PARTIALS_DIR: &str = "templates/partials";

/// Where a store keeps partial `name`, relative to its root.
pub fn partial_path(name: &str) -> String {
    format!("{PARTIALS_DIR}/{name}.md")
}

/// The built-in body of partial `name`, frontmatter stripped.
pub fn partial_body(name: &str) -> Option<String> {
    PARTIALS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, file)| body_of(file))
}

/// The body okena renders for `flow`.
///
/// Leaked rather than cached per call: there are few of them, they never
/// change within a run, and the alternative is either a lock on every launch
/// or handing callers a `String` they have to keep alive.
pub fn body(flow: Flow) -> &'static str {
    use std::sync::OnceLock;
    static BODIES: OnceLock<Vec<String>> = OnceLock::new();
    let bodies = BODIES.get_or_init(|| Flow::all().iter().map(|f| body_of(file(*f))).collect());
    let index = Flow::all()
        .iter()
        .position(|f| *f == flow)
        .expect("every flow is in Flow::all");
    &bodies[index]
}

/// Every file okena manages in its defaults store: path and contents.
///
/// The identity and the README are in here with the templates because the
/// whole folder is okena's: a stale README describing behaviour okena no
/// longer has is exactly the kind of drift this store exists to prevent.
fn managed_files() -> Vec<(String, &'static str)> {
    Flow::all()
        .iter()
        .map(|f| (f.template_path(), file(*f)))
        .chain(PARTIALS.iter().map(|(n, f)| (partial_path(n), *f)))
        .chain(SKILLS.iter().map(|(n, f)| (skill_path(n), *f)))
        .chain([
            (IDENTITY_PATH.to_string(), IDENTITY),
            ("README.md".to_string(), README),
        ])
        .collect()
}

/// What a materialize run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Materialized {
    /// Files that did not exist and now do.
    pub written: Vec<String>,
    /// Files that were on disk saying something else, and now say what the
    /// built-in says.
    pub updated: Vec<String>,
    /// Files okena used to manage here and has now removed.
    pub removed: Vec<String>,
}

impl Materialized {
    pub fn changed_anything(&self) -> bool {
        !self.written.is_empty() || !self.updated.is_empty() || !self.removed.is_empty()
    }
}

/// Where okena used to record what it had written, relative to the root.
///
/// Nothing reads it now — okena rewrites every file regardless — so a run
/// deletes it rather than leaving a stale file that claims to mean something.
const STALE_RECORD_PATH: &str = ".okena-knowledge/defaults.lock";

/// Paths okena used to manage in this store and no longer does.
///
/// The briefs used to sit flat at `templates/<flow>.md`, beside the partials
/// (QBL-427). A file left behind there would be a brief no launch reads, shown
/// in the Knowledge view as though it were current — so a materialize removes
/// it. Only okena's own store is swept: an override of your own at the old
/// path simply stops applying, and stays where you put it.
fn retired_files() -> Vec<String> {
    Flow::all()
        .iter()
        .map(|f| format!("templates/{}.md", f.id()))
        .collect()
}

/// Write okena's templates, partials and skills into `root`, overwriting
/// whatever is there.
///
/// This folder is okena's, not yours (QBL-415). Every file is restored to the
/// built-in on every run, so what the Knowledge view shows under
/// `okena-defaults` is always the text a launch would actually use — a
/// guarantee that is worth more than the ability to edit a copy in place, and
/// that the old edit-detecting behaviour quietly broke: an edited default kept
/// its edit *and* stopped receiving okena's updates, so the file on screen was
/// neither yours nor okena's.
///
/// Changing a default means overriding it from a root of your own, which the
/// layered resolution in [`super`] then prefers.
pub fn materialize(root: &Path) -> std::io::Result<Materialized> {
    let mut report = Materialized::default();
    for (rel, contents) in managed_files() {
        let path = root.join(&rel);
        match std::fs::read_to_string(&path) {
            Ok(on_disk) if on_disk == contents => {}
            Ok(_) => {
                write_file(&path, contents)?;
                report.updated.push(rel);
            }
            Err(_) => {
                write_file(&path, contents)?;
                report.written.push(rel);
            }
        }
    }
    for rel in retired_files() {
        let path = root.join(&rel);
        if path.is_file() && std::fs::remove_file(&path).is_ok() {
            report.removed.push(rel);
        }
    }
    // Best-effort: a record okena cannot remove is only clutter, and failing
    // the whole run over it would cost the user their defaults.
    let _ = std::fs::remove_file(root.join(STALE_RECORD_PATH));
    Ok(report)
}

fn write_file(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)
}

// ─── The built-in store ──────────────────────────────────────────────────────

/// The store id okena's own defaults are registered under.
pub const DEFAULT_STORE_ID: &str = "okena-defaults";

/// Folder name the defaults are materialized into, under the knowledge folder.
pub const DEFAULT_STORE_DIR: &str = "okena-defaults";

/// Where a store keeps its identity, relative to its root.
const IDENTITY_PATH: &str = ".okena-knowledge/store.yaml";

/// The defaults store's identity.
const IDENTITY: &str = "version: 1\n\
     id: okena-defaults\n\
     name: okena defaults\n\
     description: The briefs okena launches agents with. Read-only — override one from a root of your own.\n";

/// Make sure okena's defaults exist on disk at `dir`, as a knowledge root.
///
/// Writes the identity, the templates, partials and skills, and restores any
/// that differ (see [`materialize`]). Not a git repository: it is a knowledge
/// root, which is enough to be listed, read and used.
pub fn ensure_store(dir: &Path) -> std::io::Result<Materialized> {
    std::fs::create_dir_all(dir)?;
    materialize(dir)
}

/// What somebody finds when they open the folder.
const README: &str = "\
# okena defaults

These are the briefs okena launches agents with, and this folder is okena's:
every file here is rewritten to match the build each time okena starts, so what
you read is always what a launch would actually send. Editing a file here does
nothing — your change is gone by the next start.

To change one, override it. Open it in Harness → Knowledge and press
**Override**, pick one of your own knowledge roots, and edit the copy there.
okena looks for each file in every root it can see — registered stores first,
then your projects' own knowledge folders — and uses the first one that has it,
falling back to the built-in. So you can override one brief without supplying
the rest, and deleting your copy restores okena's.

- `templates/briefs/<flow>.md` — the brief for one launch flow; frontmatter
  says which.
- `templates/partials/<name>.md` — text shared between briefs, included with
  `{>name}`, or used as `{value|name}` when a value is empty.
- `skills/<name>/SKILL.md` — skills okena hands an agent whole, such as
  `project-map`. Overriding one replaces the whole file.
";

#[cfg(test)]
mod tests {
    use super::{
        Flow, PARTIALS, body, ensure_store, file, managed_files, materialize, partial_path,
    };

    #[test]
    fn every_builtin_file_declares_the_flow_it_is_for() {
        for flow in Flow::all() {
            let (fm, _) = crate::frontmatter::parse(file(*flow));
            let declared = fm.list("for");
            assert!(
                declared.iter().any(|f| Flow::from_id(f) == Some(*flow)),
                "{flow}'s template declares {declared:?}"
            );
        }
    }

    #[test]
    fn the_body_is_the_file_without_its_frontmatter() {
        for flow in Flow::all() {
            let b = body(*flow);
            assert!(!b.starts_with("---"), "{flow} kept its frontmatter");
            assert!(!b.ends_with('\n'), "{flow} kept a trailing newline");
            assert!(!b.is_empty(), "{flow} is empty");
        }
    }

    #[test]
    fn every_partial_has_a_body_and_a_valid_name() {
        for (name, _) in PARTIALS {
            assert!(crate::prompts::render::is_partial_name(name), "{name}");
            assert!(
                super::partial_body(name).is_some_and(|b| !b.is_empty()),
                "{name} is empty"
            );
        }
    }

    #[test]
    fn every_include_in_a_builtin_resolves_to_a_builtin_partial() {
        // A default that renders with a visible `{>name}` in it would be okena
        // shipping a broken brief.
        let texts: Vec<&str> = Flow::all()
            .iter()
            .map(|f| file(*f))
            .chain(PARTIALS.iter().map(|(_, f)| *f))
            .collect();
        for text in texts {
            let mut rest = text;
            while let Some(i) = rest.find("{>") {
                let after = &rest[i + 2..];
                let end = after.find('}').expect("closed include");
                let name = &after[..end];
                assert!(
                    super::partial_body(name).is_some(),
                    "missing partial {name}"
                );
                rest = &after[end..];
            }
            let mut rest = text;
            while let Some(i) = rest.find('|') {
                let after = &rest[i + 1..];
                if let Some(end) = after.find('}') {
                    let name = &after[..end];
                    if crate::prompts::render::is_partial_name(name) {
                        assert!(
                            super::partial_body(name).is_some(),
                            "missing fallback {name}"
                        );
                    }
                }
                rest = after;
            }
        }
    }

    #[test]
    fn materialize_writes_every_managed_file_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = materialize(dir.path()).expect("write");
        assert_eq!(first.written.len(), managed_files().len());
        for flow in Flow::all() {
            assert!(dir.path().join(flow.template_path()).exists(), "{flow}");
        }
        assert!(dir.path().join(partial_path("reporting")).exists());
        let second = materialize(dir.path()).expect("write again");
        assert!(!second.changed_anything(), "{second:?}");
    }

    #[test]
    fn a_brief_left_at_the_old_flat_path_is_swept_away() {
        // The briefs moved under `templates/briefs/` (QBL-427). A copy left at
        // the path they used to have is a brief no launch reads, so okena's
        // own store must not keep showing it.
        let dir = tempfile::tempdir().expect("tempdir");
        materialize(dir.path()).expect("write");
        let old = dir.path().join("templates/spec-draft.md");
        std::fs::write(&old, file(Flow::SpecDraft)).expect("old-path copy");

        let again = materialize(dir.path()).expect("write again");
        assert!(!old.exists(), "the old flat path was left on disk");
        assert_eq!(again.removed, ["templates/spec-draft.md"]);
        assert!(again.written.is_empty() && again.updated.is_empty(), "{again:?}");
        // The brief itself is where it belongs, and nothing else moved.
        assert!(
            dir.path()
                .join(Flow::SpecDraft.template_path())
                .starts_with(dir.path().join("templates/briefs"))
        );
        assert!(dir.path().join("templates/briefs/spec-draft.md").is_file());
        assert!(dir.path().join(partial_path("reporting")).is_file());
    }

    #[test]
    fn materialize_restores_an_edited_default() {
        // The rule QBL-415 replaced the record with: this folder is okena's,
        // so an edit to it does not survive the next start.
        let dir = tempfile::tempdir().expect("tempdir");
        materialize(dir.path()).expect("write");
        let rel = Flow::SpecDraft.template_path();
        let path = dir.path().join(&rel);
        std::fs::write(&path, "our own words").expect("edit");

        let again = materialize(dir.path()).expect("write again");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            file(Flow::SpecDraft),
            "an edited default was left as the user wrote it"
        );
        assert_eq!(again.updated, std::slice::from_ref(&rel));
        assert!(again.written.is_empty(), "{again:?}");
    }

    #[test]
    fn every_managed_file_is_restored_whatever_state_it_was_left_in() {
        // Deleted, emptied, edited, or a file from a build that predates the
        // record: each one comes back, and nothing else is touched.
        let dir = tempfile::tempdir().expect("tempdir");
        materialize(dir.path()).expect("write");
        let deleted = Flow::TaskVerify.template_path();
        let emptied = partial_path("reporting");
        let edited = super::skill_path(super::PROJECT_MAP_SKILL);
        std::fs::remove_file(dir.path().join(&deleted)).expect("rm");
        std::fs::write(dir.path().join(&emptied), "").expect("empty");
        std::fs::write(dir.path().join(&edited), "ours").expect("edit");
        // A lock file from before QBL-415, which nothing reads now.
        let stale = dir.path().join(super::STALE_RECORD_PATH);
        std::fs::write(&stale, "deadbeef templates/task-start.md\n").expect("stale record");

        let report = materialize(dir.path()).expect("restore");
        assert_eq!(report.written, std::slice::from_ref(&deleted));
        assert_eq!(report.updated, [emptied.clone(), edited.clone()]);
        assert_eq!(
            std::fs::read_to_string(dir.path().join(&deleted)).expect("read"),
            file(Flow::TaskVerify)
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(&edited)).expect("read"),
            super::skill_file(super::PROJECT_MAP_SKILL).expect("skill")
        );
        assert!(!stale.exists(), "the dead record was left behind");

        // And a second run has nothing left to do.
        assert!(
            !materialize(dir.path()).expect("again").changed_anything(),
            "restoring is not idempotent"
        );
    }

    #[test]
    fn the_identity_and_readme_are_okenas_too() {
        let dir = tempfile::tempdir().expect("tempdir");
        ensure_store(dir.path()).expect("create");
        let identity = dir.path().join(super::IDENTITY_PATH);
        std::fs::write(&identity, "version: 1\nid: not-okenas\n").expect("edit");
        std::fs::write(dir.path().join("README.md"), "mine").expect("edit");

        ensure_store(dir.path()).expect("restore");
        let restored = std::fs::read_to_string(&identity).expect("identity");
        assert!(restored.contains(super::DEFAULT_STORE_ID), "{restored}");
        assert!(
            std::fs::read_to_string(dir.path().join("README.md"))
                .expect("readme")
                .contains("**Override**"),
            "the README still described editing in place"
        );
    }

    #[test]
    fn every_builtin_skill_names_itself_and_says_what_it_is_for() {
        // Agent Skills are picked by `name` and `description`; a skill without
        // them is a file an agent never loads.
        for (name, file) in super::SKILLS {
            let (yaml, _) = crate::frontmatter::split(file).expect("frontmatter");
            let fields: serde_yaml_ng::Value = serde_yaml_ng::from_str(yaml).expect("yaml");
            assert_eq!(fields["name"].as_str(), Some(*name));
            assert!(
                fields["description"]
                    .as_str()
                    .is_some_and(|d| !d.is_empty()),
                "{name} has no description"
            );
        }
    }

    #[test]
    fn the_project_map_skills_example_is_a_valid_manifest() {
        // The example is what an agent copies; okena must accept it.
        let file = super::skill_file(super::PROJECT_MAP_SKILL).expect("skill");
        let start = file.find("```yaml\n").expect("yaml example") + "```yaml\n".len();
        let end = start + file[start..].find("\n```").expect("closed example");
        let map = crate::project_map::parse(&file[start..end], "SKILL.md example")
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!map.areas.is_empty() && !map.concepts.is_empty());
        assert!(!map.exposes.is_empty() && !map.consumes.is_empty());
        assert!(!map.ci.is_empty() && !map.infrastructure.is_empty());
        assert!(map.scanned.is_some());
    }

    #[test]
    fn materialize_writes_the_skills_where_a_store_lists_them() {
        let dir = tempfile::tempdir().expect("tempdir");
        materialize(dir.path()).expect("write");
        let rel = super::skill_path(super::PROJECT_MAP_SKILL);
        assert_eq!(
            std::fs::read_to_string(dir.path().join(&rel)).expect("read"),
            super::skill_file(super::PROJECT_MAP_SKILL).expect("skill")
        );
        let tree = crate::tree::read_tree(dir.path());
        assert!(
            tree.entries
                .iter()
                .any(|e| e.path == rel && e.kind == okena_core::knowledge::KnowledgeKind::Skill),
            "{:?}",
            tree.entries.iter().map(|e| &e.path).collect::<Vec<_>>()
        );
    }

    #[test]
    fn ensure_store_makes_a_readable_knowledge_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        ensure_store(dir.path()).expect("create");
        let identity = std::fs::read_to_string(dir.path().join(".okena-knowledge/store.yaml"))
            .expect("identity");
        assert!(identity.contains(super::DEFAULT_STORE_ID), "{identity}");
        assert!(crate::tree::has_kind_folder(dir.path()));
    }
}
