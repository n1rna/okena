//! A project's map: `project-map.yaml` at the top of its knowledge root.
//!
//! An agent writes the map, following okena's `project-map` skill, and the
//! repository commits it; okena reads it and never writes it (ADR-0005). The
//! manifest is the half okena parses. The prose beside it, under
//! `docs/project/`, is ordinary knowledge docs and needs nothing from here.
//!
//! Reading never fails outright. A map is not scanned, scanned, or invalid
//! with every problem found ([`ProjectMapState`]), so a broken manifest is
//! something to show rather than an error that hides the rest of a project.

use crate::{KnowledgeError, canonical, display, project};
use okena_core::project_map::{Interface, ProjectMap, ProjectMapReport, ProjectMapState};
use serde_yaml_ng::Value;
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

/// The manifest's file name, at the top of the knowledge root.
///
/// Outside the four kind folders, so the tree never lists it as a doc.
pub const MANIFEST_FILE: &str = "project-map.yaml";
/// Where the map's docs live, relative to the knowledge root.
pub const DOCS_DIR: &str = "docs/project";
/// The manifest format this okena reads.
pub const MAP_VERSION: u32 = 1;
/// Largest manifest read. The facts okena matches on are small; detail belongs
/// in the docs.
pub const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

const SHAPE_FIX: &str = "The `project-map` skill describes every field; `version: 1` and a `project` with `name` and `description` are required.";

pub fn manifest_path(root: &Path) -> PathBuf {
    root.join(MANIFEST_FILE)
}

/// Read the map of the repository at `repo`, from the knowledge root its
/// `.okena/knowledge.yaml` names (default `.okena/knowledge`).
pub fn load_for_repo(repo: &Path) -> ProjectMapState {
    report_for_repo(repo).state
}

/// [`load_for_repo`], with the key of the knowledge root the map lives in.
///
/// The key is built the way discovery builds a project root's, from the same
/// root path, so `KnowledgeRead` accepts it for the map's docs.
pub fn report_for_repo(repo: &Path) -> ProjectMapReport {
    let root =
        project::read_config(repo).and_then(|config| project::project_root(repo, config.as_ref()));
    match root {
        Ok(Some(root)) => ProjectMapReport {
            root_key: Some(okena_core::specs::path_root_key(&display(&root))),
            state: load(&root),
        },
        // No knowledge root means there is nowhere a map could be.
        Ok(None) => ProjectMapReport {
            root_key: None,
            state: ProjectMapState::NotScanned,
        },
        Err(e) => ProjectMapReport {
            root_key: None,
            state: invalid(&[e]),
        },
    }
}

/// Read the map in the knowledge root at `root`.
pub fn load(root: &Path) -> ProjectMapState {
    match read(root) {
        Ok(None) => ProjectMapState::NotScanned,
        Ok(Some(map)) => ProjectMapState::Scanned { map: Box::new(map) },
        Err(problems) => invalid(&problems),
    }
}

fn invalid(problems: &[KnowledgeError]) -> ProjectMapState {
    ProjectMapState::Invalid {
        problems: problems.iter().map(KnowledgeError::to_diagnostic).collect(),
    }
}

/// Read and validate `root`'s manifest. `Ok(None)` when there is none.
pub fn read(root: &Path) -> Result<Option<ProjectMap>, Vec<KnowledgeError>> {
    let path = manifest_path(root);
    let shown = display(&path);
    let meta = match std::fs::metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(vec![unreadable(&shown, &e)]),
    };
    if !meta.is_file() {
        return Err(vec![
            KnowledgeError::new("project_map_unreadable", format!("{shown} is not a file."))
                .with_fix(format!("Replace it with the {MANIFEST_FILE} file.")),
        ]);
    }
    // A symlinked manifest leading out of the root would show a file from
    // anywhere on disk as this project's map.
    if !canonical(&path).starts_with(canonical(root)) {
        return Err(vec![
            KnowledgeError::new(
                "project_map_outside",
                format!("{shown} resolves outside its knowledge root."),
            )
            .with_fix(format!(
                "Make {MANIFEST_FILE} a regular file in the knowledge root."
            )),
        ]);
    }
    if meta.len() > MAX_MANIFEST_BYTES {
        return Err(vec![
            KnowledgeError::new(
                "project_map_too_large",
                format!(
                    "{shown} is {} bytes; a manifest may be at most {MAX_MANIFEST_BYTES}.",
                    meta.len()
                ),
            )
            .with_fix(format!(
                "Keep the detail in the docs under {DOCS_DIR}/ and only the facts in the manifest."
            )),
        ]);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| vec![unreadable(&shown, &e)])?;
    parse(&text, &shown).map(Some)
}

fn unreadable(shown: &str, e: &std::io::Error) -> KnowledgeError {
    KnowledgeError::new(
        "project_map_unreadable",
        format!("Could not read {shown}: {e}"),
    )
    .with_fix("Check that the file is readable.")
}

/// Parse and validate manifest `text`. `origin` names it in messages.
///
/// Unknown keys are ignored, as they are in `store.yaml`, so a manifest that a
/// newer okena only added fields to still reads. A newer `version` is refused,
/// and checked before the fields: a breaking change should be reported as one,
/// not as whichever field it broke first.
pub fn parse(text: &str, origin: &str) -> Result<ProjectMap, Vec<KnowledgeError>> {
    let value: Value = serde_yaml_ng::from_str(text).map_err(|e| vec![not_valid(origin, &e)])?;
    let fields = match &value {
        Value::Mapping(fields) => fields,
        Value::Null => {
            return Err(vec![
                KnowledgeError::new("project_map_invalid", format!("{origin} is empty."))
                    .with_fix(SHAPE_FIX),
            ]);
        }
        _ => {
            return Err(vec![
                KnowledgeError::new(
                    "project_map_invalid",
                    format!("{origin} is not a set of `key: value` pairs."),
                )
                .with_fix(SHAPE_FIX),
            ]);
        }
    };
    match fields.get("version").map(Value::as_u64) {
        None => {
            return Err(vec![
                KnowledgeError::new("project_map_invalid", format!("{origin} has no `version`."))
                    .with_fix(format!("Start the file with `version: {MAP_VERSION}`.")),
            ]);
        }
        Some(Some(version)) if version > u64::from(MAP_VERSION) => {
            return Err(vec![
                KnowledgeError::new(
                    "project_map_version",
                    format!(
                        "{origin} is project map version {version}; this okena reads up to {MAP_VERSION}."
                    ),
                )
                .with_fix("Update okena."),
            ]);
        }
        Some(Some(version)) if version >= 1 => {}
        Some(_) => {
            return Err(vec![
                KnowledgeError::new(
                    "project_map_invalid",
                    format!("`version` in {origin} is not a version number."),
                )
                .with_fix(format!("Use `version: {MAP_VERSION}`.")),
            ]);
        }
    }
    // Parsed again from the text rather than from `value`, so the error keeps
    // its line and column.
    let map: ProjectMap = serde_yaml_ng::from_str(text).map_err(|e| vec![not_valid(origin, &e)])?;
    let problems = validate(&map);
    if problems.is_empty() {
        Ok(map)
    } else {
        Err(problems)
    }
}

fn not_valid(origin: &str, e: &serde_yaml_ng::Error) -> KnowledgeError {
    KnowledgeError::new(
        "project_map_invalid",
        format!("{origin} is not a valid project map: {e}"),
    )
    .with_fix(SHAPE_FIX)
}

/// Every rule the schema's types cannot express, all reported at once, so one
/// fix-up pass can address them together.
pub fn validate(map: &ProjectMap) -> Vec<KnowledgeError> {
    let mut check = Check::default();
    check.required("project.name", &map.project.name);
    check.required("project.description", &map.project.description);
    if let Some(doc) = &map.project.doc {
        check.doc("project.doc", doc);
    }
    if let Some(scanned) = &map.scanned {
        if !is_commit(&scanned.commit) {
            check.push(
                KnowledgeError::new(
                    "project_map_invalid_commit",
                    format!(
                        "`scanned.commit` is `{}`, which is not a commit hash.",
                        scanned.commit
                    ),
                )
                .with_fix("Use the hash `git rev-parse HEAD` prints, in quotes."),
            );
        }
        check.required("scanned.at", &scanned.at);
    }

    let known: BTreeSet<&str> = map.areas.iter().map(|a| a.id.as_str()).collect();
    let mut seen = BTreeSet::new();
    for (i, area) in map.areas.iter().enumerate() {
        let at = at("areas", i, &area.id);
        check.id(&format!("{at}.id"), &area.id, &mut seen, "areas");
        check.required(&format!("{at}.description"), &area.description);
        check.listed(
            &format!("{at}.paths"),
            &area.paths,
            "the paths or globs that belong to this area",
        );
        for path in &area.paths {
            check.repo_path(&format!("{at}.paths"), path);
        }
        if let Some(doc) = &area.doc {
            check.doc(&format!("{at}.doc"), doc);
        }
    }

    let mut seen = BTreeSet::new();
    for (i, concept) in map.concepts.iter().enumerate() {
        let at = at("concepts", i, &concept.id);
        check.id(&format!("{at}.id"), &concept.id, &mut seen, "concepts");
        check.required(&format!("{at}.description"), &concept.description);
        check.listed(
            &format!("{at}.areas"),
            &concept.areas,
            "the ids of the areas that implement it",
        );
        check.area_refs(&format!("{at}.areas"), &concept.areas, &known);
        if let Some(doc) = &concept.doc {
            check.doc(&format!("{at}.doc"), doc);
        }
    }

    check.interfaces("exposes", &map.exposes, &known);
    check.interfaces("consumes", &map.consumes, &known);

    for (i, pipeline) in map.ci.iter().enumerate() {
        let at = format!("ci[{i}]");
        check.required(&format!("{at}.name"), &pipeline.name);
        check.listed(
            &format!("{at}.files"),
            &pipeline.files,
            "the files that define this pipeline",
        );
        for file in &pipeline.files {
            check.repo_path(&format!("{at}.files"), file);
        }
    }

    for (i, resource) in map.infrastructure.iter().enumerate() {
        let at = format!("infrastructure[{i}]");
        check.required(&format!("{at}.name"), &resource.name);
        for file in &resource.files {
            check.repo_path(&format!("{at}.files"), file);
        }
    }

    check.problems
}

/// Where an entry of `list` is, by id when it has one: `areas[billing]` is
/// easier to find than `areas[3]`.
fn at(list: &str, index: usize, id: &str) -> String {
    match id.trim() {
        "" => format!("{list}[{index}]"),
        id => format!("{list}[{id}]"),
    }
}

fn is_commit(commit: &str) -> bool {
    (7..=40).contains(&commit.len()) && commit.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Relative, and without a `..` that could climb out of the repository.
fn is_relative_inside(path: &str) -> bool {
    !path.trim().is_empty()
        && !path.starts_with(['/', '\\'])
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

#[derive(Default)]
struct Check {
    problems: Vec<KnowledgeError>,
}

impl Check {
    fn push(&mut self, problem: KnowledgeError) {
        self.problems.push(problem);
    }

    fn required(&mut self, field: &str, value: &str) {
        if value.trim().is_empty() {
            self.push(
                KnowledgeError::new("project_map_missing_field", format!("`{field}` is empty."))
                    .with_fix(format!("Fill in `{field}`.")),
            );
        }
    }

    fn listed<T>(&mut self, field: &str, list: &[T], what: &str) {
        if list.is_empty() {
            self.push(
                KnowledgeError::new(
                    "project_map_missing_field",
                    format!("`{field}` lists nothing."),
                )
                .with_fix(format!("List {what}.")),
            );
        }
    }

    fn id(&mut self, field: &str, id: &str, seen: &mut BTreeSet<String>, list: &str) {
        if !okena_core::specs::is_kebab_id(id) {
            self.push(
                KnowledgeError::new(
                    "project_map_invalid_id",
                    format!("`{field}` is `{id}`, which is not kebab-case."),
                )
                .with_fix(
                    "Use lowercase letters and digits separated by single hyphens, e.g. `billing`.",
                ),
            );
        } else if !seen.insert(id.to_string()) {
            self.push(
                KnowledgeError::new(
                    "project_map_duplicate_id",
                    format!("`{id}` is the id of more than one entry in `{list}`."),
                )
                .with_fix(format!(
                    "Give each entry in `{list}` its own id, or merge them."
                )),
            );
        }
    }

    fn area_refs(&mut self, field: &str, refs: &[String], areas: &BTreeSet<&str>) {
        for id in refs {
            if !areas.contains(id.as_str()) {
                self.push(
                    KnowledgeError::new(
                        "project_map_unknown_area",
                        format!("`{field}` names `{id}`, which is not an area."),
                    )
                    .with_fix("Use the `id` of an entry under `areas`, or add that area."),
                );
            }
        }
    }

    fn repo_path(&mut self, field: &str, path: &str) {
        if !is_relative_inside(path) {
            self.push(
                KnowledgeError::new(
                    "project_map_path_outside",
                    format!("`{field}` has `{path}`, which is not a path inside the repository."),
                )
                .with_fix("Use a path relative to the repository root, without `..`."),
            );
        }
    }

    /// A doc is a Markdown file under `docs/` in the knowledge root, so it is
    /// a listed knowledge doc that can be opened from the map.
    fn doc(&mut self, field: &str, path: &str) {
        let as_path = Path::new(path);
        let ok = is_relative_inside(path)
            && as_path.components().next() == Some(Component::Normal(OsStr::new("docs")))
            && as_path.extension().is_some_and(|ext| ext == "md");
        if !ok {
            self.push(
                KnowledgeError::new(
                    "project_map_doc_path",
                    format!("`{field}` is `{path}`, which is not a Markdown doc under `docs/`."),
                )
                .with_fix(format!(
                    "Use a `.md` path under `docs/` in the knowledge root, e.g. `{DOCS_DIR}/overview.md`."
                )),
            );
        }
    }

    fn interfaces(&mut self, list: &str, entries: &[Interface], areas: &BTreeSet<&str>) {
        let mut seen = BTreeSet::new();
        for (i, entry) in entries.iter().enumerate() {
            let at = format!("{list}[{i}]");
            self.required(&format!("{at}.name"), &entry.name);
            self.area_refs(&format!("{at}.areas"), &entry.areas, areas);
            let name = entry.name.trim();
            if !name.is_empty() && !seen.insert((entry.kind, name)) {
                self.push(
                    KnowledgeError::new(
                        "project_map_duplicate_interface",
                        format!(
                            "`{list}` lists {} `{name}` more than once.",
                            entry.kind.id()
                        ),
                    )
                    .with_fix("List it once, with every area that uses it in its `areas`."),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::write;
    use okena_core::project_map::InterfaceKind;

    const HEAD: &str = "version: 1\nproject:\n  name: api\n  description: The public API.\n";

    fn manifest(rest: &str) -> String {
        format!("{HEAD}{rest}")
    }

    fn parsed(rest: &str) -> ProjectMap {
        parse(&manifest(rest), "project-map.yaml").unwrap_or_else(|e| panic!("{e:?}"))
    }

    /// The codes `text` is refused with. Every problem must carry a fix.
    fn codes(text: &str) -> Vec<&'static str> {
        let problems = parse(text, "project-map.yaml").expect_err("refused");
        for p in &problems {
            assert!(p.fix.is_some(), "{} has no fix: {}", p.code, p.message);
        }
        problems.iter().map(|p| p.code).collect()
    }

    #[test]
    fn the_smallest_map_is_a_version_and_a_project() {
        let map = parsed("");
        assert_eq!(map.version, 1);
        assert_eq!(map.project.name, "api");
        assert_eq!(map.project.description, "The public API.");
        assert!(map.scanned.is_none());
        assert!(map.areas.is_empty() && map.exposes.is_empty() && map.ci.is_empty());
    }

    #[test]
    fn project_and_scan_stamp() {
        let map = parsed(
            "  doc: docs/project/overview.md\nscanned:\n  commit: \"3f9a2c1\"\n  at: \"2026-09-13T10:00:00Z\"\n",
        );
        assert_eq!(map.project.doc.as_deref(), Some("docs/project/overview.md"));
        let scanned = map.scanned.expect("scanned");
        assert_eq!(scanned.commit, "3f9a2c1");
        assert_eq!(scanned.at, "2026-09-13T10:00:00Z");
    }

    #[test]
    fn areas_and_concepts() {
        let map = parsed(
            "areas:
  - id: http
    description: Routes and middleware.
    paths: [src/http/**]
  - id: billing
    name: Billing
    description: Invoices and payment runs.
    paths: [src/billing/**, ./migrations/billing]
    doc: docs/project/areas/billing.md
concepts:
  - id: invoice
    name: Invoice
    description: A bill for one period.
    areas: [billing, http]
    doc: docs/project/concepts.md
",
        );
        assert_eq!(map.areas.len(), 2);
        let billing = map.area("billing").expect("billing");
        assert_eq!(billing.label(), "Billing");
        assert_eq!(billing.paths, ["src/billing/**", "./migrations/billing"]);
        assert_eq!(
            billing.doc.as_deref(),
            Some("docs/project/areas/billing.md")
        );
        assert_eq!(map.area("http").map(|a| a.label()), Some("http"));
        let invoice = map.concept("invoice").expect("invoice");
        assert_eq!(invoice.areas, ["billing", "http"]);
    }

    #[test]
    fn exposes_and_consumes() {
        let map = parsed(
            "areas:
  - id: http
    description: Routes.
    paths: [src]
exposes:
  - type: http
    name: api.acme.com/v1
    description: Public REST API.
    areas: [http]
  - type: topic
    name: billing.invoice-issued
consumes:
  - type: grpc
    name: acme.accounts.v1.AccountService
  - type: package
    name: \"@acme/money\"
  - type: database
    name: accounts
",
        );
        let kinds = |list: &[Interface]| list.iter().map(|i| i.kind).collect::<Vec<_>>();
        assert_eq!(
            kinds(&map.exposes),
            [InterfaceKind::Http, InterfaceKind::Topic]
        );
        assert_eq!(
            kinds(&map.consumes),
            [
                InterfaceKind::Grpc,
                InterfaceKind::Package,
                InterfaceKind::Database
            ]
        );
        assert_eq!(map.exposes[0].name, "api.acme.com/v1");
        assert_eq!(map.exposes[0].areas, ["http"]);
        assert_eq!(map.consumes[1].name, "@acme/money");
    }

    #[test]
    fn ci_and_infrastructure() {
        let map = parsed(
            "ci:
  - name: test
    provider: github-actions
    description: Tests on every pull request.
    files: [.github/workflows/test.yml]
infrastructure:
  - name: postgres
    kind: database
    files: [docker-compose.yml]
  - name: cdn
",
        );
        assert_eq!(map.ci[0].provider.as_deref(), Some("github-actions"));
        assert_eq!(map.ci[0].files, [".github/workflows/test.yml"]);
        assert_eq!(map.infrastructure[0].kind.as_deref(), Some("database"));
        assert!(map.infrastructure[1].files.is_empty());
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let map = parsed(
            "owner: platform-team\nareas:\n  - id: core\n    description: d\n    paths: [src]\n    colour: red\n",
        );
        assert_eq!(map.areas[0].id, "core");
    }

    #[test]
    fn a_malformed_manifest_is_refused_without_panicking() {
        assert_eq!(codes("version: 1\nproject: [\n"), ["project_map_invalid"]);
        assert_eq!(codes(""), ["project_map_invalid"]);
        assert_eq!(codes("- version: 1\n"), ["project_map_invalid"]);
        assert_eq!(codes("version: one\n"), ["project_map_invalid"]);
        assert_eq!(codes("version: 0\n"), ["project_map_invalid"]);
    }

    #[test]
    fn a_missing_version_or_required_field_is_refused() {
        assert_eq!(
            codes("project:\n  name: api\n  description: d\n"),
            ["project_map_invalid"]
        );
        let problems =
            parse("version: 1\nproject:\n  name: api\n", "m.yaml").expect_err("no description");
        assert_eq!(problems[0].code, "project_map_invalid");
        assert!(
            problems[0].message.contains("description"),
            "{}",
            problems[0].message
        );
        assert_eq!(
            codes(&manifest("areas:\n  - id: core\n    description: d\n")),
            ["project_map_invalid"],
            "an area without paths"
        );
    }

    #[test]
    fn a_newer_version_is_refused_as_newer_even_with_fields_this_okena_lacks() {
        assert_eq!(
            codes("version: 2\nsomething_new: {}\n"),
            ["project_map_version"]
        );
    }

    #[test]
    fn an_unknown_interface_type_names_the_valid_ones() {
        let problems = parse(
            &manifest("exposes:\n  - type: rest\n    name: x\n"),
            "m.yaml",
        )
        .expect_err("refused");
        assert_eq!(problems[0].code, "project_map_invalid");
        assert!(
            problems[0].message.contains("http"),
            "{}",
            problems[0].message
        );
    }

    #[test]
    fn every_rule_broken_is_reported_at_once() {
        let got = codes(&manifest(
            "scanned:
  commit: not-a-hash
  at: \"\"
areas:
  - id: Billing_Module
    description: \"\"
    paths: [../elsewhere, /etc]
    doc: README.md
  - id: http
    description: d
    paths: []
  - id: http
    description: d
    paths: [src]
concepts:
  - id: invoice
    description: d
    areas: [payments]
exposes:
  - type: http
    name: api.acme.com
  - type: http
    name: api.acme.com
consumes:
  - type: queue
    name: \"  \"
    areas: [nowhere]
ci:
  - name: test
    files: []
infrastructure:
  - name: db
    files: [../../compose.yml]
",
        ));
        for code in [
            "project_map_invalid_commit",
            "project_map_missing_field",
            "project_map_invalid_id",
            "project_map_path_outside",
            "project_map_doc_path",
            "project_map_duplicate_id",
            "project_map_unknown_area",
            "project_map_duplicate_interface",
        ] {
            assert!(got.contains(&code), "{code} missing from {got:?}");
        }
    }

    #[test]
    fn messages_locate_an_entry_by_its_id() {
        let problems = parse(
            &manifest("areas:\n  - id: billing\n    description: \"\"\n    paths: [src]\n"),
            "m.yaml",
        )
        .expect_err("refused");
        assert!(
            problems[0].message.contains("areas[billing].description"),
            "{}",
            problems[0].message
        );
    }

    #[test]
    fn doc_and_repo_paths() {
        let area = |paths: &str, doc: &str| {
            manifest(&format!(
                "areas:\n  - id: a\n    description: d\n    paths: [{paths}]\n    doc: {doc}\n"
            ))
        };
        assert!(parse(&area("./src/**", "docs/project/a.md"), "m").is_ok());
        for doc in [
            "README.md",
            "docs/../x.md",
            "docs/a.txt",
            "/docs/a.md",
            "docs",
        ] {
            assert_eq!(codes(&area("src", doc)), ["project_map_doc_path"], "{doc}");
        }
        for path in ["../x", "/etc", "src/../../x"] {
            assert_eq!(
                codes(&area(path, "docs/a.md")),
                ["project_map_path_outside"],
                "{path}"
            );
        }
    }

    #[test]
    fn a_commit_is_seven_to_forty_hex_digits() {
        let stamp =
            |commit: &str| manifest(&format!("scanned:\n  commit: \"{commit}\"\n  at: now\n"));
        assert!(parse(&stamp("3f9a2c1"), "m").is_ok());
        assert!(parse(&stamp(&"a".repeat(40)), "m").is_ok());
        for bad in ["3f9a2c", "zzzzzzz", &"a".repeat(41)] {
            assert_eq!(codes(&stamp(bad)), ["project_map_invalid_commit"], "{bad}");
        }
    }

    #[test]
    fn the_same_name_under_another_type_is_not_a_duplicate() {
        assert!(
            parse(
                &manifest("exposes:\n  - type: http\n    name: billing\n  - type: database\n    name: billing\n"),
                "m"
            )
            .is_ok()
        );
    }

    #[test]
    fn load_reports_not_scanned_scanned_and_invalid() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(load(dir.path()), ProjectMapState::NotScanned);
        assert_eq!(
            load(&dir.path().join("missing")),
            ProjectMapState::NotScanned
        );

        write(&manifest_path(dir.path()), &manifest(""));
        assert_eq!(
            load(dir.path()).map().map(|m| m.project.name.as_str()),
            Some("api")
        );

        write(&manifest_path(dir.path()), "version: 1\nproject: [\n");
        let state = load(dir.path());
        let problem = state.problem().expect("invalid");
        assert_eq!(problem.code, "project_map_invalid");
        assert!(problem.fix.is_some());
    }

    #[test]
    fn an_oversized_or_non_file_manifest_is_invalid() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(manifest_path(dir.path())).expect("mkdir");
        assert_eq!(
            load(dir.path()).problem().map(|d| d.code.as_str()),
            Some("project_map_unreadable")
        );

        let dir = tempfile::tempdir().expect("tempdir");
        let padding = "#".repeat(MAX_MANIFEST_BYTES as usize + 1);
        write(
            &manifest_path(dir.path()),
            &format!("{}{padding}\n", manifest("")),
        );
        assert_eq!(
            load(dir.path()).problem().map(|d| d.code.as_str()),
            Some("project_map_too_large")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_manifest_symlinked_out_of_the_root_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).expect("mkdir");
        write(&dir.path().join("elsewhere.yaml"), &manifest(""));
        std::os::unix::fs::symlink(dir.path().join("elsewhere.yaml"), manifest_path(&root))
            .expect("symlink");
        assert_eq!(
            load(&root).problem().map(|d| d.code.as_str()),
            Some("project_map_outside")
        );
    }

    #[test]
    fn a_repo_map_is_read_from_its_knowledge_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        assert_eq!(load_for_repo(repo), ProjectMapState::NotScanned);

        write(
            &manifest_path(&repo.join(project::DEFAULT_PROJECT_ROOT)),
            &manifest(""),
        );
        assert!(load_for_repo(repo).map().is_some(), "default root");

        write(
            &repo.join(project::PROJECT_CONFIG),
            "root: docs/knowledge\n",
        );
        assert_eq!(
            load_for_repo(repo),
            ProjectMapState::NotScanned,
            "a configured root without a map"
        );
        write(&manifest_path(&repo.join("docs/knowledge")), &manifest(""));
        assert!(load_for_repo(repo).map().is_some(), "configured root");

        write(&repo.join(project::PROJECT_CONFIG), "stores: acme\n");
        assert_eq!(
            load_for_repo(repo).problem().map(|d| d.code.as_str()),
            Some("project_config_invalid")
        );
    }

    #[test]
    fn a_reports_root_key_is_the_one_discovery_gives_the_root() {
        // Otherwise a map doc opened from the report is refused as a root the
        // daemon never discovered.
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("api");
        write(
            &manifest_path(&repo.join(project::DEFAULT_PROJECT_ROOT)),
            &manifest(""),
        );
        write(
            &repo
                .join(project::DEFAULT_PROJECT_ROOT)
                .join(DOCS_DIR)
                .join("overview.md"),
            "# Overview\n",
        );
        let report = report_for_repo(&repo);
        assert!(report.state.map().is_some());

        let found = crate::discover::discover(&crate::discover::Sources {
            registry_path: dir.path().join("config/knowledge/stores.yaml"),
            projects: vec![crate::discover::ProjectSource {
                name: "api".into(),
                path: repo.clone(),
            }],
        });
        let keys: Vec<_> = found.roots.iter().map(|r| r.key.clone()).collect();
        assert_eq!(keys, [report.root_key.expect("a root key")]);

        assert_eq!(report_for_repo(&dir.path().join("none")).root_key, None);
    }

    #[test]
    fn map_docs_list_as_docs_and_the_manifest_does_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(&manifest_path(dir.path()), &manifest(""));
        write(
            &dir.path().join(DOCS_DIR).join("overview.md"),
            "---\ntitle: Overview\ndescription: What api is\n---\n",
        );
        let tree = crate::tree::read_tree(dir.path());
        let paths: Vec<_> = tree.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["docs/project/overview.md"]);
    }
}
