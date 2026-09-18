//! What each agent is doing, decided next to its PTY.
//!
//! Clients used to guess this from a pane view's idle loop, which only ran for
//! a mounted pane, was off by default and almost never fired for an agent. The
//! daemon now keeps every agent terminal's signals and resolves them with
//! [`okena_core::agent_activity::resolve`]; clients read the result from
//! `ApiProject::agent_activity`, so an agent no pane shows — or one on a remote
//! daemon — updates like any other.
//!
//! Signals arrive from three places:
//!
//! * the command loop — native hook events (`ActionRequest::AgentHookEvent`);
//! * the PTY loop — bells and OSC notifications;
//! * the terminals themselves — last input and last output, read on refresh.
//!
//! [`run_agent_activity_poll`] re-resolves on a short tick, since going quiet
//! is itself a signal, and at once when a hook event arrives. It bumps
//! `state_version` only when an activity changed.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use okena_core::agent_activity::{
    AgentActivity, AgentHookEvent, AgentSignals, Report, report_is_stale, resolve,
};
use okena_hooks::{HookMonitor, HookRunner};
use okena_terminal::TerminalsRegistry;
use okena_workspace::context::WorkspaceCx;
use okena_workspace::state::Workspace;
use parking_lot::Mutex;
use tokio::sync::{Notify, watch};

use crate::workspace_cx::DaemonWorkspaceCx;

/// How often activity is re-resolved without a hook event to prompt it. Short
/// against [`okena_core::agent_activity::QUIET_PERIOD_MS`], so an agent that
/// went quiet shows as waiting soon after.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// The signals that arrive as events rather than being read off a terminal.
#[derive(Clone, Copy, Debug, Default)]
struct Received {
    native: Option<(AgentHookEvent, u64)>,
    attention_at: Option<u64>,
}

/// Every agent terminal's signals, and the activity last published for each.
#[derive(Default)]
pub struct AgentActivityTracker {
    /// By terminal id. Kept for any live terminal: whether it runs an agent is
    /// decided on refresh, since a title can change.
    received: Mutex<HashMap<String, Received>>,
    /// Terminal id → activity, for terminals running an agent.
    published: Mutex<HashMap<String, AgentActivity>>,
    /// Wakes the poll when a hook event arrives, so it shows without waiting
    /// for the next tick.
    wake: Notify,
}

impl AgentActivityTracker {
    /// Record a lifecycle event the agent in `terminal_id` fired.
    pub fn record_hook_event(&self, terminal_id: &str, event: AgentHookEvent) {
        self.received
            .lock()
            .entry(terminal_id.to_string())
            .or_default()
            .native = Some((event, unix_millis()));
        self.wake.notify_one();
    }

    /// Record a bell or OSC notification from `terminal_id`.
    pub fn record_attention(&self, terminal_id: &str) {
        self.received
            .lock()
            .entry(terminal_id.to_string())
            .or_default()
            .attention_at = Some(unix_millis());
    }

    /// Terminal id → activity, as last published.
    pub fn activities(&self) -> HashMap<String, AgentActivity> {
        self.published.lock().clone()
    }

    /// Re-resolve every agent terminal, and clear the question and suggestions
    /// of any agent answered since it reported. Returns whether any activity
    /// changed.
    pub fn refresh(
        &self,
        workspace: &Mutex<Workspace>,
        terminals: &TerminalsRegistry,
        cx: &mut impl WorkspaceCx,
    ) -> bool {
        let now = unix_millis();
        // Out of the registry first, so its lock is never held with the
        // workspace's — the PTY loop takes them in the other order.
        let live: HashMap<String, Arc<okena_terminal::terminal::Terminal>> = terminals
            .lock()
            .iter()
            .map(|(id, terminal)| (id.clone(), terminal.clone()))
            .collect();
        let received = {
            let mut received = self.received.lock();
            received.retain(|id, _| live.contains_key(id));
            received.clone()
        };

        let mut next = HashMap::new();
        let mut answered: Vec<String> = Vec::new();
        {
            let ws = workspace.lock();
            for project in ws.projects() {
                let Some(layout) = project.layout.as_ref() else {
                    continue;
                };
                let agent = project.agent.as_ref();
                let report = agent.and_then(|a| {
                    a.state.map(|state| Report {
                        state,
                        reported_at: a.reported_at,
                    })
                });
                for terminal_id in layout.collect_terminal_ids() {
                    let Some(terminal) = live.get(&terminal_id) else {
                        continue;
                    };
                    if project
                        .terminal_agent(&terminal_id, terminal.title().as_deref())
                        .is_none()
                    {
                        continue;
                    }
                    let r = received.get(&terminal_id).copied().unwrap_or_default();
                    let output_ago = terminal.last_output_time().elapsed().as_millis() as u64;
                    let signals = AgentSignals {
                        native: r.native,
                        attention_at: r.attention_at,
                        input_at: terminal.last_input_at(),
                        output_at: Some(now.saturating_sub(output_ago)),
                    };
                    next.insert(terminal_id, resolve(true, &signals, report, now));

                    let asks_something = agent.is_some_and(|a| {
                        a.state.is_some() || a.question.is_some() || !a.suggestions.is_empty()
                    });
                    if asks_something
                        && report_is_stale(agent.and_then(|a| a.reported_at), signals.input_at)
                        && !answered.contains(&project.id)
                    {
                        answered.push(project.id.clone());
                    }
                }
            }
        }

        if !answered.is_empty() {
            let mut ws = workspace.lock();
            for project_id in &answered {
                if let Some(agent) = ws
                    .data
                    .projects
                    .iter_mut()
                    .find(|p| &p.id == project_id)
                    .and_then(|p| p.agent.as_mut())
                {
                    // The status line stays: it is still the last thing it said.
                    agent.state = None;
                    agent.question = None;
                    agent.suggestions.clear();
                }
            }
            ws.notify_data(cx);
        }

        let mut published = self.published.lock();
        if *published == next {
            false
        } else {
            *published = next;
            true
        }
    }
}

/// Keep agent activity current until the daemon shuts down.
///
/// Must run on the daemon's `LocalSet`, like the other reactor tasks.
pub async fn run_agent_activity_poll(
    tracker: Arc<AgentActivityTracker>,
    workspace: Arc<Mutex<Workspace>>,
    terminals: TerminalsRegistry,
    workspace_tick: watch::Sender<u64>,
    hook_runner: Option<HookRunner>,
    hook_monitor: Option<HookMonitor>,
    state_version: watch::Sender<u64>,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(POLL_INTERVAL) => {}
            _ = tracker.wake.notified() => {}
        }
        let mut cx = DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
        if tracker.refresh(&workspace, &terminals, &mut cx) {
            state_version.send_modify(|v| *v += 1);
        }
    }
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::AgentActivityTracker;
    use crate::test_support::{StubTransport, empty_workspace_data};
    use crate::workspace_cx::DaemonWorkspaceCx;
    use okena_core::agent_activity::{AgentActivity, AgentHookEvent};
    use okena_core::harness::{AgentSessionState, AgentState, AgentSuggestion};
    use okena_terminal::TerminalsRegistry;
    use okena_terminal::shell_config::ShellType;
    use okena_terminal::terminal::{Terminal, TerminalSize};
    use okena_workspace::state::{LayoutNode, ProjectData, Workspace};
    use parking_lot::Mutex;
    use std::sync::Arc;
    use tokio::sync::watch;

    /// An agent session running `command` in terminal `t1`. Not open in any
    /// pane: the daemon has none.
    fn session(command: &str) -> ProjectData {
        let mut project: ProjectData = serde_json::from_value(serde_json::json!({
            "id": "s1", "name": "session", "path": "/tmp", "custom_session": "do the thing",
        }))
        .expect("project");
        project.layout = Some(LayoutNode::Terminal {
            terminal_id: Some("t1".into()),
            minimized: false,
            detached: false,
            shell_type: ShellType::Custom {
                path: command.into(),
                args: Vec::new(),
            },
            zoom_level: 1.0,
            agent: false,
        });
        project
    }

    struct Fixture {
        tracker: AgentActivityTracker,
        workspace: Mutex<Workspace>,
        terminals: TerminalsRegistry,
        terminal: Arc<Terminal>,
        tick: watch::Sender<u64>,
    }

    impl Fixture {
        fn new(project: ProjectData) -> Self {
            let mut data = empty_workspace_data();
            data.project_order.push(project.id.clone());
            data.projects.push(project);
            let terminal = Arc::new(Terminal::new(
                "t1".into(),
                TerminalSize {
                    cols: 80,
                    rows: 24,
                    cell_width: 8.0,
                    cell_height: 16.0,
                },
                Arc::new(StubTransport),
                "/tmp".into(),
            ));
            let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
            terminals.lock().insert("t1".into(), terminal.clone());
            Self {
                tracker: AgentActivityTracker::default(),
                workspace: Mutex::new(Workspace::new(data)),
                terminals,
                terminal,
                tick: watch::channel(0u64).0,
            }
        }

        fn refresh(&self) -> Option<AgentActivity> {
            let mut cx = DaemonWorkspaceCx::new(&self.tick, &None, &None);
            self.tracker
                .refresh(&self.workspace, &self.terminals, &mut cx);
            self.tracker.activities().get("t1").copied()
        }

        fn agent(&self) -> Option<AgentSessionState> {
            self.workspace
                .lock()
                .project("s1")
                .and_then(|p| p.agent.clone())
        }
    }

    /// Enough that two stamps taken either side of it differ.
    fn tick() {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    #[test]
    fn hook_events_decide_what_the_agent_is_doing() {
        let f = Fixture::new(session("claude"));
        for (event, expected) in [
            (AgentHookEvent::TurnStarted, AgentActivity::Working),
            (AgentHookEvent::NeedsInput, AgentActivity::NeedsInput),
            (AgentHookEvent::ToolActivity, AgentActivity::Working),
            (AgentHookEvent::TurnEnded, AgentActivity::Waiting),
        ] {
            f.tracker.record_hook_event("t1", event);
            assert_eq!(f.refresh(), Some(expected), "after {event:?}");
        }
    }

    #[test]
    fn only_a_change_is_reported() {
        let f = Fixture::new(session("claude"));
        f.tracker
            .record_hook_event("t1", AgentHookEvent::TurnStarted);
        let mut cx = DaemonWorkspaceCx::new(&f.tick, &None, &None);
        assert!(f.tracker.refresh(&f.workspace, &f.terminals, &mut cx));
        f.tracker
            .record_hook_event("t1", AgentHookEvent::ToolActivity);
        assert!(
            !f.tracker.refresh(&f.workspace, &f.terminals, &mut cx),
            "working → working is no change for clients to resync on"
        );
    }

    #[test]
    fn a_report_says_why_a_finished_turn_stopped() {
        let mut project = session("claude");
        project.agent = Some(AgentSessionState {
            status: Some("PR ready".into()),
            state: Some(AgentState::ReadyForReview),
            reported_at: Some(super::unix_millis()),
            ..Default::default()
        });
        let f = Fixture::new(project);
        tick();
        f.tracker.record_hook_event("t1", AgentHookEvent::TurnEnded);
        assert_eq!(f.refresh(), Some(AgentActivity::ReadyForReview));
    }

    #[test]
    fn typing_a_reply_clears_the_question_and_the_agent_works_again() {
        let mut project = session("claude");
        project.agent = Some(AgentSessionState {
            status: Some("waiting on you".into()),
            state: Some(AgentState::NeedsInput),
            question: Some("Which database?".into()),
            suggestions: vec![AgentSuggestion {
                label: "Postgres".into(),
                instruction: "Use Postgres".into(),
            }],
            reported_at: Some(super::unix_millis()),
            ..Default::default()
        });
        let f = Fixture::new(project);
        tick();
        f.tracker.record_hook_event("t1", AgentHookEvent::TurnEnded);
        assert_eq!(f.refresh(), Some(AgentActivity::NeedsInput));
        assert!(f.agent().and_then(|a| a.question).is_some());

        tick();
        f.terminal.send_input("Postgres\r");
        // The agent echoes and starts on it.
        f.terminal.process_output(b"Postgres\r\n");
        assert_eq!(f.refresh(), Some(AgentActivity::Working));
        let agent = f.agent().expect("agent state");
        assert_eq!(agent.state, None);
        assert_eq!(agent.question, None);
        assert!(agent.suggestions.is_empty());
        assert_eq!(agent.status.as_deref(), Some("waiting on you"));
    }

    #[test]
    fn focusing_the_pane_does_not_answer_the_agent() {
        let mut project = session("claude");
        project.agent = Some(AgentSessionState {
            state: Some(AgentState::NeedsInput),
            question: Some("Which database?".into()),
            reported_at: Some(super::unix_millis()),
            ..Default::default()
        });
        let f = Fixture::new(project);
        tick();
        f.terminal.send_bytes(b"\x1b[I");
        f.refresh();
        assert!(f.agent().and_then(|a| a.question).is_some());
    }

    #[test]
    fn a_bell_from_an_agent_without_hooks_needs_input() {
        let f = Fixture::new(session("aider"));
        f.tracker.record_attention("t1");
        assert_eq!(f.refresh(), Some(AgentActivity::NeedsInput));
    }

    #[test]
    fn an_agent_without_hooks_is_working_while_it_writes() {
        let f = Fixture::new(session("copilot"));
        f.terminal.process_output(b"thinking...");
        assert_eq!(f.refresh(), Some(AgentActivity::Working));
    }

    #[test]
    fn a_plain_shell_has_no_agent_activity() {
        let f = Fixture::new(session("/bin/zsh"));
        f.tracker
            .record_hook_event("t1", AgentHookEvent::NeedsInput);
        assert_eq!(f.refresh(), None);
    }

    #[test]
    fn an_exited_agent_drops_out() {
        let f = Fixture::new(session("claude"));
        f.tracker
            .record_hook_event("t1", AgentHookEvent::TurnStarted);
        assert_eq!(f.refresh(), Some(AgentActivity::Working));
        f.terminals.lock().remove("t1");
        assert_eq!(f.refresh(), None);
        assert!(f.tracker.received.lock().is_empty());
    }
}
