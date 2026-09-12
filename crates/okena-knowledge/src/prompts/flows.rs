//! The launch flows, and the variables each one fills.
//!
//! ADR-0003 reserved `for:` in a template's frontmatter for "the launch flows
//! the template applies to" and left which flows exist undefined. This is that
//! definition: the contract between okena, which fills the variables, and an
//! organisation, which writes the prose around them.
//!
//! It is deliberately a closed set. A template naming a flow okena does not
//! have is a typo the user wants to hear about, not an extension point —
//! nothing would ever render it.

use std::fmt;

/// A point at which okena briefs an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Flow {
    /// Starting work on a task, with worktrees.
    TaskStart,
    /// Breaking a task into sub-tasks. Reads and writes through okena's MCP
    /// tools rather than doing the work.
    TaskBreakDown,
    /// Deciding how a task with sub-tasks splits among agents, and starting
    /// them. Distinct from breaking down: the children already exist, and what
    /// is being decided is how many agents the work is worth.
    TaskCoordinate,
    /// Drafting a new task from a title and a kind.
    TaskCreate,
    /// Drafting an OpenSpec change into a scaffolded directory.
    SpecDraft,
    /// Adding to or updating a knowledge root.
    KnowledgeDraft,
    /// A free-form session against a goal the user typed.
    AgentSession,
}

impl Flow {
    /// Stable id: the `for:` value, and the template's file name.
    ///
    /// Kebab-case because it is both, and because these are read and typed by
    /// people far more often than they are matched in code.
    pub const fn id(self) -> &'static str {
        match self {
            Flow::TaskStart => "task-start",
            Flow::TaskBreakDown => "break-down",
            Flow::TaskCoordinate => "task-coordinate",
            Flow::TaskCreate => "task-create",
            Flow::SpecDraft => "spec-draft",
            Flow::KnowledgeDraft => "knowledge-draft",
            Flow::AgentSession => "agent-session",
        }
    }

    /// What to call it in the UI.
    pub const fn label(self) -> &'static str {
        match self {
            Flow::TaskStart => "Start work on a task",
            Flow::TaskBreakDown => "Break a task down",
            Flow::TaskCoordinate => "Split a task among agents",
            Flow::TaskCreate => "Draft a new task",
            Flow::SpecDraft => "Draft a spec change",
            Flow::KnowledgeDraft => "Write knowledge",
            Flow::AgentSession => "Free-form session",
        }
    }

    /// Where a store keeps this flow's template, relative to the root.
    ///
    /// A flat file per flow under `templates/`, so the mapping from "which
    /// brief is this" to "which file do I edit" needs no index to look up. A
    /// store that wants to organise its own templates in folders still can —
    /// those are simply not the ones okena launches with.
    pub fn template_path(self) -> String {
        format!("templates/{}.md", self.id())
    }

    /// The variables okena fills for this flow.
    ///
    /// A template may use any subset. Using anything else renders verbatim and
    /// is reported (see [`super::render`]) rather than silently emptied.
    ///
    /// Several of these are pre-composed prose rather than raw values —
    /// `store_note`, `references`, `projects`, `description`. That is the cost
    /// of a renderer with no conditionals, and it is the right trade: a
    /// template stays a thing you can read straight through, and the branch
    /// that decides whether a store has referenced roots stays in Rust where
    /// it can be tested.
    pub const fn variables(self) -> &'static [&'static str] {
        match self {
            Flow::TaskStart => &[
                "key",
                "title",
                "url",
                "branch",
                "description",
                "projects",
                // What a coordinating agent said this share of the work is.
                // Empty for a session somebody started themselves.
                "note",
            ],
            Flow::TaskCoordinate => &[
                "key",
                "title",
                "description",
                "children",
                // Where it and its sub-agents work, and anything okena was
                // told to pass on.
                "projects",
                "note",
            ],
            Flow::TaskBreakDown => &[
                "key",
                // The provider's own id, which is what `okena_create_subtask`
                // takes for `parent` — not the human key beside it.
                "parent_id",
                "title",
                "kind",
                "url",
                "description",
                "child_kind",
            ],
            Flow::TaskCreate => &["title", "kind", "container", "parent", "description"],
            Flow::SpecDraft => &[
                "idea",
                "change",
                "change_dir",
                "root_path",
                "store_note",
                "references",
            ],
            Flow::KnowledgeDraft => &["request", "path", "what", "commit_note"],
            Flow::AgentSession => &["goal", "projects"],
        }
    }

    pub const fn all() -> &'static [Flow] {
        &[
            Flow::TaskStart,
            Flow::TaskBreakDown,
            Flow::TaskCoordinate,
            Flow::TaskCreate,
            Flow::SpecDraft,
            Flow::KnowledgeDraft,
            Flow::AgentSession,
        ]
    }

    /// Read a `for:` value back. Case- and separator-insensitive, because
    /// `for: Task_Start` in someone's frontmatter means what it looks like.
    pub fn from_id(id: &str) -> Option<Flow> {
        let norm = |s: &str| s.trim().to_ascii_lowercase().replace('_', "-");
        let id = norm(id);
        Flow::all().iter().copied().find(|f| norm(f.id()) == id)
    }
}

impl fmt::Display for Flow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

#[cfg(test)]
mod tests {
    use super::Flow;
    use std::collections::BTreeSet;

    #[test]
    fn every_flow_has_a_distinct_id_and_path() {
        let ids: BTreeSet<&str> = Flow::all().iter().map(|f| f.id()).collect();
        assert_eq!(ids.len(), Flow::all().len(), "two flows share an id");
        let paths: BTreeSet<String> = Flow::all().iter().map(|f| f.template_path()).collect();
        assert_eq!(paths.len(), Flow::all().len());
    }

    #[test]
    fn an_id_round_trips() {
        for flow in Flow::all() {
            assert_eq!(Flow::from_id(flow.id()), Some(*flow), "{flow}");
        }
    }

    #[test]
    fn a_for_value_is_read_leniently() {
        // What people actually type in frontmatter.
        assert_eq!(Flow::from_id(" Task_Start "), Some(Flow::TaskStart));
        assert_eq!(Flow::from_id("SPEC-DRAFT"), Some(Flow::SpecDraft));
    }

    #[test]
    fn an_unknown_flow_is_not_invented() {
        // A typo in `for:` should be reportable, not silently accepted.
        assert_eq!(Flow::from_id("task-startt"), None);
        assert_eq!(Flow::from_id(""), None);
    }

    #[test]
    fn a_template_lives_under_templates_named_for_its_flow() {
        assert_eq!(Flow::SpecDraft.template_path(), "templates/spec-draft.md");
    }

    #[test]
    fn no_flow_declares_a_duplicate_variable() {
        for flow in Flow::all() {
            let set: BTreeSet<&&str> = flow.variables().iter().collect();
            assert_eq!(set.len(), flow.variables().len(), "{flow}");
        }
    }
}
