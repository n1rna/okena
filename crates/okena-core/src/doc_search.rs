//! Searching Knowledge and Specs across every root (QBL-436).
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
//! - **Within a group, OR.** Two roots means "either".
//! - **Across groups, AND.** A root *and* a kind *and* text means all three.

use crate::knowledge::KnowledgeKind;
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

/// What the Knowledge island is narrowed to. `Default` is everything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KnowledgeSearchFilter {
    pub query: String,
    /// Root keys. Empty is every root.
    pub roots: Vec<String>,
    /// Empty is every kind.
    pub kinds: Vec<KnowledgeFacet>,
}

impl KnowledgeSearchFilter {
    /// Whether anything narrows the list.
    pub fn is_active(&self) -> bool {
        !self.query.trim().is_empty() || !self.roots.is_empty() || !self.kinds.is_empty()
    }
}

/// Whether a knowledge file of `facet` survives `filter`.
pub fn knowledge_matches(
    filter: &KnowledgeSearchFilter,
    facet: KnowledgeFacet,
    doc: &SearchDoc,
) -> bool {
    root_allows(&filter.roots, doc.root_key)
        && (filter.kinds.is_empty() || filter.kinds.contains(&facet))
        && text_matches(&filter.query, doc)
}

/// What the Specs island is narrowed to. `Default` is everything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpecSearchFilter {
    pub query: String,
    /// Root keys. Empty is every root.
    pub roots: Vec<String>,
}

impl SpecSearchFilter {
    /// Whether anything narrows the list.
    pub fn is_active(&self) -> bool {
        !self.query.trim().is_empty() || !self.roots.is_empty()
    }
}

/// Whether a spec document survives `filter`.
pub fn spec_matches(filter: &SpecSearchFilter, doc: &SearchDoc) -> bool {
    root_allows(&filter.roots, doc.root_key) && text_matches(&filter.query, doc)
}

/// One knowledge file a search found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeHit {
    pub root_key: String,
    /// Relative to the root, as `KnowledgeTree` gives it.
    pub path: String,
    pub title: String,
    pub facet: KnowledgeFacet,
}

/// What a `KnowledgeSearch` found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeSearchResult {
    /// In root order, then by kind and path, as the trees list them.
    #[serde(default)]
    pub hits: Vec<KnowledgeHit>,
    /// How many files every healthy root holds, for "N of M".
    #[serde(default)]
    pub total: usize,
}

/// One spec document a search found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecHit {
    pub root_key: String,
    /// Relative to the root, as `SpecsTree` gives it.
    pub path: String,
    /// What to show: a capability id, or a change's name and its file.
    pub label: String,
}

/// What a `SpecSearch` found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecSearchResult {
    /// In root order, then changes, specs and the archive, as the trees list
    /// them.
    #[serde(default)]
    pub hits: Vec<SpecHit>,
    /// How many documents every healthy root holds, for "N of M".
    #[serde(default)]
    pub total: usize,
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

    #[test]
    fn an_empty_knowledge_filter_keeps_everything() {
        let f = KnowledgeSearchFilter::default();
        assert!(!f.is_active());
        assert!(knowledge_matches(&f, KnowledgeFacet::Doc, &pipeline()));
    }

    #[test]
    fn blank_text_alone_is_not_a_filter() {
        let f = KnowledgeSearchFilter {
            query: "   ".into(),
            ..Default::default()
        };
        assert!(!f.is_active());
        let s = SpecSearchFilter {
            query: " ".into(),
            ..Default::default()
        };
        assert!(!s.is_active());
    }

    #[test]
    fn kinds_widen_among_themselves() {
        let f = KnowledgeSearchFilter {
            kinds: vec![KnowledgeFacet::Partial, KnowledgeFacet::Brief],
            ..Default::default()
        };
        assert!(f.is_active());
        assert!(knowledge_matches(&f, KnowledgeFacet::Partial, &pipeline()));
        assert!(knowledge_matches(&f, KnowledgeFacet::Brief, &pipeline()));
        assert!(!knowledge_matches(
            &f,
            KnowledgeFacet::Template,
            &pipeline()
        ));
        assert!(!knowledge_matches(&f, KnowledgeFacet::Skill, &pipeline()));
    }

    #[test]
    fn roots_widen_among_themselves() {
        let f = KnowledgeSearchFilter {
            roots: vec!["store:acme".into(), "path:/repo".into()],
            ..Default::default()
        };
        assert!(knowledge_matches(&f, KnowledgeFacet::Doc, &pipeline()));
        let elsewhere = doc("store:other", &["Release pipeline"], &["docs/p.md"], "");
        assert!(!knowledge_matches(&f, KnowledgeFacet::Doc, &elsewhere));
    }

    #[test]
    fn root_kind_and_text_narrow_each_other() {
        let f = KnowledgeSearchFilter {
            query: "smoke".into(),
            roots: vec!["store:acme".into()],
            kinds: vec![KnowledgeFacet::Doc],
        };
        assert!(knowledge_matches(&f, KnowledgeFacet::Doc, &pipeline()));
        // Each group alone can refuse what the other two accept.
        assert!(!knowledge_matches(&f, KnowledgeFacet::Skill, &pipeline()));
        let other_root = SearchDoc {
            root_key: "store:other",
            ..pipeline()
        };
        assert!(!knowledge_matches(&f, KnowledgeFacet::Doc, &other_root));
        let other_text = KnowledgeSearchFilter {
            query: "kubernetes".into(),
            ..f.clone()
        };
        assert!(!knowledge_matches(
            &other_text,
            KnowledgeFacet::Doc,
            &pipeline()
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
        let by = |query: &str| SpecSearchFilter {
            query: query.into(),
            ..Default::default()
        };
        assert!(spec_matches(&by("Proposal"), &proposal()));
        assert!(spec_matches(&by("changes/add-login"), &proposal()));
        assert!(spec_matches(&by(" google "), &proposal()));
        assert!(!spec_matches(&by("archive"), &proposal()));
    }

    #[test]
    fn a_spec_root_narrows_and_text_narrows_within_it() {
        let mut f = SpecSearchFilter {
            roots: vec!["path:/repo".into()],
            ..Default::default()
        };
        assert!(f.is_active());
        assert!(spec_matches(&f, &proposal()));
        let elsewhere = SearchDoc {
            root_key: "store:acme",
            ..proposal()
        };
        assert!(!spec_matches(&f, &elsewhere));

        f.query = "google".into();
        assert!(spec_matches(&f, &proposal()));
        f.query = "github".into();
        assert!(!spec_matches(&f, &proposal()));
    }

    #[test]
    fn an_empty_spec_filter_keeps_everything() {
        let f = SpecSearchFilter::default();
        assert!(!f.is_active());
        assert!(spec_matches(&f, &proposal()));
    }

    #[test]
    fn results_survive_the_wire() {
        let result = KnowledgeSearchResult {
            hits: vec![KnowledgeHit {
                root_key: "store:acme".into(),
                path: "templates/partials/context.md".into(),
                title: "context".into(),
                facet: KnowledgeFacet::Partial,
            }],
            total: 12,
        };
        let json = serde_json::to_string(&result).expect("encode");
        assert!(json.contains("\"facet\":\"partial\""), "{json}");
        let back: KnowledgeSearchResult = serde_json::from_str(&json).expect("decode");
        assert_eq!(back, result);
    }
}
