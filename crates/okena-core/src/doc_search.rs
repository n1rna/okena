//! Searching the Library across every origin (QBL-436, QBL-440).
//!
//! The rules and the shapes that cross the wire. A client cannot see the
//! files, so the daemon reads them and runs these matchers; they live here so
//! both sides agree on what a search means and the rules can be tested without
//! a disk.
//!
//! Three rules, the same ones the Tasks filter follows:
//!
//! - **Text** is trimmed and compared without case against a file's names, its
//!   path — which is how a directory name matches — and its content. Blank
//!   text matches everything.
//! - **Within a group, OR.** Two origins means "either".
//! - **Across groups, AND.** An origin *and* a type *and* a kind *and* text
//!   means all four.
//!
//! What a search returns is `okena_core::library::LibrarySearchResult`.

use crate::knowledge::KnowledgeKind;
use crate::library::OriginType;
use serde::{Deserialize, Serialize};

/// What a knowledge file is, as the Kind filter offers it.
///
/// Finer than [`KnowledgeKind`]: a partial and a brief are both templates on
/// disk, told apart by the folder under `templates/` they sit in, and they are
/// what people look for by kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeFacet {
    Doc,
    Skill,
    Agent,
    /// A template that is neither a partial nor a brief.
    Template,
    Partial,
    Brief,
}

impl KnowledgeFacet {
    /// Every facet, in the order the chips are drawn.
    pub const fn all() -> [KnowledgeFacet; 6] {
        [
            KnowledgeFacet::Doc,
            KnowledgeFacet::Skill,
            KnowledgeFacet::Agent,
            KnowledgeFacet::Template,
            KnowledgeFacet::Partial,
            KnowledgeFacet::Brief,
        ]
    }

    pub const fn label(self) -> &'static str {
        match self {
            KnowledgeFacet::Doc => "doc",
            KnowledgeFacet::Skill => "skill",
            KnowledgeFacet::Agent => "agent",
            KnowledgeFacet::Template => "template",
            KnowledgeFacet::Partial => "partial",
            KnowledgeFacet::Brief => "brief",
        }
    }

    /// The facet of an entry of `kind` named `name` — its path under the kind
    /// folder, so `partials/context` is a partial and `briefs/task-start` a
    /// brief.
    pub fn of(kind: KnowledgeKind, name: &str) -> Self {
        match kind {
            KnowledgeKind::Doc => KnowledgeFacet::Doc,
            KnowledgeKind::Skill => KnowledgeFacet::Skill,
            KnowledgeKind::Agent => KnowledgeFacet::Agent,
            KnowledgeKind::Template if name.starts_with("partials/") => KnowledgeFacet::Partial,
            KnowledgeKind::Template if name.starts_with("briefs/") => KnowledgeFacet::Brief,
            KnowledgeKind::Template => KnowledgeFacet::Template,
        }
    }
}

/// One file as a matcher sees it.
#[derive(Clone, Copy, Debug)]
pub struct SearchDoc<'a> {
    /// Key of the root the file is in.
    pub root_key: &'a str,
    /// What the file is called: a title, a name, a capability id.
    pub names: &'a [&'a str],
    /// Its path relative to the root, and any file that belongs to it — a
    /// skill's supporting files.
    pub paths: &'a [&'a str],
    /// Its text. Empty when it could not be read.
    pub content: &'a str,
}

/// Whether `doc` matches `query`: trimmed, case-insensitive, over its names,
/// paths and content. A blank query matches everything.
pub fn text_matches(query: &str, doc: &SearchDoc) -> bool {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    doc.names
        .iter()
        .chain(doc.paths)
        .any(|field| field.to_lowercase().contains(&needle))
        || doc.content.to_lowercase().contains(&needle)
}

/// Whether the Root group lets `root_key` through: nothing selected is no
/// opinion.
fn root_allows(selected: &[String], root_key: &str) -> bool {
    selected.is_empty() || selected.iter().any(|r| r == root_key)
}

/// What the Library island is narrowed to. `Default` is everything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LibrarySearchFilter {
    pub query: String,
    /// Origin keys. Empty is every origin.
    pub roots: Vec<String>,
    /// Origin types. Empty is every type.
    pub types: Vec<OriginType>,
    /// What a knowledge file is. Empty is no opinion; anything chosen leaves
    /// out every file that is not a knowledge file, since only those have a
    /// kind.
    pub kinds: Vec<KnowledgeFacet>,
}

impl LibrarySearchFilter {
    /// Whether anything narrows the list.
    pub fn is_active(&self) -> bool {
        !self.query.trim().is_empty()
            || !self.roots.is_empty()
            || !self.types.is_empty()
            || !self.kinds.is_empty()
    }

    /// The same choices without the text: what tells whether a file is worth
    /// opening to read its content.
    pub fn without_text(&self) -> Self {
        Self {
            query: String::new(),
            ..self.clone()
        }
    }
}

/// Whether a file in an origin of `origin_type` survives `filter`. `facet` is
/// what the file is when it is a knowledge file, and `None` otherwise.
pub fn library_matches(
    filter: &LibrarySearchFilter,
    origin_type: OriginType,
    facet: Option<KnowledgeFacet>,
    doc: &SearchDoc,
) -> bool {
    root_allows(&filter.roots, doc.root_key)
        && (filter.types.is_empty() || filter.types.contains(&origin_type))
        && (filter.kinds.is_empty() || facet.is_some_and(|f| filter.kinds.contains(&f)))
        && text_matches(&filter.query, doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc<'a>(
        root_key: &'a str,
        names: &'a [&'a str],
        paths: &'a [&'a str],
        content: &'a str,
    ) -> SearchDoc<'a> {
        SearchDoc {
            root_key,
            names,
            paths,
            content,
        }
    }

    fn pipeline() -> SearchDoc<'static> {
        doc(
            "store:acme",
            &["Release pipeline", "ci/pipeline"],
            &["docs/ci/pipeline.md"],
            "Every merge runs the Smoke suite before it ships.",
        )
    }

    #[test]
    fn text_matches_a_name() {
        assert!(text_matches("release", &pipeline()));
    }

    #[test]
    fn text_matches_a_directory_through_the_path() {
        // No name says `docs`; only the path does.
        assert!(text_matches("docs/ci", &pipeline()));
        assert!(text_matches("ci/", &pipeline()));
    }

    #[test]
    fn text_matches_a_word_only_the_content_has() {
        let d = pipeline();
        assert!(!d.names.iter().chain(d.paths).any(|f| f.contains("suite")));
        assert!(text_matches("suite", &d));
    }

    #[test]
    fn text_matches_a_supporting_file_of_a_skill() {
        let d = doc(
            "store:acme",
            &["release"],
            &["skills/release/SKILL.md", "skills/release/checklist.txt"],
            "",
        );
        assert!(text_matches("checklist", &d));
    }

    #[test]
    fn text_ignores_case_both_ways() {
        assert!(text_matches("SMOKE", &pipeline()));
        assert!(text_matches("release PIPELINE", &pipeline()));
    }

    #[test]
    fn text_is_trimmed_and_blank_matches_everything() {
        assert!(text_matches("  smoke \t", &pipeline()));
        assert!(text_matches("", &pipeline()));
        assert!(text_matches("   ", &pipeline()));
    }

    #[test]
    fn text_that_appears_nowhere_does_not_match() {
        assert!(!text_matches("kubernetes", &pipeline()));
    }

    #[test]
    fn a_template_under_partials_or_briefs_is_its_own_kind() {
        use KnowledgeKind as K;
        assert_eq!(
            KnowledgeFacet::of(K::Template, "partials/context"),
            KnowledgeFacet::Partial
        );
        assert_eq!(
            KnowledgeFacet::of(K::Template, "briefs/task-start"),
            KnowledgeFacet::Brief
        );
        assert_eq!(
            KnowledgeFacet::of(K::Template, "house-style"),
            KnowledgeFacet::Template
        );
        // Only templates split: a doc filed under a `partials/` folder is a doc.
        assert_eq!(
            KnowledgeFacet::of(K::Doc, "partials/notes"),
            KnowledgeFacet::Doc
        );
        assert_eq!(
            KnowledgeFacet::of(K::Skill, "release"),
            KnowledgeFacet::Skill
        );
        assert_eq!(
            KnowledgeFacet::of(K::Agent, "reviewer"),
            KnowledgeFacet::Agent
        );
    }

    const KNOWLEDGE: OriginType = OriginType::Knowledge;
    const DOC: Option<KnowledgeFacet> = Some(KnowledgeFacet::Doc);

    #[test]
    fn an_empty_filter_keeps_everything_of_every_type() {
        let f = LibrarySearchFilter::default();
        assert!(!f.is_active());
        assert!(library_matches(&f, KNOWLEDGE, DOC, &pipeline()));
        assert!(library_matches(&f, OriginType::Spec, None, &proposal()));
        assert!(library_matches(&f, OriginType::Freeform, None, &pipeline()));
    }

    #[test]
    fn blank_text_alone_is_not_a_filter() {
        let f = LibrarySearchFilter {
            query: "   ".into(),
            ..Default::default()
        };
        assert!(!f.is_active());
    }

    #[test]
    fn kinds_widen_among_themselves() {
        let f = LibrarySearchFilter {
            kinds: vec![KnowledgeFacet::Partial, KnowledgeFacet::Brief],
            ..Default::default()
        };
        assert!(f.is_active());
        let of = |facet| library_matches(&f, KNOWLEDGE, Some(facet), &pipeline());
        assert!(of(KnowledgeFacet::Partial));
        assert!(of(KnowledgeFacet::Brief));
        assert!(!of(KnowledgeFacet::Template));
        assert!(!of(KnowledgeFacet::Skill));
    }

    #[test]
    fn choosing_a_kind_leaves_out_files_that_have_none() {
        // Only knowledge files have a kind, so "docs" means knowledge docs: a
        // spec or a freeform file is not one of them.
        let f = LibrarySearchFilter {
            kinds: vec![KnowledgeFacet::Doc],
            ..Default::default()
        };
        assert!(library_matches(&f, KNOWLEDGE, DOC, &pipeline()));
        assert!(!library_matches(&f, OriginType::Spec, None, &proposal()));
        assert!(!library_matches(&f, OriginType::Freeform, None, &pipeline()));
    }

    #[test]
    fn types_widen_among_themselves_and_narrow_the_rest() {
        let f = LibrarySearchFilter {
            types: vec![OriginType::Spec, OriginType::Freeform],
            ..Default::default()
        };
        assert!(f.is_active());
        assert!(library_matches(&f, OriginType::Spec, None, &proposal()));
        assert!(library_matches(&f, OriginType::Freeform, None, &pipeline()));
        assert!(!library_matches(&f, KNOWLEDGE, DOC, &pipeline()));
    }

    #[test]
    fn roots_widen_among_themselves() {
        let f = LibrarySearchFilter {
            roots: vec!["store:acme".into(), "path:/repo".into()],
            ..Default::default()
        };
        assert!(library_matches(&f, KNOWLEDGE, DOC, &pipeline()));
        let elsewhere = doc("store:other", &["Release pipeline"], &["docs/p.md"], "");
        assert!(!library_matches(&f, KNOWLEDGE, DOC, &elsewhere));
    }

    #[test]
    fn root_type_kind_and_text_narrow_each_other() {
        let f = LibrarySearchFilter {
            query: "smoke".into(),
            roots: vec!["store:acme".into()],
            types: vec![OriginType::Knowledge],
            kinds: vec![KnowledgeFacet::Doc],
        };
        assert!(library_matches(&f, KNOWLEDGE, DOC, &pipeline()));
        // Each group alone can refuse what the others accept.
        assert!(!library_matches(
            &f,
            KNOWLEDGE,
            Some(KnowledgeFacet::Skill),
            &pipeline()
        ));
        assert!(!library_matches(&f, OriginType::Freeform, DOC, &pipeline()));
        let other_root = SearchDoc {
            root_key: "store:other",
            ..pipeline()
        };
        assert!(!library_matches(&f, KNOWLEDGE, DOC, &other_root));
        let other_text = LibrarySearchFilter {
            query: "kubernetes".into(),
            ..f.clone()
        };
        assert!(!library_matches(&other_text, KNOWLEDGE, DOC, &pipeline()));
        // Without its text the filter still holds its other choices, which is
        // what decides whether a file is opened at all.
        assert!(library_matches(&other_text.without_text(), KNOWLEDGE, DOC, &pipeline()));
        assert!(!library_matches(
            &other_text.without_text(),
            KNOWLEDGE,
            DOC,
            &other_root
        ));
    }

    fn proposal() -> SearchDoc<'static> {
        doc(
            "path:/repo",
            &["add-login/proposal.md"],
            &["openspec/changes/add-login/proposal.md"],
            "## Why\nUsers want to sign in with Google.",
        )
    }

    #[test]
    fn a_spec_matches_by_name_path_and_content() {
        let by = |query: &str| LibrarySearchFilter {
            query: query.into(),
            ..Default::default()
        };
        let spec = |f: &LibrarySearchFilter| library_matches(f, OriginType::Spec, None, &proposal());
        assert!(spec(&by("Proposal")));
        assert!(spec(&by("changes/add-login")));
        assert!(spec(&by(" google ")));
        assert!(!spec(&by("archive")));
    }

    #[test]
    fn results_survive_the_wire() {
        use crate::library::{LibraryHit, LibrarySearchResult};
        let result = LibrarySearchResult {
            hits: vec![
                LibraryHit {
                    root_key: "knowledge:store:acme".into(),
                    origin_type: OriginType::Knowledge,
                    path: "templates/partials/context.md".into(),
                    label: "context".into(),
                    facet: Some(KnowledgeFacet::Partial),
                },
                LibraryHit {
                    root_key: "freeform:path:/notes".into(),
                    origin_type: OriginType::Freeform,
                    path: "workflows/release.md".into(),
                    label: "Release".into(),
                    facet: None,
                },
            ],
            total: 12,
        };
        let json = serde_json::to_string(&result).expect("encode");
        assert!(json.contains("\"facet\":\"partial\""), "{json}");
        assert!(json.contains("\"type\":\"freeform\""), "{json}");
        let back: LibrarySearchResult = serde_json::from_str(&json).expect("decode");
        assert_eq!(back, result);
    }
}
