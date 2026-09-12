//! What okena knows about one agent session, independent of how it is shown.
//!
//! Collected from the workspace mirror in one place so the sidebar beside a
//! session's terminal and the cards in the Agents overview cannot drift apart —
//! they were separately written and had already started to.

use crate::workspace::state::Workspace;
use okena_core::harness::{AgentAsset, AgentState, AgentSuggestion};
use okena_core::tasks::TaskRef;

/// What a session was started to do.
///
/// An open enum rather than a boolean: task work and spec writing are the two
/// kinds today, and the panel is meant to grow more without every call site
/// learning about them. Anything unrecognized still renders as a session.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentSessionKind {
    /// Working a task from a provider, across one or more worktrees.
    Task(TaskRef),
    /// Drafting an OpenSpec change in the spec repository.
    Spec { change: String },
    /// A free-form session the user configured themselves.
    Custom { goal: String },
    /// A recognized agent running in an ordinary project.
    Plain,
}

impl AgentSessionKind {
    /// Short label for the session's kind, for a badge.
    pub fn label(&self) -> &'static str {
        match self {
            AgentSessionKind::Task(_) => "task",
            AgentSessionKind::Spec { .. } => "spec",
            AgentSessionKind::Custom { .. } => "agent",
            AgentSessionKind::Plain => "session",
        }
    }

    /// The line identifying what is being worked on, if there is one.
    pub fn subject(&self) -> Option<String> {
        match self {
            AgentSessionKind::Task(t) => Some(format!("{} — {}", t.display_key, t.title)),
            AgentSessionKind::Spec { change } => Some(change.clone()),
            AgentSessionKind::Custom { goal } => Some(goal.clone()),
            AgentSessionKind::Plain => None,
        }
    }

    /// Heading for the section listing where this session's work lands.
    pub fn workspace_heading(&self) -> &'static str {
        match self {
            AgentSessionKind::Spec { .. } => "DOCUMENTS",
            _ => "WORKTREES",
        }
    }

    /// What to say when that section is empty. Different per kind because
    /// "no worktrees" means something different for a spec writer, which never
    /// has any, than for a task agent, where it means the checkouts are gone.
    pub fn empty_workspace_note(&self) -> &'static str {
        match self {
            AgentSessionKind::Task(_) => "No related worktrees.",
            AgentSessionKind::Spec { .. } => "Writes directly to the spec repository.",
            AgentSessionKind::Custom { .. } | AgentSessionKind::Plain => {
                "Runs directly in its working directory."
            }
        }
    }
}

/// A checkout this session's work lands in.
#[derive(Clone, Debug)]
pub struct RelatedWorkspace {
    pub project_id: String,
    pub name: String,
    /// Repo the worktree belongs to, so a card says where it lives.
    pub repo: Option<String>,
    pub branch: Option<String>,
}

/// Everything the panel shows about one session.
#[derive(Clone, Debug)]
pub struct AgentSessionInfo {
    pub project_id: String,
    pub name: String,
    pub kind: AgentSessionKind,
    /// Working directory the session runs in.
    pub root: String,
    /// Agent command okena launched, when it recognizes one.
    pub agent: Option<String>,
    /// Whether okena's MCP server was wired into the launch, so the agent can
    /// report status and register assets at all.
    pub mcp: bool,
    /// Whether an agent is actually running in the session right now.
    pub running: bool,
    /// Whether a restart can bring back this session's conversation, rather
    /// than only start a new one.
    pub resumable: bool,
    /// Whether its terminal is sitting at a prompt waiting for input.
    pub waiting: bool,
    /// How long it has been idle, pre-formatted.
    pub idle: String,
    /// The last status the agent reported over MCP.
    pub status: Option<String>,
    /// Why the agent says it stopped, when it has said.
    pub reported: Option<AgentState>,
    /// What it is asking, when it needs input.
    pub question: Option<String>,
    /// What it suggests you tell it next.
    pub suggestions: Vec<AgentSuggestion>,
    pub assets: Vec<AgentAsset>,
    pub workspaces: Vec<RelatedWorkspace>,
    /// The agent on this task's parent, when one is running.
    pub parent: Option<RelatedAgent>,
    /// Agents on this task's sub-tasks.
    pub children: Vec<RelatedAgent>,
}

/// Another agent session in the same ticket breakdown.
#[derive(Clone, Debug)]
pub struct RelatedAgent {
    pub project_id: String,
    pub name: String,
    /// The ticket it is on, so a row can say which part of the breakdown.
    pub key: Option<String>,
    pub activity: SessionActivity,
}

/// What a session is doing right now, as its terminal shows it.
///
/// The terminal's word rather than the agent's: an agent that stopped reporting
/// still shows as waiting when its prompt is waiting, which is the state you
/// actually need to act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionActivity {
    /// No agent running — the session is on a bare shell, or has no terminal.
    Stopped,
    /// Sitting at a prompt, idle for `idle` (pre-formatted, possibly empty).
    Waiting {
        idle: String,
    },
    /// Sitting at a prompt *and* the agent says why: a question, work to
    /// review, a blocker. The case that most needs you, told apart from an
    /// agent merely idle.
    NeedsAttention {
        reason: AgentState,
    },
    Running,
}

impl SessionActivity {
    pub fn label(&self) -> String {
        match self {
            SessionActivity::Stopped => "stopped".to_string(),
            SessionActivity::Waiting { idle } if idle.is_empty() => "waiting".to_string(),
            SessionActivity::Waiting { idle } => format!("waiting · {idle}"),
            SessionActivity::NeedsAttention { reason } => reason.label().to_string(),
            SessionActivity::Running => "running".to_string(),
        }
    }

    /// Whether this is something you need to act on.
    pub fn wants_attention(&self) -> bool {
        matches!(self, SessionActivity::NeedsAttention { .. })
    }

    /// Chip colour: muted when nothing runs, a warning when it wants you.
    pub fn color(&self, t: &crate::theme::ThemeColors) -> u32 {
        match self {
            SessionActivity::Stopped => t.text_muted,
            SessionActivity::Waiting { .. } => t.warning,
            SessionActivity::NeedsAttention { reason } => match reason {
                AgentState::Blocked => t.error,
                _ => t.warning,
            },
            SessionActivity::Running => t.success,
        }
    }
}

/// Combine what the terminal shows with what the agent says.
///
/// The agent's reason only counts while its prompt is actually waiting. An
/// agent that reported "ready for review" and was then told to carry on is
/// working again the moment output resumes, and flagging it until it next
/// reports would cry wolf on exactly the sessions you have just answered.
pub(super) fn activity_of(
    running: bool,
    waiting: bool,
    idle: String,
    reported: Option<AgentState>,
) -> SessionActivity {
    if !running {
        return SessionActivity::Stopped;
    }
    if !waiting {
        return SessionActivity::Running;
    }
    match reported {
        Some(reason) if reason.wants_attention() => SessionActivity::NeedsAttention { reason },
        _ => SessionActivity::Waiting { idle },
    }
}

/// Whether `candidate` belongs to the same task as the session, and isn't the
/// session itself.
///
/// Matched on the provider's own task id rather than the display key, which
/// changes when an issue moves team and would silently drop the worktrees.
pub(super) fn is_related(
    session_id: &str,
    task_external_id: &str,
    candidate_id: &str,
    candidate_task: Option<&TaskRef>,
) -> bool {
    candidate_id != session_id
        && candidate_task.is_some_and(|t| t.id.external_id == task_external_id)
}

/// Classify a project as an agent session.
///
/// Returns `None` for anything that isn't one, so a caller can use this as the
/// single test for "does this deserve an agent panel".
pub fn session_kind(project: &crate::workspace::state::ProjectData) -> Option<AgentSessionKind> {
    if let Some(change) = project.spec_change.clone() {
        return Some(AgentSessionKind::Spec { change });
    }
    if let Some(goal) = project.custom_session.clone() {
        return Some(AgentSessionKind::Custom { goal });
    }
    if let Some(task) = project.task_ref.clone() {
        // A worktree carries the task too, but it is a checkout, not a session.
        if project.worktree_info.is_none() {
            return Some(AgentSessionKind::Task(task));
        }
    }
    None
}

impl AgentSessionInfo {
    /// Collect everything shown about `project_id`, or `None` if it is gone.
    ///
    /// `running`, `waiting` and `idle` come from the terminals registry, which
    /// the caller holds — the workspace mirror knows the layout but not what is
    /// alive inside it.
    pub fn collect(
        ws: &Workspace,
        terminals: &okena_terminal::TerminalsRegistry,
        project_id: &str,
    ) -> Option<Self> {
        let project = ws.project(project_id)?;
        let kind = session_kind(project).unwrap_or(AgentSessionKind::Plain);

        // Where this session's work lands. Found by task rather than by path: a
        // session is rooted above the repos precisely so one agent can span
        // several, so it has no directory that would place its checkouts.
        let workspaces: Vec<RelatedWorkspace> = match &kind {
            AgentSessionKind::Task(task) => ws
                .projects()
                .iter()
                .filter(|p| {
                    is_related(project_id, &task.id.external_id, &p.id, p.task_ref.as_ref())
                })
                .map(|p| RelatedWorkspace {
                    project_id: p.id.clone(),
                    name: p.name.clone(),
                    repo: p
                        .worktree_info
                        .as_ref()
                        .and_then(|wi| ws.project(&wi.parent_project_id))
                        .map(|parent| parent.name.clone()),
                    branch: ws
                        .remote_snapshot(&p.id)
                        .and_then(|s| s.git_status.as_ref())
                        .and_then(|g| g.branch.clone()),
                })
                .collect(),
            _ => Vec::new(),
        };

        let (agent, mcp, waiting, idle) = Self::terminal_facts(ws, terminals, project);
        let reported = project.agent.as_ref().and_then(|a| a.state);

        // The breakdown around it: the agent on the task's parent, and the
        // agents on its sub-tasks. Read from the tickets, so it holds however
        // each was started.
        let (parent, children) = match &kind {
            AgentSessionKind::Task(task) => (
                task.parent_id.as_deref().and_then(|parent_id| {
                    ws.projects()
                        .iter()
                        .filter(|p| p.id != project.id)
                        .find(|p| {
                            session_kind(p).is_some()
                                && p.task_ref
                                    .as_ref()
                                    .is_some_and(|t| t.id.external_id == parent_id)
                        })
                        .map(|p| Self::related_agent(ws, terminals, p))
                }),
                ws.projects()
                    .iter()
                    .filter(|p| p.id != project.id && session_kind(p).is_some())
                    .filter(|p| {
                        p.task_ref.as_ref().and_then(|t| t.parent_id.as_deref())
                            == Some(task.id.external_id.as_str())
                    })
                    .map(|p| Self::related_agent(ws, terminals, p))
                    .collect(),
            ),
            _ => (None, Vec::new()),
        };

        Some(Self {
            project_id: project.id.clone(),
            name: project.name.clone(),
            kind,
            root: project.path.clone(),
            running: agent.is_some(),
            resumable: project.default_shell.as_ref().is_some_and(|shell| {
                // Not offered when "the latest conversation here" could be
                // another agent's: the daemon would refuse it anyway.
                let shares_dir = ws.projects().iter().any(|p| {
                    p.id != project.id
                        && p.path == project.path
                        && p.worktree_info.is_none()
                        && p.is_any_agent_session()
                });
                okena_app_core::workspace::actions::execute::agent_resume::resumable(
                    shell, shares_dir,
                )
            }),
            agent,
            mcp,
            waiting,
            idle,
            status: project.agent.as_ref().and_then(|a| a.status.clone()),
            reported,
            question: project.agent.as_ref().and_then(|a| a.question.clone()),
            suggestions: project
                .agent
                .as_ref()
                .map(|a| a.suggestions.clone())
                .unwrap_or_default(),
            assets: project
                .agent
                .as_ref()
                .map(|a| a.assets.clone())
                .unwrap_or_default(),
            workspaces,
            parent,
            children,
        })
    }

    /// A neighbouring session, summarized for a row.
    fn related_agent(
        ws: &Workspace,
        terminals: &okena_terminal::TerminalsRegistry,
        project: &crate::workspace::state::ProjectData,
    ) -> RelatedAgent {
        let (agent, _, waiting, idle) = Self::terminal_facts(ws, terminals, project);
        RelatedAgent {
            project_id: project.id.clone(),
            name: project.name.clone(),
            key: project.task_ref.as_ref().map(|t| t.display_key.clone()),
            activity: activity_of(
                agent.is_some(),
                waiting,
                idle,
                project.agent.as_ref().and_then(|a| a.state),
            ),
        }
    }

    /// Read the session's terminals: which agent is running, whether okena's
    /// MCP was wired in, and how the prompt is doing.
    ///
    /// "A terminal is alive" is deliberately not the question — a session left
    /// on a bare shell has a live terminal and no agent, which is exactly the
    /// case worth offering a restart for.
    fn terminal_facts(
        _ws: &Workspace,
        terminals: &okena_terminal::TerminalsRegistry,
        project: &crate::workspace::state::ProjectData,
    ) -> (Option<String>, bool, bool, String) {
        use okena_terminal::shell_config::ShellType;

        let Some(layout) = project.layout.as_ref() else {
            return (None, false, false, String::new());
        };
        let registry = terminals.lock();

        for id in layout.collect_terminal_ids() {
            // Resolve the pane's shell the way the spawn does: an unset pane
            // inherits the project's configured agent.
            let node_shell = layout
                .find_terminal_path(&id)
                .and_then(|path| layout.get_at_path(&path).cloned())
                .and_then(|node| match node {
                    okena_workspace::state::LayoutNode::Terminal { shell_type, .. } => {
                        Some(shell_type)
                    }
                    _ => None,
                })
                .unwrap_or_default();
            let shell = match node_shell {
                ShellType::Default => project.default_shell.clone().unwrap_or_default(),
                explicit => explicit,
            };
            let terminal = registry.get(&id);
            let title = terminal.and_then(|t| t.title());
            let Some(agent) = crate::views::agent_session::detect_agent(&shell, title.as_deref())
            else {
                continue;
            };
            let mcp = match &shell {
                ShellType::Custom { args, .. } => {
                    okena_app_core::workspace::actions::execute::agent_mcp::args_have_mcp(args)
                }
                // Detected by title alone: okena did not launch it, so it has
                // whatever MCP config its own environment gave it.
                _ => false,
            };
            return (
                Some(agent),
                mcp,
                terminal.is_some_and(|t| t.is_waiting_for_input()),
                terminal
                    .map(|t| t.idle_duration_display())
                    .unwrap_or_default(),
            );
        }
        (None, false, false, String::new())
    }

    /// What the session is doing right now. Stopped wins over waiting: a
    /// prompt with no agent behind it is a shell, not an agent waiting on you.
    pub fn activity(&self) -> SessionActivity {
        activity_of(self.running, self.waiting, self.idle.clone(), self.reported)
    }

    /// The terminal to show for this session, if any.
    pub fn visible_terminal_id(ws: &Workspace, project_id: &str) -> Option<String> {
        ws.project(project_id)?
            .layout
            .as_ref()
            .and_then(|l| l.visible_terminal_id())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgentSessionInfo, AgentSessionKind, SessionActivity, activity_of, is_related, session_kind,
    };
    use okena_core::harness::AgentState;
    use okena_core::tasks::{TaskId, TaskRef};

    fn info(running: bool, waiting: bool, idle: &str) -> AgentSessionInfo {
        AgentSessionInfo {
            project_id: "s1".into(),
            name: "s".into(),
            kind: AgentSessionKind::Plain,
            root: "/p".into(),
            agent: None,
            mcp: false,
            running,
            resumable: false,
            waiting,
            idle: idle.into(),
            status: None,
            reported: None,
            question: None,
            suggestions: Vec::new(),
            assets: Vec::new(),
            workspaces: Vec::new(),
            parent: None,
            children: Vec::new(),
        }
    }

    #[test]
    fn a_waiting_agent_that_says_why_needs_attention() {
        assert_eq!(
            activity_of(true, true, "2m".into(), Some(AgentState::ReadyForReview)),
            SessionActivity::NeedsAttention {
                reason: AgentState::ReadyForReview
            }
        );
    }

    #[test]
    fn a_stale_reason_does_not_flag_an_agent_that_is_working_again() {
        // Told to carry on, it produces output before it reports again. The
        // old "ready for review" must not keep it flagged in the meantime.
        assert_eq!(
            activity_of(true, false, String::new(), Some(AgentState::ReadyForReview)),
            SessionActivity::Running
        );
    }

    #[test]
    fn a_waiting_agent_with_no_reason_is_only_waiting() {
        assert_eq!(
            activity_of(true, true, "1m".into(), Some(AgentState::Working)).label(),
            "waiting · 1m"
        );
        assert!(!activity_of(true, true, String::new(), None).wants_attention());
    }

    #[test]
    fn no_agent_means_stopped_whatever_it_last_said() {
        assert_eq!(
            activity_of(false, true, String::new(), Some(AgentState::NeedsInput)),
            SessionActivity::Stopped
        );
    }

    #[test]
    fn a_prompt_with_no_agent_behind_it_is_stopped_not_waiting() {
        assert_eq!(info(false, true, "3m").activity(), SessionActivity::Stopped);
    }

    #[test]
    fn a_waiting_agent_says_how_long_it_has_waited() {
        assert_eq!(info(true, true, "3m").activity().label(), "waiting · 3m");
        assert_eq!(info(true, true, "").activity().label(), "waiting");
        assert_eq!(info(true, false, "").activity(), SessionActivity::Running);
    }

    fn task(external: &str, key: &str) -> TaskRef {
        TaskRef {
            id: TaskId::new("linear", external),
            display_key: key.to_string(),
            title: "Title".to_string(),
            url: "http://x".to_string(),
            parent_id: None,
            parent_key: None,
        }
    }

    fn project(json: serde_json::Value) -> crate::workspace::state::ProjectData {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_worktree_on_the_same_task_is_related() {
        assert!(is_related("s1", "u1", "wt1", Some(&task("u1", "QBL-1"))));
    }

    #[test]
    fn the_session_is_not_related_to_itself() {
        // It carries the same task as its worktrees, so it would otherwise
        // list itself as one of its own checkouts.
        assert!(!is_related("s1", "u1", "s1", Some(&task("u1", "QBL-1"))));
    }

    #[test]
    fn a_different_task_is_not_related() {
        assert!(!is_related("s1", "u1", "wt1", Some(&task("u9", "QBL-9"))));
    }

    #[test]
    fn an_unlinked_project_is_not_related() {
        assert!(!is_related("s1", "u1", "p1", None));
    }

    #[test]
    fn matching_is_by_provider_id_not_display_key() {
        // The display key changes when an issue moves team; the provider id
        // does not. Matching on the key would silently drop the worktrees.
        let moved = TaskRef {
            id: TaskId::new("linear", "u1"),
            display_key: "NEW-7".to_string(),
            title: "Title".to_string(),
            url: "http://x".to_string(),
            parent_id: None,
            parent_key: None,
        };
        assert!(is_related("s1", "u1", "wt1", Some(&moved)));
    }

    #[test]
    fn a_spec_session_is_classified_as_spec() {
        let p = project(serde_json::json!({
            "id": "s1", "name": "add-login (spec)", "path": "/specs",
            "spec_change": "add-login",
        }));
        assert_eq!(
            session_kind(&p),
            Some(AgentSessionKind::Spec {
                change: "add-login".into()
            })
        );
    }

    #[test]
    fn a_task_session_is_classified_as_task() {
        let p = project(serde_json::json!({
            "id": "s1", "name": "QBL-1 (agent)", "path": "/p",
            "task_ref": {
                "id": { "provider": "linear", "external_id": "u1" },
                "display_key": "QBL-1", "title": "t", "url": "http://x",
            },
        }));
        assert!(matches!(session_kind(&p), Some(AgentSessionKind::Task(_))));
    }

    #[test]
    fn a_worktree_is_not_a_session_even_carrying_a_task() {
        // Its checkout is the work, not a place an agent was started.
        let p = project(serde_json::json!({
            "id": "wt1", "name": "okena (QBL-1)", "path": "/p/wt",
            "worktree_info": {
                "parent_project_id": "repo1",
                "main_repo_path": "/p/okena",
                "worktree_path": "/p/wt",
                "branch_name": "feat/x",
            },
            "task_ref": {
                "id": { "provider": "linear", "external_id": "u1" },
                "display_key": "QBL-1", "title": "t", "url": "http://x",
            },
        }));
        assert_eq!(session_kind(&p), None);
    }

    #[test]
    fn an_ordinary_project_is_not_a_session() {
        let p = project(serde_json::json!({
            "id": "p1", "name": "okena", "path": "/p/okena",
        }));
        assert_eq!(session_kind(&p), None);
    }

    #[test]
    fn each_kind_names_its_own_empty_state() {
        // "No worktrees" means something different for a spec writer, which
        // never has any, than for a task agent, where the checkouts are gone.
        let spec = AgentSessionKind::Spec { change: "x".into() };
        assert_ne!(
            spec.empty_workspace_note(),
            AgentSessionKind::Task(task("u1", "QBL-1")).empty_workspace_note()
        );
        assert_eq!(spec.workspace_heading(), "DOCUMENTS");
    }
}
