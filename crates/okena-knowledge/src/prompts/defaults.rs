//! okena's own templates and partials, compiled in.
//!
//! These are ordinary knowledge-store files, not Rust strings, and they are
//! the same bytes [`materialize`] writes to disk. That is the point: "show me
//! the default so I can edit it" hands you the exact text that would otherwise
//! have been sent, rather than a documented approximation of it that drifts.

use super::{Flow, body_of};
use std::collections::BTreeMap;
use std::path::Path;

/// The raw file for `flow`, frontmatter and all.
pub const fn file(flow: Flow) -> &'static str {
    match flow {
        Flow::TaskStart => include_str!("templates/task-start.md"),
        Flow::TaskBreakDown => include_str!("templates/break-down.md"),
        Flow::TaskCoordinate => include_str!("templates/task-coordinate.md"),
        Flow::TaskCreate => include_str!("templates/task-create.md"),
        Flow::SpecDraft => include_str!("templates/spec-draft.md"),
        Flow::KnowledgeDraft => include_str!("templates/knowledge-draft.md"),
        Flow::AgentSession => include_str!("templates/agent-session.md"),
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
        "coordinate-child",
        include_str!("templates/partials/coordinate-child.md"),
    ),
];

/// Where a store keeps partial `name`, relative to its root.
pub fn partial_path(name: &str) -> String {
    format!("templates/partials/{name}.md")
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
fn managed_files() -> Vec<(String, &'static str)> {
    Flow::all()
        .iter()
        .map(|f| (f.template_path(), file(*f)))
        .chain(PARTIALS.iter().map(|(n, f)| (partial_path(n), *f)))
        .collect()
}

/// What a materialize run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Materialized {
    /// Files that did not exist and now do.
    pub written: Vec<String>,
    /// Files okena had written, you had not touched, and okena has since
    /// changed — brought up to date.
    pub updated: Vec<String>,
    /// Files that differ from the default because you changed them. Left as
    /// they are.
    pub customized: Vec<String>,
}

impl Materialized {
    pub fn changed_anything(&self) -> bool {
        !self.written.is_empty() || !self.updated.is_empty()
    }
}

/// Where okena records what it last wrote, relative to the root.
const RECORD_PATH: &str = ".okena-knowledge/defaults.lock";

/// Write okena's templates and partials into `root`, keeping them current.
///
/// The rule that makes this safe to run on every start: okena only ever
/// replaces a file it can prove it wrote and you have not edited since. It
/// records a hash of each file as it writes it; a file whose contents still
/// match that hash is okena's to update when the default changes, and a file
/// that no longer matches is yours and is left alone.
///
/// A file with no record — a store from before the record existed — is
/// adopted if it already matches the default and otherwise treated as edited,
/// because okena cannot tell an old default from your change.
pub fn materialize(root: &Path) -> std::io::Result<Materialized> {
    let mut record = read_record(root);
    let mut report = Materialized::default();

    for (rel, contents) in managed_files() {
        let path = root.join(&rel);
        let current = std::fs::read_to_string(&path).ok();
        match current {
            None => {
                write_file(&path, contents)?;
                record.insert(rel.clone(), fingerprint(contents));
                report.written.push(rel);
            }
            Some(on_disk) if on_disk == contents => {
                record.insert(rel, fingerprint(contents));
            }
            Some(on_disk) => match record.get(&rel) {
                // Unedited since okena wrote it, and the default has moved on.
                Some(hash) if *hash == fingerprint(&on_disk) => {
                    write_file(&path, contents)?;
                    record.insert(rel.clone(), fingerprint(contents));
                    report.updated.push(rel);
                }
                _ => report.customized.push(rel),
            },
        }
    }
    write_record(root, &record)?;
    Ok(report)
}

fn write_file(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)
}

/// FNV-1a, 64-bit. Not a security boundary — it only has to notice that a
/// file changed, and it keeps this crate free of a hashing dependency.
fn fingerprint(contents: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in contents.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn read_record(root: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(root.join(RECORD_PATH))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (hash, path) = line.split_once(' ')?;
            Some((path.to_string(), hash.to_string()))
        })
        .collect()
}

fn write_record(root: &Path, record: &BTreeMap<String, String>) -> std::io::Result<()> {
    let mut out = String::from(
        "# Written by okena. Which default files it wrote, and what they held,\n\
         # so it can update the ones you have not edited. Safe to delete.\n",
    );
    for (path, hash) in record {
        out.push_str(&format!("{hash} {path}\n"));
    }
    write_file(&root.join(RECORD_PATH), &out)
}

// ─── The built-in store ──────────────────────────────────────────────────────

/// The store id okena's own defaults are registered under.
pub const DEFAULT_STORE_ID: &str = "okena-defaults";

/// Folder name the defaults are materialized into, under the knowledge folder.
pub const DEFAULT_STORE_DIR: &str = "okena-defaults";

/// Make sure okena's defaults exist on disk at `dir`, as a knowledge root.
///
/// Writes an identity, the templates and partials, and keeps unedited ones
/// current (see [`materialize`]). Not a git repository: it is a knowledge
/// root, which is enough to be listed, read and used.
pub fn ensure_store(dir: &Path) -> std::io::Result<Materialized> {
    std::fs::create_dir_all(dir)?;
    let identity = dir.join(".okena-knowledge/store.yaml");
    if !identity.exists() {
        write_file(
            &identity,
            &format!(
                "version: 1\nid: {DEFAULT_STORE_ID}\nname: okena defaults\n\
                 description: The briefs okena launches agents with. Copy one into your own store to override it.\n"
            ),
        )?;
    }
    let mut report = materialize(dir)?;
    let readme = dir.join("README.md");
    if !readme.exists() {
        std::fs::write(&readme, README)?;
        report.written.push("README.md".to_string());
    }
    Ok(report)
}

/// What somebody finds when they open the folder.
const README: &str = "\
# okena defaults

These are the briefs okena launches agents with. Read them here; to change one
for your organisation, copy it into your own knowledge store under the same path
and point Settings → Knowledge at that store. A file you do not override falls
back to the built-in, so you can change one brief without supplying the rest.

- `templates/<flow>.md` — the brief for one launch flow; frontmatter says which.
- `templates/partials/<name>.md` — text shared between briefs, included with
  `{>name}`, or used as `{value|name}` when a value is empty.

okena keeps the files here current: it updates any it wrote that you have not
changed, and leaves the ones you have edited alone.
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
    fn materialize_never_reverts_an_edit() {
        let dir = tempfile::tempdir().expect("tempdir");
        materialize(dir.path()).expect("write");
        let path = dir.path().join(Flow::SpecDraft.template_path());
        std::fs::write(&path, "our own words").expect("edit");
        let again = materialize(dir.path()).expect("write again");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "our own words"
        );
        assert_eq!(again.customized, [Flow::SpecDraft.template_path()]);
    }

    #[test]
    fn an_untouched_default_is_brought_up_to_date() {
        // What an upgrade looks like: okena wrote an older default, nobody
        // edited it, and the built-in has since changed.
        let dir = tempfile::tempdir().expect("tempdir");
        materialize(dir.path()).expect("write");
        let rel = Flow::TaskStart.template_path();
        let old = "an older default";
        std::fs::write(dir.path().join(&rel), old).expect("simulate old");
        // Record it as okena's own writing, the way that older run would have.
        let record_path = dir.path().join(super::RECORD_PATH);
        let record = std::fs::read_to_string(&record_path).expect("record");
        let rewritten: String = record
            .lines()
            .map(|line| {
                if line.ends_with(&format!(" {rel}")) {
                    format!("{} {rel}\n", super::fingerprint(old))
                } else {
                    format!("{line}\n")
                }
            })
            .collect();
        std::fs::write(&record_path, rewritten).expect("record old");

        let report = materialize(dir.path()).expect("upgrade");
        assert_eq!(report.updated, std::slice::from_ref(&rel));
        assert_eq!(
            std::fs::read_to_string(dir.path().join(&rel)).expect("read"),
            file(Flow::TaskStart)
        );
    }

    #[test]
    fn a_file_from_before_the_record_existed_is_not_overwritten() {
        // okena cannot tell an old default from an edit, so it assumes an edit.
        let dir = tempfile::tempdir().expect("tempdir");
        let rel = Flow::TaskStart.template_path();
        std::fs::create_dir_all(dir.path().join("templates")).expect("mkdir");
        std::fs::write(dir.path().join(&rel), "unknown provenance").expect("write");
        let report = materialize(dir.path()).expect("write");
        assert_eq!(report.customized, std::slice::from_ref(&rel));
        assert_eq!(
            std::fs::read_to_string(dir.path().join(&rel)).expect("read"),
            "unknown provenance"
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
