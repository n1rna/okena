//! An agent session's panes: only the agent's own pane runs the agent, and
//! the session's agent controls act on that pane alone.

use super::{AppSettings, spawn_uninitialized_terminals};
use crate::workspace::focus::FocusManager;
use crate::workspace::settings::HooksConfig;
use crate::workspace::state::{LayoutNode, ProjectData, WindowState, Workspace, WorkspaceData};
use okena_core::types::SplitDirection;
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::{TerminalBackend, TerminalLaunchPlan};
use okena_terminal::shell_config::ShellType;
use okena_terminal::terminal::TerminalTransport;
use okena_workspace::context::WorkspaceCx;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

struct StubTransport;

impl TerminalTransport for StubTransport {
    fn send_input(&self, _terminal_id: &str, _data: &[u8]) {}
    fn resize(&self, _terminal_id: &str, _cols: u16, _rows: u16) {}
    fn uses_mouse_backend(&self) -> bool {
        false
    }
}

/// Hands out fresh ids and records what each spawn ran and what was killed.
#[derive(Default)]
struct RecordingBackend {
    next: Mutex<usize>,
    spawned: Mutex<Vec<(String, ShellType)>>,
    killed: Mutex<Vec<String>>,
}

impl RecordingBackend {
    fn spawned(&self) -> Vec<(String, ShellType)> {
        self.spawned.lock().expect("spawned lock").clone()
    }

    fn killed(&self) -> Vec<String> {
        self.killed.lock().expect("killed lock").clone()
    }
}

impl TerminalBackend for RecordingBackend {
    fn transport(&self) -> Arc<dyn TerminalTransport> {
        Arc::new(StubTransport)
    }

    fn create_terminal(&self, _cwd: &str, _shell: Option<&ShellType>) -> anyhow::Result<String> {
        unreachable!("spawns go through launch plans")
    }

    fn create_terminal_with_plan(
        &self,
        _cwd: &str,
        plan: &TerminalLaunchPlan,
    ) -> anyhow::Result<String> {
        let mut next = self.next.lock().expect("id lock");
        *next += 1;
        let id = format!("t{next}");
        self.spawned
            .lock()
            .expect("spawned lock")
            .push((id.clone(), plan.route.clone()));
        Ok(id)
    }

    fn reconnect_terminal(
        &self,
        terminal_id: &str,
        _cwd: &str,
        _shell: Option<&ShellType>,
    ) -> anyhow::Result<String> {
        Ok(terminal_id.to_string())
    }

    fn kill(&self, terminal_id: &str) {
        self.killed
            .lock()
            .expect("killed lock")
            .push(terminal_id.to_string());
    }
    fn capture_buffer(&self, _terminal_id: &str) -> Option<std::path::PathBuf> {
        None
    }
    fn supports_buffer_capture(&self) -> bool {
        false
    }
    fn is_remote(&self) -> bool {
        false
    }
    fn get_shell_pid(&self, _terminal_id: &str) -> Option<u32> {
        None
    }
    fn get_service_pids(&self, _terminal_id: &str) -> Vec<u32> {
        Vec::new()
    }
}

struct TestCx;

impl WorkspaceCx for TestCx {
    fn notify(&mut self) {}
    fn refresh_views(&mut self) {}
    fn hook_runner(&self) -> Option<crate::workspace::hooks::HookRunner> {
        None
    }
    fn hook_monitor(&self) -> Option<crate::workspace::hook_monitor::HookMonitor> {
        None
    }
}

fn agent_launch() -> ShellType {
    ShellType::Custom {
        path: "/usr/local/bin/claude".into(),
        args: vec!["--session-id".into(), "abc".into(), "Do the task".into()],
    }
}

fn global_shell() -> ShellType {
    ShellType::Custom {
        path: "/bin/global-shell".into(),
        args: Vec::new(),
    }
}

fn settings() -> AppSettings {
    let mut settings = AppSettings::default();
    settings.default_shell = global_shell();
    settings
}

fn project(session: bool, default_shell: Option<ShellType>, layout: LayoutNode) -> ProjectData {
    ProjectData {
        id: "session".into(),
        name: "Session".into(),
        path: "/work/tree".into(),
        layout: Some(layout),
        terminal_names: HashMap::new(),
        hidden_terminals: HashMap::new(),
        worktree_info: None,
        worktree_ids: Vec::new(),
        task_ref: None,
        also_tasks: Vec::new(),
        repo_ids: Vec::new(),
        spec_change: None,
        knowledge_root: None,
        project_scan: None,
        task_draft: None,
        custom_session: session.then(|| "Session".to_string()),
        agent_purpose: None,
        context_projects: Vec::new(),
        agent: None,
        folder_color: Default::default(),
        hooks: HooksConfig::default(),
        connection_id: None,
        service_terminals: HashMap::new(),
        default_shell,
        hook_terminals: HashMap::new(),
        pinned: false,
        last_activity_at: None,
        is_creating: false,
        is_closing: false,
        creating_progress: None,
        verification_runs: Vec::new(),
    }
}

fn workspace(project: ProjectData) -> Workspace {
    Workspace::new(WorkspaceData {
        version: 1,
        projects: vec![project],
        project_order: vec!["session".into()],
        folders: Vec::new(),
        service_panel_heights: HashMap::new(),
        hook_panel_heights: HashMap::new(),
        main_window: WindowState::default(),
        extra_windows: Vec::new(),
    })
}

fn pane(terminal_id: Option<&str>, agent: bool) -> LayoutNode {
    LayoutNode::Terminal {
        terminal_id: terminal_id.map(str::to_string),
        minimized: false,
        detached: false,
        shell_type: ShellType::Default,
        zoom_level: 1.0,
        agent,
    }
}

/// A session whose agent runs in `a`, alone in the layout.
fn running_session() -> Workspace {
    workspace(project(true, Some(agent_launch()), pane(Some("a"), true)))
}

fn layout(ws: &Workspace) -> LayoutNode {
    ws.project("session")
        .and_then(|p| p.layout.clone())
        .expect("session layout")
}

#[test]
fn a_split_in_a_session_opens_the_global_shell_not_the_agent() {
    let mut ws = running_session();
    let mut focus = FocusManager::new();
    let terminals: TerminalsRegistry = Arc::new(Default::default());
    let backend = RecordingBackend::default();

    let result = super::terminal::split(
        &mut ws,
        &mut focus,
        "session".into(),
        Vec::new(),
        SplitDirection::Horizontal,
        None,
        &backend,
        &terminals,
        &settings(),
        &mut TestCx,
    );
    if let super::ActionResult::Err(e) = result {
        panic!("{e}");
    }

    assert_eq!(backend.spawned(), vec![("t1".to_string(), global_shell())]);
    let layout = layout(&ws);
    assert_eq!(layout.agent_terminal_id().as_deref(), Some("a"));
    assert!(!layout.is_agent_terminal("t1"));
    let session = ws.project("session").expect("session");
    // `Default` here is the global setting, which project data cannot see.
    assert_eq!(session.terminal_shell("t1"), ShellType::Default);
    assert_eq!(session.terminal_shell("a"), agent_launch());
}

#[test]
fn a_new_tab_in_a_session_opens_the_global_shell_not_the_agent() {
    let mut ws = running_session();
    let mut focus = FocusManager::new();
    let terminals: TerminalsRegistry = Arc::new(Default::default());
    let backend = RecordingBackend::default();

    let result = super::tab::add_tab(
        &mut ws,
        &mut focus,
        "session".into(),
        Vec::new(),
        false,
        None,
        &backend,
        &terminals,
        &settings(),
        &mut TestCx,
    );
    if let super::ActionResult::Err(e) = result {
        panic!("{e}");
    }

    assert_eq!(backend.spawned(), vec![("t1".to_string(), global_shell())]);
    assert_eq!(layout(&ws).agent_terminal_id().as_deref(), Some("a"));
}

#[test]
fn an_ordinary_project_still_opens_its_own_shell() {
    let project_shell = ShellType::Custom {
        path: "/bin/project-shell".into(),
        args: Vec::new(),
    };
    let mut ws = workspace(project(
        false,
        Some(project_shell.clone()),
        pane(Some("a"), false),
    ));
    let mut focus = FocusManager::new();
    let terminals: TerminalsRegistry = Arc::new(Default::default());
    let backend = RecordingBackend::default();

    super::terminal::split(
        &mut ws,
        &mut focus,
        "session".into(),
        Vec::new(),
        SplitDirection::Vertical,
        None,
        &backend,
        &terminals,
        &settings(),
        &mut TestCx,
    );

    assert_eq!(backend.spawned(), vec![("t1".to_string(), project_shell)]);
}

#[test]
fn a_stopped_agent_is_not_started_by_opening_another_pane() {
    let mut ws = workspace(project(true, Some(agent_launch()), pane(None, true)));
    let mut focus = FocusManager::new();
    let terminals: TerminalsRegistry = Arc::new(Default::default());
    let backend = RecordingBackend::default();

    super::terminal::create(
        &mut ws,
        &mut focus,
        "session".into(),
        &backend,
        &terminals,
        &settings(),
        &mut TestCx,
    );
    // Only the new pane spawned, as a shell; the agent pane stays stopped.
    assert_eq!(backend.spawned(), vec![("t1".to_string(), global_shell())]);
    let layout = layout(&ws);
    assert!(layout.agent_terminal_path().is_some());
    assert_eq!(layout.agent_terminal_id(), None);

    // A later spawn pass still leaves it alone.
    spawn_uninitialized_terminals(
        &mut ws,
        "session",
        &backend,
        &terminals,
        &settings(),
        None,
        &mut TestCx,
    );
    assert_eq!(backend.spawned().len(), 1);
}

#[test]
fn a_new_session_runs_its_agent_in_a_pane_marked_as_the_agent() {
    let mut ws = workspace(project(
        true,
        Some(agent_launch()),
        LayoutNode::new_terminal(),
    ));
    let terminals: TerminalsRegistry = Arc::new(Default::default());
    let backend = RecordingBackend::default();

    let result = super::spawn_session_terminals(
        &mut ws,
        "session",
        &backend,
        &terminals,
        &settings(),
        &mut TestCx,
    );
    if let super::ActionResult::Err(e) = result {
        panic!("{e}");
    }

    assert_eq!(backend.spawned(), vec![("t1".to_string(), agent_launch())]);
    assert_eq!(layout(&ws).agent_terminal_id().as_deref(), Some("t1"));
}

#[test]
fn a_session_started_without_an_agent_has_no_agent_pane() {
    let mut ws = workspace(project(true, None, LayoutNode::new_terminal()));
    let terminals: TerminalsRegistry = Arc::new(Default::default());
    let backend = RecordingBackend::default();

    super::spawn_session_terminals(
        &mut ws,
        "session",
        &backend,
        &terminals,
        &settings(),
        &mut TestCx,
    );

    assert_eq!(backend.spawned(), vec![("t1".to_string(), global_shell())]);
    assert_eq!(layout(&ws).agent_terminal_path(), None);
}

/// A session whose agent runs in `a` beside a shell in `b`, with the shell's
/// tab showing and focused.
fn session_with_a_shell_in_front() -> (Workspace, FocusManager, TerminalsRegistry) {
    let ws = workspace(project(
        true,
        Some(agent_launch()),
        LayoutNode::Tabs {
            children: vec![pane(Some("a"), true), pane(Some("b"), false)],
            active_tab: 1,
        },
    ));
    let mut focus = FocusManager::new();
    focus.focus_terminal("session".into(), vec![1]);
    let terminals: TerminalsRegistry = Arc::new(Default::default());
    for id in ["a", "b"] {
        terminals.lock().insert(
            id.to_string(),
            Arc::new(okena_terminal::terminal::Terminal::new(
                id.to_string(),
                okena_terminal::terminal::TerminalSize::default(),
                Arc::new(StubTransport),
                "/work/tree".to_string(),
            )),
        );
    }
    (ws, focus, terminals)
}

#[test]
fn stop_ends_only_the_agent_and_keeps_its_pane() {
    let (mut ws, _focus, terminals) = session_with_a_shell_in_front();
    let backend = RecordingBackend::default();

    let result =
        super::terminal::stop_agent(&mut ws, "session".into(), &backend, &terminals, &mut TestCx);
    if let super::ActionResult::Err(e) = result {
        panic!("{e}");
    }

    assert_eq!(backend.killed(), vec!["a".to_string()]);
    let layout = layout(&ws);
    // Still there, still the agent, now stopped.
    assert_eq!(layout.agent_terminal_path(), Some(vec![0]));
    assert_eq!(layout.agent_terminal_id(), None);
    // The shell beside it is untouched.
    assert_eq!(layout.collect_terminal_ids(), vec!["b".to_string()]);
    assert!(terminals.lock().contains_key("b"));
    assert!(!terminals.lock().contains_key("a"));
}

#[test]
fn restart_resumes_the_agent_in_its_own_pane_whichever_pane_is_in_front() {
    let (mut ws, mut focus, terminals) = session_with_a_shell_in_front();
    let backend = RecordingBackend::default();

    let result = super::terminal::restart_agent(
        &mut ws,
        &mut focus,
        "session".into(),
        &backend,
        &terminals,
        &settings(),
        &mut TestCx,
    );
    if let super::ActionResult::Err(e) = result {
        panic!("{e}");
    }

    assert_eq!(backend.killed(), vec!["a".to_string()]);
    let spawned = backend.spawned();
    assert_eq!(spawned.len(), 1);
    let (id, shell) = &spawned[0];
    let ShellType::Custom { path, args } = shell else {
        panic!("resumed as {shell:?}");
    };
    assert_eq!(path, "/usr/local/bin/claude");
    assert_eq!(&args[..2], ["--resume", "abc"]);
    // Same pane, new terminal; the shell and its tab are as they were.
    let layout = layout(&ws);
    assert_eq!(layout.agent_terminal_path(), Some(vec![0]));
    assert_eq!(layout.agent_terminal_id().as_ref(), Some(id));
    assert!(layout.find_terminal_path("b") == Some(vec![1]));
    assert!(matches!(layout, LayoutNode::Tabs { active_tab: 1, .. }));
    assert!(terminals.lock().contains_key("b"));
}

#[test]
fn start_brings_a_stopped_agent_back_from_its_brief_in_the_same_pane() {
    let (mut ws, mut focus, terminals) = session_with_a_shell_in_front();
    let backend = RecordingBackend::default();
    super::terminal::stop_agent(&mut ws, "session".into(), &backend, &terminals, &mut TestCx);

    let result = super::terminal::start_agent(
        &mut ws,
        &mut focus,
        "session".into(),
        &backend,
        &terminals,
        &settings(),
        &mut TestCx,
    );
    if let super::ActionResult::Err(e) = result {
        panic!("{e}");
    }

    assert_eq!(backend.spawned(), vec![("t1".to_string(), agent_launch())]);
    let layout = layout(&ws);
    assert_eq!(layout.agent_terminal_path(), Some(vec![0]));
    assert_eq!(layout.agent_terminal_id().as_deref(), Some("t1"));
    assert_eq!(backend.killed(), vec!["a".to_string()]);
}

#[test]
fn an_agent_whose_pane_was_closed_gets_a_new_agent_pane() {
    let mut ws = workspace(project(true, Some(agent_launch()), pane(Some("b"), false)));
    let mut focus = FocusManager::new();
    let terminals: TerminalsRegistry = Arc::new(Default::default());
    let backend = RecordingBackend::default();

    super::terminal::start_agent(
        &mut ws,
        &mut focus,
        "session".into(),
        &backend,
        &terminals,
        &settings(),
        &mut TestCx,
    );

    assert_eq!(backend.spawned(), vec![("t1".to_string(), agent_launch())]);
    assert!(backend.killed().is_empty());
    let layout = layout(&ws);
    assert_eq!(layout.agent_terminal_id().as_deref(), Some("t1"));
    assert!(layout.find_terminal_path("b").is_some());
}

#[test]
fn an_instruction_goes_to_the_agent_not_the_shell_in_front() {
    let (mut ws, _focus, terminals) = session_with_a_shell_in_front();
    let backend = RecordingBackend::default();

    let result = super::tasks::send_instruction(
        &mut ws,
        "session".into(),
        "carry on".into(),
        &backend,
        &terminals,
        &settings(),
        &mut TestCx,
    );
    let super::ActionResult::Ok(Some(sent)) = result else {
        panic!("instruction not sent");
    };
    assert_eq!(sent["terminal_id"], "a");

    // A stopped agent is said to be stopped, not typed into the shell.
    super::terminal::stop_agent(&mut ws, "session".into(), &backend, &terminals, &mut TestCx);
    let result = super::tasks::send_instruction(
        &mut ws,
        "session".into(),
        "carry on".into(),
        &backend,
        &terminals,
        &settings(),
        &mut TestCx,
    );
    assert!(matches!(result, super::ActionResult::Err(_)));
}
