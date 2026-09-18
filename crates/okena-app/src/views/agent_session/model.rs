//! What okena knows about one agent session, independent of how it is shown.
//!
//! Collected from the workspace mirror in one place so the sidebar beside a
//! session's terminal and the cards in the Agents overview cannot drift apart —
//! they were separately written and had already started to.

use crate::workspace::state::Workspace;
use crate::workspace::state::agent_links::is_related;
use okena_core::agent_activity::AgentActivity;
use okena_core::harness::{AgentState, AgentSuggestion};
use okena_core::session_assets::{LinkedCheckout, SessionAsset, derive_session_assets};
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

/// How a closed session can be brought back, as its closed view offers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReopenChoice {
    /// Resume the conversation the agent was having.
    Resume,
    /// okena cannot resume this agent: start a new conversation from its brief.
    StartAgain,
    /// Its working directory is gone — its worktree was removed — so it can
    /// be neither resumed nor started there.
    WorktreeRemoved,
}

impl ReopenChoice {
    /// What the button says.
    pub fn label(self) -> &'static str {
        match self {
            ReopenChoice::Resume => "Resume conversation",
            ReopenChoice::StartAgain => "Start again from its brief",
            ReopenChoice::WorktreeRemoved => "Its worktree was removed",
        }
    }

    /// Whether the button does anything.
    pub fn enabled(self) -> bool {
        !matches!(self, ReopenChoice::WorktreeRemoved)
    }

    /// Whether reopening starts a new conversation rather than resuming.
    pub fn fresh(self) -> bool {
        matches!(self, ReopenChoice::StartAgain)
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
    /// The extension that started it, and the item it is about: e.g.
    /// `("cli-table", Some("job-7 (acme)"))`.
    pub origin: Option<(String, Option<String>)>,
    /// Every task the session covers: the one it is named after, then the
    /// others picked with it. Empty unless it is a task session.
    pub tasks: Vec<TaskRef>,
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
    /// What the agent is doing, as the daemon decided it from the agent's own
    /// signals. `None` from a daemon that predates agent activity.
    pub live: Option<AgentActivity>,
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
    /// What it produced: the branches and PRs okena detects on its worktrees,
    /// merged with what the agent registered.
    pub assets: Vec<SessionAsset>,
    pub workspaces: Vec<RelatedWorkspace>,
    /// The agent on this task's parent, when one is running.
    pub parent: Option<RelatedAgent>,
    /// Agents on this task's sub-tasks.
    pub children: Vec<RelatedAgent>,
    /// When the session was closed, in Unix millis; `None` while it is open.
    pub closed_at: Option<u64>,
    /// Whether a closed session's working directory is gone, as the daemon
    /// that owns the disk said.
    pub cwd_missing: bool,
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

/// What a session is doing right now, as the panel shows it.
///
/// Decided by the daemon from the agent's own signals — its hooks, its
/// terminal, and only then its report — so an agent that never reports still
/// shows as waiting, or as needing you on a permission prompt.
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

/// The panel's view of what the daemon says the agent is doing.
///
/// A running agent the daemon has not described — one on a daemon from before
/// agent activity — shows as running, which is what it was always shown as.
pub(super) fn activity_of(
    running: bool,
    live: Option<AgentActivity>,
    idle: String,
) -> SessionActivity {
    if !running {
        return SessionActivity::Stopped;
    }
    let reason = |reason| SessionActivity::NeedsAttention { reason };
    match live.unwrap_or(AgentActivity::Working) {
        AgentActivity::Working => SessionActivity::Running,
        AgentActivity::Stopped => SessionActivity::Stopped,
        AgentActivity::NeedsInput => reason(AgentState::NeedsInput),
        AgentActivity::ReadyForReview => reason(AgentState::ReadyForReview),
        AgentActivity::Blocked => reason(AgentState::Blocked),
        AgentActivity::Unknown => reason(AgentState::Unknown),
        AgentActivity::Waiting | AgentActivity::Done => SessionActivity::Waiting { idle },
    }
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

/// Every task a task session covers, its own first.
///
/// One agent can be started on several picked tasks. The kind names only the
/// first, which the branch and the session are named after, so a panel reading
/// the kind alone looked as if the rest had been dropped.
pub(super) fn session_tasks(project: &crate::workspace::state::ProjectData) -> Vec<TaskRef> {
    match session_kind(project) {
        Some(AgentSessionKind::Task(_)) => project.linked_tasks().cloned().collect(),
        _ => Vec::new(),
    }
}

/// A session's task keys for a narrow row: the first, and how many more.
pub(super) fn tasks_key(project: &crate::workspace::state::ProjectData) -> Option<String> {
    let mut tasks = project.linked_tasks();
    let first = tasks.next()?.display_key.clone();
    Some(match tasks.count() {
        0 => first,
        more => format!("{first} +{more}"),
    })
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
        let session_task_ids: Vec<String> = project
            .linked_tasks()
            .map(|t| t.id.external_id.clone())
            .collect();
        let workspaces: Vec<RelatedWorkspace> = match &kind {
            AgentSessionKind::Task(_) => ws
                .projects()
                .iter()
                .filter(|p| is_related(project_id, &session_task_ids, &p.id, p.linked_tasks()))
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

        let (agent, mcp, live, idle) = Self::terminal_facts(ws, terminals, project);
        let reported = project.agent.as_ref().and_then(|a| a.state);

        // What it produced. Derived here, each time it is read, from the same
        // mirror a remote session's snapshot fills — so a branch's push state
        // cannot go stale and a registered PR cannot show twice.
        let checkouts: Vec<LinkedCheckout<'_>> = workspaces
            .iter()
            .filter_map(|w| {
                let p = ws.project(&w.project_id)?;
                let info = p.worktree_info.as_ref()?;
                Some(LinkedCheckout {
                    project: ws
                        .project(&info.parent_project_id)
                        .map_or(p.name.as_str(), |parent| parent.name.as_str()),
                    branch: Some(info.branch_name.as_str()),
                    git: ws
                        .remote_snapshot(&p.id)
                        .and_then(|s| s.git_status.as_ref()),
                })
            })
            .collect();
        let assets = match project.agent.as_ref() {
            Some(a) => {
                derive_session_assets(&a.assets, &checkouts, &a.tracked_prs, &a.pushed_branches)
            }
            None => derive_session_assets(&[], &checkouts, &[], &[]),
        };

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

        let origin = match &project.agent_purpose {
            Some(okena_core::harness::AgentPurpose::Extension {
                extension,
                item,
                item_label,
            }) => Some((extension.clone(), item_label.clone().or_else(|| item.clone()))),
            _ => None,
        };
        Some(Self {
            project_id: project.id.clone(),
            name: project.name.clone(),
            origin,
            tasks: session_tasks(project),
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
            live,
            idle,
            status: project.agent.as_ref().and_then(|a| a.status.clone()),
            reported,
            question: project.agent.as_ref().and_then(|a| a.question.clone()),
            suggestions: project
                .agent
                .as_ref()
                .map(|a| a.suggestions.clone())
                .unwrap_or_default(),
            assets,
            workspaces,
            parent,
            children,
            closed_at: project.closed_at,
            cwd_missing: ws
                .remote_snapshot(project_id)
                .is_some_and(|s| s.cwd_missing),
        })
    }

    /// A neighbouring session, summarized for a row.
    fn related_agent(
        ws: &Workspace,
        terminals: &okena_terminal::TerminalsRegistry,
        project: &crate::workspace::state::ProjectData,
    ) -> RelatedAgent {
        let (agent, _, live, idle) = Self::terminal_facts(ws, terminals, project);
        RelatedAgent {
            project_id: project.id.clone(),
            name: project.name.clone(),
            key: tasks_key(project),
            activity: activity_of(agent.is_some(), live, idle),
        }
    }

    /// Read the session's terminals: which agent is running, whether okena's
    /// MCP was wired in, and what the daemon says it is doing.
    ///
    /// "A terminal is alive" is deliberately not the question — a session left
    /// on a bare shell has a live terminal and no agent, which is exactly the
    /// case worth offering a restart for.
    fn terminal_facts(
        ws: &Workspace,
        terminals: &okena_terminal::TerminalsRegistry,
        project: &crate::workspace::state::ProjectData,
    ) -> (Option<String>, bool, Option<AgentActivity>, String) {
        use okena_terminal::shell_config::ShellType;

        let Some(layout) = project.layout.as_ref() else {
            return (None, false, None, String::new());
        };
        let registry = terminals.lock();

        // The agent's own pane when the session has one: its other panes are
        // shells, whatever someone runs in them.
        let candidates = if layout.agent_terminal_path().is_some() {
            layout.agent_terminal_id().into_iter().collect()
        } else {
            layout.collect_terminal_ids()
        };
        for id in candidates {
            let terminal = registry.get(&id);
            let title = terminal.and_then(|t| t.title());
            let Some(agent) = project.terminal_agent(&id, title.as_deref()) else {
                continue;
            };
            let shell = project.terminal_shell(&id);
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
                terminal.and(ws.agent_activity(&project.id, &id)),
                terminal
                    .map(|t| t.idle_duration_display())
                    .unwrap_or_default(),
            );
        }
        (None, false, None, String::new())
    }

    /// What the session is doing right now. Stopped wins over waiting: a
    /// prompt with no agent behind it is a shell, not an agent waiting on you.
    pub fn activity(&self) -> SessionActivity {
        activity_of(self.running, self.live, self.idle.clone())
    }

    /// Whether the session was closed, and so is shown as history.
    pub fn closed(&self) -> bool {
        self.closed_at.is_some()
    }

    /// How the closed view offers to bring this session back.
    ///
    /// Resuming is the point of keeping a closed session, so it is offered
    /// whenever okena can; a fresh start only when it cannot. A missing
    /// directory rules out both, and says so rather than hiding the button.
    pub fn reopen_choice(&self) -> ReopenChoice {
        if self.cwd_missing {
            ReopenChoice::WorktreeRemoved
        } else if self.resumable {
            ReopenChoice::Resume
        } else {
            ReopenChoice::StartAgain
        }
    }

    /// The line identifying what the session works on, for a card.
    ///
    /// A session on several picked tasks says how many more beside the first,
    /// right after its key: a long title is cut at the card's edge, and the
    /// count must not be what goes.
    pub fn subject(&self) -> Option<String> {
        let Some((first, rest)) = self.tasks.split_first() else {
            return self.kind.subject();
        };
        Some(match rest.len() {
            0 => format!("{} — {}", first.display_key, first.title),
            more => format!("{} +{more} — {}", first.display_key, first.title),
        })
    }

    /// The terminal to show for this session, if any: its agent's, or none
    /// while the agent is stopped. A session started without an agent shows
    /// whatever terminal is in front.
    pub fn visible_terminal_id(ws: &Workspace, project_id: &str) -> Option<String> {
        let layout = ws.project(project_id)?.layout.as_ref()?;
        if layout.agent_terminal_path().is_some() {
            return layout.agent_terminal_id();
        }
        layout.visible_terminal_id()
    }
}

/// The last `keep` parts of a path, behind an ellipsis when anything was cut.
///
/// A working directory is recognised by its end — the worktree folder — while
/// its start is the same home directory on every row, so the end is what a
/// narrow panel should spend its width on.
pub fn short_path(path: &str, keep: usize) -> String {
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    if keep == 0 || parts.len() <= keep {
        return path.to_string();
    }
    format!("…/{}", parts[parts.len() - keep..].join("/"))
}

#[cfg(test)]
mod tests {
    use super::{
        AgentSessionInfo, AgentSessionKind, ReopenChoice, SessionActivity, activity_of,
        session_kind, session_tasks, short_path, tasks_key,
    };
    use okena_core::agent_activity::AgentActivity;
    use okena_core::harness::AgentState;
    use okena_core::tasks::{TaskId, TaskRef};

    fn info(running: bool, live: Option<AgentActivity>, idle: &str) -> AgentSessionInfo {
        AgentSessionInfo {
            project_id: "s1".into(),
            name: "s".into(),
            kind: AgentSessionKind::Plain,
            origin: None,
            tasks: Vec::new(),
            root: "/p".into(),
            agent: None,
            mcp: false,
            running,
            resumable: false,
            live,
            idle: idle.into(),
            status: None,
            reported: None,
            question: None,
            suggestions: Vec::new(),
            assets: Vec::new(),
            workspaces: Vec::new(),
            parent: None,
            children: Vec::new(),
            closed_at: None,
            cwd_missing: false,
        }
    }

    #[test]
    fn a_closed_session_resumes_when_okena_can() {
        let mut s = info(false, None, "");
        s.closed_at = Some(1);
        s.resumable = true;
        assert_eq!(s.reopen_choice(), ReopenChoice::Resume);
        assert!(!s.reopen_choice().fresh());
        assert_eq!(s.reopen_choice().label(), "Resume conversation");
    }

    #[test]
    fn an_agent_okena_cannot_resume_starts_again_from_its_brief() {
        let mut s = info(false, None, "");
        s.closed_at = Some(1);
        s.resumable = false;
        assert_eq!(s.reopen_choice(), ReopenChoice::StartAgain);
        assert!(s.reopen_choice().fresh() && s.reopen_choice().enabled());
    }

    #[test]
    fn a_removed_worktree_disables_reopening_and_says_why() {
        for resumable in [true, false] {
            let mut s = info(false, None, "");
            s.closed_at = Some(1);
            s.resumable = resumable;
            s.cwd_missing = true;
            assert_eq!(s.reopen_choice(), ReopenChoice::WorktreeRemoved);
            assert!(!s.reopen_choice().enabled());
            assert_eq!(s.reopen_choice().label(), "Its worktree was removed");
        }
    }

    #[test]
    fn an_agent_that_stopped_for_a_reason_needs_attention() {
        assert_eq!(
            activity_of(true, Some(AgentActivity::ReadyForReview), "2m".into()),
            SessionActivity::NeedsAttention {
                reason: AgentState::ReadyForReview
            }
        );
        assert_eq!(
            activity_of(true, Some(AgentActivity::NeedsInput), String::new()),
            SessionActivity::NeedsAttention {
                reason: AgentState::NeedsInput
            }
        );
    }

    #[test]
    fn a_working_agent_is_running() {
        assert_eq!(
            activity_of(true, Some(AgentActivity::Working), String::new()),
            SessionActivity::Running
        );
    }

    #[test]
    fn a_waiting_agent_with_no_reason_is_only_waiting() {
        assert_eq!(
            activity_of(true, Some(AgentActivity::Waiting), "1m".into()).label(),
            "waiting · 1m"
        );
        assert!(!activity_of(true, Some(AgentActivity::Done), String::new()).wants_attention());
    }

    #[test]
    fn no_agent_means_stopped_whatever_it_last_said() {
        assert_eq!(
            activity_of(false, Some(AgentActivity::NeedsInput), String::new()),
            SessionActivity::Stopped
        );
    }

    #[test]
    fn a_prompt_with_no_agent_behind_it_is_stopped_not_waiting() {
        assert_eq!(
            info(false, Some(AgentActivity::Waiting), "3m").activity(),
            SessionActivity::Stopped
        );
    }

    #[test]
    fn a_waiting_agent_says_how_long_it_has_waited() {
        let waiting = Some(AgentActivity::Waiting);
        assert_eq!(info(true, waiting, "3m").activity().label(), "waiting · 3m");
        assert_eq!(info(true, waiting, "").activity().label(), "waiting");
        assert_eq!(info(true, None, "").activity(), SessionActivity::Running);
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

    fn task_session(also: &[(&str, &str)]) -> crate::workspace::state::ProjectData {
        let mut p = project(serde_json::json!({
            "id": "s1", "name": "QBL-1", "path": "/p",
        }));
        p.task_ref = Some(task("u1", "QBL-1"));
        p.also_tasks = also.iter().map(|(id, key)| task(id, key)).collect();
        p
    }

    #[test]
    fn a_session_on_several_picked_tasks_shows_every_one_first_task_first() {
        let p = task_session(&[("u2", "QBL-2"), ("u3", "QBL-3")]);
        let keys: Vec<String> = session_tasks(&p)
            .into_iter()
            .map(|t| t.display_key)
            .collect();
        assert_eq!(keys, ["QBL-1", "QBL-2", "QBL-3"]);
        assert_eq!(tasks_key(&p).as_deref(), Some("QBL-1 +2"));
    }

    #[test]
    fn a_session_on_one_task_shows_exactly_that_one() {
        let p = task_session(&[]);
        let keys: Vec<String> = session_tasks(&p)
            .into_iter()
            .map(|t| t.display_key)
            .collect();
        assert_eq!(keys, ["QBL-1"]);
        assert_eq!(tasks_key(&p).as_deref(), Some("QBL-1"));
    }

    #[test]
    fn a_card_on_several_picked_tasks_counts_the_others_beside_the_key() {
        let mut s = info(true, None, "");
        s.kind = AgentSessionKind::Task(task("u1", "QBL-1"));
        s.tasks = vec![
            task("u1", "QBL-1"),
            task("u2", "QBL-2"),
            task("u3", "QBL-3"),
        ];
        assert_eq!(s.subject().as_deref(), Some("QBL-1 +2 — Title"));
    }

    #[test]
    fn a_card_on_one_task_reads_as_it_did() {
        let mut s = info(true, None, "");
        s.kind = AgentSessionKind::Task(task("u1", "QBL-1"));
        s.tasks = vec![task("u1", "QBL-1")];
        assert_eq!(s.subject(), s.kind.subject());
        assert_eq!(s.subject().as_deref(), Some("QBL-1 — Title"));
    }

    #[test]
    fn a_card_not_on_a_task_keeps_its_kind_subject() {
        let mut s = info(true, None, "");
        s.kind = AgentSessionKind::Spec {
            change: "add-login".into(),
        };
        assert_eq!(s.subject().as_deref(), Some("add-login"));
    }

    #[test]
    fn a_session_that_is_not_on_a_task_shows_no_tasks() {
        let p = project(serde_json::json!({
            "id": "s1", "name": "add-login (spec)", "path": "/specs",
            "spec_change": "add-login",
        }));
        assert!(session_tasks(&p).is_empty());
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

    #[test]
    fn a_long_path_keeps_its_end() {
        assert_eq!(
            short_path("/Users/me/p/okena-wt/qbl-372-harness", 2),
            "…/okena-wt/qbl-372-harness"
        );
    }

    #[test]
    fn a_short_path_is_left_alone() {
        assert_eq!(short_path("/tmp/x", 2), "/tmp/x");
        assert_eq!(short_path(r"C:\work", 2), r"C:\work");
        assert_eq!(short_path(r"C:\work\wt\repo", 2), "…/wt/repo");
    }
}
