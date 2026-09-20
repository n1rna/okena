//! Engineering-harness view identifiers.
//!
//! Lives in `okena-core` because both the sidebar (which renders the nav) and
//! the window (which renders the view as a tab) need to name the same sections,
//! and those live in different crates that only share this one.

use serde::{Deserialize, Serialize};

/// The harness views, in nav order.
///
/// `all()` drives the nav, the tab strip and persistence, so the three cannot
/// drift out of sync.
///
/// There is no Projects view: a project's worktrees, agents and git state live
/// in the project's own column, behind its info toggle — the same place an
/// agent session keeps its context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessSection {
    Tasks,
    Specs,
    Knowledge,
    Testing,
}

impl HarnessSection {
    pub const fn all() -> [HarnessSection; 4] {
        [
            HarnessSection::Tasks,
            HarnessSection::Specs,
            HarnessSection::Knowledge,
            HarnessSection::Testing,
        ]
    }

    /// The section a persisted slug names. `None` for a slug this build does
    /// not know, so a layout written by a newer okena falls back to no view
    /// rather than failing to load.
    pub fn from_slug(slug: &str) -> Option<HarnessSection> {
        HarnessSection::all().into_iter().find(|s| s.slug() == slug)
    }

    pub const fn label(self) -> &'static str {
        match self {
            HarnessSection::Tasks => "Tasks",
            HarnessSection::Specs => "Specs",
            HarnessSection::Knowledge => "Knowledge",
            HarnessSection::Testing => "Testing",
        }
    }

    /// Stable id used for element ids and persistence.
    pub const fn slug(self) -> &'static str {
        match self {
            HarnessSection::Tasks => "tasks",
            HarnessSection::Specs => "specs",
            HarnessSection::Knowledge => "knowledge",
            HarnessSection::Testing => "testing",
        }
    }

    /// One-line description of what the view is for. Shown as the section
    /// subtitle while the view itself is a stub.
    pub const fn blurb(self) -> &'static str {
        match self {
            HarnessSection::Tasks => {
                "Epics, features and stories from your task manager — launch an agent on one."
            }
            HarnessSection::Specs => {
                "Spec documents broken down into epics, features and stories. Git-backed."
            }
            HarnessSection::Knowledge => {
                "Skills, technical designs and feature docs. Git-backed collections."
            }
            HarnessSection::Testing => {
                "Agents verifying their work — each run's plan, and every step as it passes or fails."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_unique_and_stable() {
        // Slugs key element ids and persisted tab state, so a duplicate would
        // silently collapse two tabs into one.
        let mut seen = std::collections::HashSet::new();
        for s in HarnessSection::all() {
            assert!(seen.insert(s.slug()), "duplicate slug: {}", s.slug());
        }
    }

    #[test]
    fn every_section_has_label_and_blurb() {
        for s in HarnessSection::all() {
            assert!(!s.label().is_empty());
            assert!(!s.blurb().is_empty());
        }
    }

    #[test]
    fn every_section_is_found_by_its_own_slug() {
        for s in HarnessSection::all() {
            assert_eq!(HarnessSection::from_slug(s.slug()), Some(s));
        }
        assert_eq!(HarnessSection::from_slug("deployments"), None);
    }

    #[test]
    fn round_trips_through_serde() {
        for s in HarnessSection::all() {
            let json = serde_json::to_string(&s).expect("serialize");
            let back: HarnessSection = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, s);
        }
    }
}

// ─── Agent sessions ──────────────────────────────────────────────────────────

use serde::{Deserialize as De, Serialize as Ser};

/// What an agent produced.
///
/// Open-ended on purpose: agents report what they made, and the harness should
/// display a kind it doesn't model rather than dropping it.
#[derive(Clone, Debug, PartialEq, Eq, Ser, De, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentAssetKind {
    PullRequest,
    Branch,
    Document,
    /// A task filed through okena's MCP. The task itself is on
    /// [`AgentAsset::task`].
    Task,
    #[default]
    #[serde(other)]
    Other,
}

impl AgentAssetKind {
    pub const fn label(&self) -> &'static str {
        match self {
            AgentAssetKind::PullRequest => "PR",
            AgentAssetKind::Branch => "branch",
            AgentAssetKind::Document => "doc",
            AgentAssetKind::Task => "task",
            AgentAssetKind::Other => "asset",
        }
    }
}

/// One thing an agent produced — the sketch's "agent#1#asset#1 (PR on Proj1)".
#[derive(Clone, Debug, PartialEq, Eq, Ser, De)]
pub struct AgentAsset {
    pub kind: AgentAssetKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Which repo it landed in. An agent spanning several projects produces
    /// assets in more than one, so the asset carries its own project rather
    /// than inheriting the session's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Branch it is on, so okena can merge it into the branch it detects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Unix millis, stamped by the daemon on receipt. Agents have no reliable
    /// clock agreement with the host, so their timestamps are not trusted.
    #[serde(default)]
    pub created_at: u64,
    /// The task, when this asset is one. Identity only: a task's state moves
    /// on the provider, so it is read from the harness's task data when shown
    /// rather than frozen here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<crate::tasks::TaskRef>,
}

/// A pull request whose worktree has been removed.
///
/// Detected assets are otherwise derived from the live checkouts, so this is
/// the one thing a session has to remember: with the worktree gone there is
/// nothing left to poll by branch. While open, the daemon's git poller
/// refreshes it by repo and number. Once merged or closed it keeps that state,
/// is no longer polled, and stays on the session's card for as long as the
/// session exists.
#[derive(Clone, Debug, PartialEq, Eq, Ser, De)]
pub struct TrackedPullRequest {
    /// Repo label, as the worktree's row showed it.
    pub project: String,
    /// The parent repo's checkout, which still resolves the GitHub remote and
    /// credentials after the worktree is gone.
    pub repo_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub number: u32,
    pub url: String,
    pub state: crate::api::PrState,
    /// Mergeability and reviews as last read with the PR, while it is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness: Option<crate::api::PrReadiness>,
    /// The last read left readiness out, because the repo rejected it
    /// recently. See [`PrInfo::readiness_unavailable`](crate::api::PrInfo).
    #[serde(default, skip_serializing_if = "crate::api::is_false")]
    pub readiness_unavailable: bool,
}

impl TrackedPullRequest {
    /// Merged or closed: still listed, but no longer polled.
    pub fn is_finished(&self) -> bool {
        matches!(
            self.state,
            crate::api::PrState::Merged | crate::api::PrState::Closed
        )
    }
}

/// Where an agent says it is, as distinct from what its terminal shows.
///
/// The terminal can tell okena that a prompt is idle; it cannot tell okena
/// *why*. An agent that stopped because it has a question and one that stopped
/// because the work is ready for you look identical from outside, and they ask
/// for different things from you. So the agent says which.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Ser, De, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Getting on with it; nothing needed from you.
    #[default]
    Working,
    /// Stopped to ask you something it cannot decide alone.
    NeedsInput,
    /// Finished a piece of work and waiting for you to look, or to say what
    /// happens next — commit, open a PR, carry on.
    ReadyForReview,
    /// Cannot continue: a failing build, a missing credential, a conflict.
    Blocked,
    /// Done, with nothing further planned.
    Done,
    /// A state a newer agent reported that this build doesn't model. Treated
    /// as wanting attention, since an unknown reason to stop is still a stop.
    #[serde(other)]
    Unknown,
}

impl AgentState {
    /// Whether this is a reason for you to look, rather than a report.
    pub const fn wants_attention(self) -> bool {
        matches!(
            self,
            AgentState::NeedsInput
                | AgentState::ReadyForReview
                | AgentState::Blocked
                | AgentState::Unknown
        )
    }

    pub const fn label(self) -> &'static str {
        match self {
            AgentState::Working => "working",
            AgentState::NeedsInput => "needs input",
            AgentState::ReadyForReview => "ready for review",
            AgentState::Blocked => "blocked",
            AgentState::Done => "done",
            AgentState::Unknown => "needs attention",
        }
    }
}

/// Something the agent suggests you tell it next.
///
/// The label is what you read; the instruction is what gets typed into the
/// agent if you pick it. Kept apart so a button can say "Commit and open a PR"
/// while the agent receives the precise sentence it asked to be given.
#[derive(Clone, Debug, PartialEq, Eq, Ser, De)]
pub struct AgentSuggestion {
    pub label: String,
    pub instruction: String,
}

/// Agent-reported state for a session project.
#[derive(Clone, Debug, PartialEq, Eq, Ser, De, Default)]
pub struct AgentSessionState {
    /// Free-text status the agent last reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Why it stopped, when it has. `None` from an agent that only ever sent a
    /// status line, which is every agent before this existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<AgentState>,
    /// What it is asking, when it needs input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    /// What it suggests you tell it next.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<AgentSuggestion>,
    /// Unix millis the daemon received the report. Input reaching the agent's
    /// terminal after this outdates the report. `None` for a report stored
    /// before reports were stamped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_at: Option<u64>,
    /// What the agent registered over MCP. Branches and PRs okena detects are
    /// not stored here — see `crate::session_assets`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<AgentAsset>,
    /// PRs of this session's removed worktrees, whatever their state: an open
    /// one refreshed until it closes, a merged or closed one kept as it ended.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracked_prs: Vec<TrackedPullRequest>,
    /// Branches this session's agent pushed, however their checkout came to
    /// be — a Start work worktree, a `git worktree add` of its own, or its own
    /// checkout. Seen through the agent's hooks, and kept so their PRs are
    /// looked up, and stay listed, after that checkout is gone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pushed_branches: Vec<PushedBranch>,
}

/// A branch an agent session pushed.
#[derive(Clone, Debug, PartialEq, Eq, Ser, De)]
pub struct PushedBranch {
    /// Repo label: the name of the main checkout's directory.
    pub project: String,
    /// The repository's main checkout, which outlives any worktree the branch
    /// was pushed from and still resolves its GitHub remote.
    pub repo_path: String,
    pub branch: String,
}

/// What a free-form session is being started to build, which decides the
/// brief it opens with.
///
/// Absent — every session but one — is a session against a goal the person
/// typed, briefed by the `agent-session` flow. A variant here names a flow
/// whose brief wraps that text in a standing job, so the client that opens
/// the launcher does not have to carry the prose.
///
/// Not [`AgentPurpose`]: that records which card lists the session
/// afterwards. This one is only read while starting it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionBrief {
    /// Building an okena extension from a summary, started from the
    /// Extensions page. Briefed by the `extension-build` flow.
    ExtensionBuild,
}

impl SessionBrief {
    /// The launch flow this brief comes from, as templates and launchers
    /// name it.
    pub const fn flow_id(self) -> &'static str {
        match self {
            SessionBrief::ExtensionBuild => "extension-build",
        }
    }
}

/// What an agent session was started for: which card started it, and on what.
///
/// Set by the daemon when it starts the session, so every card lists exactly
/// the sessions it started. Matching on the markers alone could not tell two
/// helpers on the same ticket apart — a breakdown and a refine both carry the
/// task and a goal — so each card would have listed the other's agents.
///
/// A task's own identity is the session's `task_ref`; the document variants
/// name their target here because nothing else on the session does.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentPurpose {
    /// Doing the work a task describes.
    Work,
    /// Breaking a task into sub-tasks.
    Breakdown,
    /// Rewriting a task's title and description.
    Refine,
    /// Drafting a new OpenSpec change. `root` is the spec root's key.
    SpecDraft { root: String, change: String },
    /// Changing one document of a spec root, by path relative to the root.
    SpecEdit { root: String, path: String },
    /// Adding to a knowledge root. `root` is the root's key.
    KnowledgeDraft { root: String },
    /// Changing one file of a knowledge root, by path relative to the root.
    KnowledgeEdit { root: String, path: String },
    /// Started by an action of an extension installed from git, about one of
    /// its items (a row's id) when it names one.
    Extension {
        extension: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_label: Option<String>,
    },
}

#[cfg(test)]
mod purpose_tests {
    use super::AgentPurpose;

    #[test]
    fn a_purpose_round_trips_with_its_target() {
        for purpose in [
            AgentPurpose::Work,
            AgentPurpose::Refine,
            AgentPurpose::SpecEdit {
                root: "store:plans".into(),
                path: "openspec/specs/auth/spec.md".into(),
            },
            AgentPurpose::KnowledgeDraft {
                root: "store:eng".into(),
            },
            AgentPurpose::Extension {
                extension: "cli-table".into(),
                item: Some("job-7".into()),
                item_label: Some("job-7 (acme)".into()),
            },
        ] {
            let json = serde_json::to_value(&purpose).expect("encode");
            assert_eq!(
                serde_json::from_value::<AgentPurpose>(json).expect("decode"),
                purpose
            );
        }
    }

    #[test]
    fn the_wire_names_the_kind() {
        let json = serde_json::to_value(AgentPurpose::KnowledgeEdit {
            root: "r".into(),
            path: "docs/ci.md".into(),
        })
        .expect("encode");
        assert_eq!(json["kind"], "knowledge_edit");
        assert_eq!(json["path"], "docs/ci.md");
    }
}

#[cfg(test)]
mod agent_tests {
    use super::{AgentAsset, AgentAssetKind, AgentSessionState};

    #[test]
    fn only_stopping_states_want_attention() {
        use super::AgentState;
        assert!(!AgentState::Working.wants_attention());
        assert!(!AgentState::Done.wants_attention());
        for s in [
            AgentState::NeedsInput,
            AgentState::ReadyForReview,
            AgentState::Blocked,
        ] {
            assert!(s.wants_attention(), "{s:?}");
        }
    }

    #[test]
    fn an_unknown_state_from_a_newer_agent_still_asks_for_attention() {
        // An unrecognized reason to stop is still a stop; reading it as
        // "working" would hide an agent that is waiting on you.
        let s: super::AgentState = serde_json::from_str("\"awaiting_approval\"").expect("decode");
        assert_eq!(s, super::AgentState::Unknown);
        assert!(s.wants_attention());
    }

    #[test]
    fn a_state_from_an_older_daemon_decodes_with_no_reason() {
        let s: AgentSessionState =
            serde_json::from_str(r#"{"status":"reading the code"}"#).expect("decode");
        assert_eq!(s.status.as_deref(), Some("reading the code"));
        assert_eq!(s.state, None);
        assert!(s.suggestions.is_empty());
    }

    #[test]
    fn unknown_asset_kind_decodes_as_other() {
        // A newer agent reporting a kind this build doesn't model must still
        // show up, not fail the whole payload.
        let k: AgentAssetKind = serde_json::from_str("\"deployment\"").expect("decode");
        assert_eq!(k, AgentAssetKind::Other);
    }

    #[test]
    fn known_kinds_round_trip() {
        for k in [
            AgentAssetKind::PullRequest,
            AgentAssetKind::Branch,
            AgentAssetKind::Document,
            AgentAssetKind::Task,
        ] {
            let j = serde_json::to_string(&k).expect("encode");
            let back: AgentAssetKind = serde_json::from_str(&j).expect("decode");
            assert_eq!(back, k);
        }
    }

    #[test]
    fn empty_session_state_serializes_compactly() {
        // Every project carries this field; an empty one must not bloat
        // workspace.json with nulls and empty arrays.
        let j = serde_json::to_string(&AgentSessionState::default()).expect("encode");
        assert_eq!(j, "{}");
    }

    #[test]
    fn asset_keeps_its_own_project() {
        let a = AgentAsset {
            kind: AgentAssetKind::PullRequest,
            title: "Add harness".into(),
            url: Some("https://github.com/x/y/pull/12".into()),
            project: Some("okena".into()),
            branch: None,
            created_at: 42,
            task: None,
        };
        let back: AgentAsset =
            serde_json::from_str(&serde_json::to_string(&a).expect("encode")).expect("decode");
        assert_eq!(back, a);
        assert_eq!(back.kind.label(), "PR");
    }
}
