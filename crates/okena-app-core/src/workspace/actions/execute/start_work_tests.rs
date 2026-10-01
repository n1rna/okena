//! Starts an agent makes (`okena_start_work`): a coordinator's sub-agents.
//!
//! Run against real repositories in a temp directory, since what is being
//! checked is what ends up on disk and in the workspace: which worktrees were
//! cut where, on which branch, and what a start that cannot finish leaves.
//! The provider hands back tasks without a network, and the backend records
//! what each spawn ran instead of running it.

use super::{StartWork, start_work_with};
use crate::workspace::actions::execute::ActionResult;
use crate::workspace::focus::FocusManager;
use crate::workspace::persistence::AppSettings;
use crate::workspace::state::{ProjectData, WindowId, WindowState, Workspace, WorkspaceData};
use okena_core::tasks::{Task, TaskId, TaskState};
use okena_tasks::provider::{AuthStatus, TaskError, TaskProvider};
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::{TerminalBackend, TerminalLaunchPlan};
use okena_terminal::shell_config::ShellType;
use okena_terminal::terminal::TerminalTransport;
use okena_workspace::context::WorkspaceCx;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

// ── A provider that knows three tasks ───────────────────────────────────────

struct Tasks(Vec<Task>);

fn task(n: u32, title: &str) -> Task {
    Task {
        id: TaskId::new("linear", format!("uuid-{n}")),
        display_key: format!("QBL-{n}"),
        title: title.into(),
        description: Some(format!("What QBL-{n} asks for.")),
        state: TaskState::Todo,
        state_name: "Todo".into(),
        url: format!("https://linear.app/x/issue/QBL-{n}"),
        branch_name: String::new(),
        updated_at: "2026-10-01T00:00:00Z".into(),
        kind: okena_core::tasks::TaskKind::Task,
        parent_id: None,
        parent_key: None,
        labels: Vec::new(),
        groups: Vec::new(),
    }
}

fn provider() -> Tasks {
    Tasks(vec![
        task(1, "First thing"),
        task(2, "Second thing"),
        task(3, "Third thing"),
    ])
}

impl TaskProvider for Tasks {
    fn id(&self) -> &'static str {
        "linear"
    }
    fn display_name(&self) -> &'static str {
        "Linear"
    }
    fn auth_status(&self) -> AuthStatus {
        AuthStatus::Disconnected
    }
    fn list_assigned(&self) -> Result<Vec<Task>, TaskError> {
        Ok(self.0.clone())
    }
    fn set_state(&self, _id: &TaskId, _state: TaskState) -> Result<(), TaskError> {
        Ok(())
    }
    fn get_task(&self, id: &TaskId) -> Result<Task, TaskError> {
        self.0
            .iter()
            .find(|t| t.id.external_id == id.external_id || t.display_key == id.external_id)
            .cloned()
            .ok_or_else(|| TaskError::Protocol {
                provider: "linear",
                message: format!("no task `{}`", id.external_id),
            })
    }
}

// ── A backend that records each spawn, and can refuse the agent's ───────────

struct StubTransport;

impl TerminalTransport for StubTransport {
    fn send_input(&self, _terminal_id: &str, _data: &[u8]) {}
    fn resize(&self, _terminal_id: &str, _cols: u16, _rows: u16) {}
    fn uses_mouse_backend(&self) -> bool {
        false
    }
}

#[derive(Default)]
struct Backend {
    next: Mutex<usize>,
    /// Every spawn: where, and what it ran.
    spawned: Mutex<Vec<(String, ShellType)>>,
    /// Refuse to spawn anything that is not a plain shell.
    refuse_agents: bool,
}

impl Backend {
    /// The agents started: the directory each runs in, and its arguments.
    fn agents(&self) -> Vec<(String, Vec<String>)> {
        self.spawned
            .lock()
            .expect("spawned lock")
            .iter()
            .filter_map(|(cwd, shell)| match shell {
                ShellType::Custom { args, .. } => Some((cwd.clone(), args.clone())),
                _ => None,
            })
            .collect()
    }
}

impl TerminalBackend for Backend {
    fn transport(&self) -> Arc<dyn TerminalTransport> {
        Arc::new(StubTransport)
    }
    fn create_terminal(&self, _cwd: &str, _shell: Option<&ShellType>) -> anyhow::Result<String> {
        unreachable!("spawns go through launch plans")
    }
    fn create_terminal_with_plan(
        &self,
        cwd: &str,
        plan: &TerminalLaunchPlan,
    ) -> anyhow::Result<String> {
        if self.refuse_agents && matches!(plan.route, ShellType::Custom { .. }) {
            anyhow::bail!("no pty to give it");
        }
        let mut next = self.next.lock().expect("id lock");
        *next += 1;
        self.spawned
            .lock()
            .expect("spawned lock")
            .push((cwd.to_string(), plan.route.clone()));
        Ok(format!("t{next}"))
    }
    fn reconnect_terminal(
        &self,
        terminal_id: &str,
        _cwd: &str,
        _shell: Option<&ShellType>,
    ) -> anyhow::Result<String> {
        Ok(terminal_id.to_string())
    }
    fn kill(&self, _terminal_id: &str) {}
    fn capture_buffer(&self, _terminal_id: &str) -> Option<PathBuf> {
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

// ── The fixture: a projects root with repositories, and a coordinator ───────

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn project(id: &str, path: &Path) -> ProjectData {
    serde_json::from_value(serde_json::json!({
        "id": id, "name": id, "path": path.to_string_lossy(),
    }))
    .expect("a project")
}

struct Fixture {
    /// Held so the directory outlives the test body.
    _dir: tempfile::TempDir,
    root: PathBuf,
    ws: Workspace,
    focus: FocusManager,
    backend: Backend,
    terminals: TerminalsRegistry,
    settings: AppSettings,
}

/// The id of the coordinator every fixture has: a session over tasks picked
/// together, with no worktree, started in a directory that is no repository —
/// the projects root itself, as `plain`.
const COORDINATOR: &str = "coord";

impl Fixture {
    /// Repositories `repos` under one projects root, the root itself as the
    /// project `plain`, and a coordinator started there.
    fn new(repos: &[&str]) -> Self {
        let dir = tempfile::tempdir().expect("a temp dir");
        // Canonical, as git reports paths: macOS's temp dir is a symlink.
        let root = dir.path().canonicalize().expect("canonical root").join("p");
        std::fs::create_dir_all(&root).expect("create root");

        let mut projects = vec![project("plain", &root)];
        for repo in repos {
            let path = root.join(repo);
            std::fs::create_dir_all(&path).expect("create repo dir");
            git(&path, &["init", "--initial-branch=main"]);
            git(&path, &["config", "user.email", "test@example.com"]);
            git(&path, &["config", "user.name", "Test"]);
            std::fs::write(path.join("README.md"), repo).expect("write file");
            git(&path, &["add", "README.md"]);
            git(&path, &["commit", "-m", "init"]);
            projects.push(project(repo, &path));
        }

        let mut coordinator = project(COORDINATOR, &root);
        let picked: Vec<okena_core::tasks::TaskRef> = provider().0.iter().map(Into::into).collect();
        coordinator.task_ref = picked.first().cloned();
        coordinator.also_tasks = picked[1..].to_vec();
        coordinator.repo_ids = vec!["plain".into()];
        coordinator.agent_purpose = Some(okena_core::harness::AgentPurpose::Work);
        projects.push(coordinator);

        let project_order = projects.iter().map(|p| p.id.clone()).collect();
        let ws = Workspace::new(WorkspaceData {
            version: 1,
            projects,
            project_order,
            folders: Vec::new(),
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            main_window: WindowState::default(),
            extra_windows: Vec::new(),
        });

        let mut settings = AppSettings::default();
        settings.harness.agent_command = Some("claude".into());
        settings.harness.agent_root = Some(root.to_string_lossy().into_owned());

        // Briefs go to files, so a test can read what an agent was told.
        super::super::briefs::test_briefs_dir::set(Some(dir.path().join("briefs")));

        Self {
            _dir: dir,
            root,
            ws,
            focus: FocusManager::new(),
            backend: Backend::default(),
            terminals: Arc::new(Default::default()),
            settings,
        }
    }

    /// What `okena_start_work` sends for the coordinator: `tasks` in `projects`.
    fn start(&mut self, tasks: &[&str], projects: &[&str]) -> ActionResult {
        self.start_as(tasks, projects, None)
    }

    fn start_as(&mut self, tasks: &[&str], projects: &[&str], agent: Option<&str>) -> ActionResult {
        let req = StartWork {
            provider: "linear".into(),
            task_external_id: tasks[0].into(),
            project_ids: projects.iter().map(|p| p.to_string()).collect(),
            agent_root: None,
            branch: None,
            agent_command: agent.map(str::to_string),
            model: None,
            note: None,
            coordinate: false,
            also: tasks[1..].iter().map(|t| t.to_string()).collect(),
            siblings: Vec::new(),
            branches: Default::default(),
            hand_picked: false,
            context: Vec::new(),
            started_by: Some(COORDINATOR.into()),
        };
        start_work_with(
            &mut self.ws,
            WindowId::Main,
            &mut self.focus,
            req,
            &provider(),
            &self.backend,
            &self.terminals,
            &self.settings,
            &mut TestCx,
        )
    }

    /// Every worktree okena has, as `repo: branch`, sorted.
    fn worktrees(&self) -> Vec<String> {
        let mut found: Vec<String> = self
            .ws
            .data
            .projects
            .iter()
            .filter_map(|p| {
                let w = p.worktree_info.as_ref()?;
                Some(format!("{}: {}", w.parent_project_id, w.branch_name))
            })
            .collect();
        found.sort();
        found
    }

    /// The sessions the coordinator started, by name.
    fn sub_agents(&self) -> Vec<&ProjectData> {
        self.ws
            .data
            .projects
            .iter()
            .filter(|p| p.started_by.as_deref() == Some(COORDINATOR))
            .collect()
    }

    /// The checkouts git itself has for `repo`, beside the repository's own.
    fn git_worktrees(&self, repo: &str) -> usize {
        git(&self.root.join(repo), &["worktree", "list", "--porcelain"])
            .lines()
            .filter(|l| l.starts_with("worktree "))
            .count()
            - 1
    }

    fn branches(&self, repo: &str) -> Vec<String> {
        git(
            &self.root.join(repo),
            &["branch", "--format=%(refname:short)"],
        )
        .lines()
        .map(str::to_string)
        .collect()
    }

    /// The brief the agent running in `cwd` was handed.
    fn brief_in(&self, cwd: &str) -> String {
        let (_, args) = self
            .backend
            .agents()
            .into_iter()
            .find(|(at, _)| at == cwd)
            .unwrap_or_else(|| panic!("no agent runs in {cwd}"));
        let file = args
            .iter()
            .find_map(|a| okena_terminal::brief_file::path_of(a))
            .expect("a brief file");
        std::fs::read_to_string(file).expect("read the brief")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        super::super::briefs::test_briefs_dir::set(None);
    }
}

fn ok(result: ActionResult) -> serde_json::Value {
    match result {
        ActionResult::Ok(Some(value)) => value,
        ActionResult::Ok(None) => panic!("the start returned nothing"),
        ActionResult::Err(e) => panic!("the start failed: {e}"),
    }
}

fn err(result: ActionResult) -> String {
    match result {
        ActionResult::Err(e) => e,
        ActionResult::Ok(v) => panic!("expected the start to fail, got {v:?}"),
    }
}

const BRANCH_1: &str = "chore/qbl-1-first-thing";
const BRANCH_2: &str = "chore/qbl-2-second-thing";
const BRANCH_3: &str = "chore/qbl-3-third-thing";

// ── The three shapes a coordinator starts ───────────────────────────────────

#[test]
fn three_tickets_in_three_projects_each_get_a_worktree_of_their_own_project() {
    let mut f = Fixture::new(&["alpha", "beta", "gamma"]);
    ok(f.start(&["QBL-1"], &["alpha"]));
    ok(f.start(&["QBL-2"], &["beta"]));
    ok(f.start(&["QBL-3"], &["gamma"]));

    // One worktree per ticket, in that ticket's project and no other.
    assert_eq!(
        f.worktrees(),
        [
            format!("alpha: {BRANCH_1}"),
            format!("beta: {BRANCH_2}"),
            format!("gamma: {BRANCH_3}"),
        ]
    );
    for repo in ["alpha", "beta", "gamma"] {
        assert_eq!(f.git_worktrees(repo), 1, "{repo}");
    }

    // Three sessions, each the coordinator's, each in its own worktree —
    // not in the plain directory the coordinator runs in.
    let subs = f.sub_agents();
    let names: Vec<&str> = subs.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["QBL-1", "QBL-2", "QBL-3"]);
    let wt = |repo: &str, branch: &str| {
        f.root
            .join(format!("{repo}-wt"))
            .join(branch.replace('/', "-"))
            .to_string_lossy()
            .into_owned()
    };
    let expected = [
        wt("alpha", BRANCH_1),
        wt("beta", BRANCH_2),
        wt("gamma", BRANCH_3),
    ];
    let roots: Vec<&str> = subs.iter().map(|p| p.path.as_str()).collect();
    assert_eq!(roots, expected);

    // And an agent was started in each, holding its own task.
    let agents = f.backend.agents();
    assert_eq!(agents.len(), 3);
    for (path, key) in expected.iter().zip(["QBL-1", "QBL-2", "QBL-3"]) {
        let brief = f.brief_in(path);
        assert!(brief.starts_with(&format!("Work on {key}:")), "{brief}");
        assert!(brief.contains(path.as_str()), "{brief}");
    }
}

#[test]
fn three_tickets_in_one_project_get_three_worktrees_on_three_branches() {
    let mut f = Fixture::new(&["alpha"]);
    for key in ["QBL-1", "QBL-2", "QBL-3"] {
        ok(f.start(&[key], &["alpha"]));
    }
    assert_eq!(
        f.worktrees(),
        [
            format!("alpha: {BRANCH_1}"),
            format!("alpha: {BRANCH_2}"),
            format!("alpha: {BRANCH_3}"),
        ]
    );
    assert_eq!(f.git_worktrees("alpha"), 3);

    // Three agents, in three different directories.
    let mut cwds: Vec<String> = f.backend.agents().into_iter().map(|(cwd, _)| cwd).collect();
    cwds.sort();
    cwds.dedup();
    assert_eq!(cwds.len(), 3);
    assert_eq!(f.sub_agents().len(), 3);
}

#[test]
fn one_ticket_in_two_projects_gets_a_worktree_in_each_and_is_told_which_is_which() {
    let mut f = Fixture::new(&["alpha", "beta", "gamma"]);
    let started = ok(f.start(&["QBL-1"], &["alpha", "beta"]));

    assert_eq!(
        f.worktrees(),
        [format!("alpha: {BRANCH_1}"), format!("beta: {BRANCH_1}")]
    );
    // Nothing in the project it was not given.
    assert_eq!(f.git_worktrees("gamma"), 0);

    // One session, above both worktrees so it reaches each.
    let subs = f.sub_agents();
    assert_eq!(subs.len(), 1);
    let root = f.root.to_string_lossy().into_owned();
    assert_eq!(subs[0].path, root);
    assert_eq!(started["agent_session"]["root"], root.as_str());

    // Both worktrees carry the task, so its changes are listed per project.
    let on_task: Vec<&str> =
        f.ws.data
            .projects
            .iter()
            .filter(|p| p.worktree_info.is_some() && p.works_on("uuid-1"))
            .map(|p| {
                p.worktree_info
                    .as_ref()
                    .expect("a worktree")
                    .parent_project_id
                    .as_str()
            })
            .collect();
    assert_eq!(on_task, ["alpha", "beta"]);

    let slug = BRANCH_1.replace('/', "-");
    let brief = f.brief_in(&root);
    assert!(
        brief.contains(&format!(
            "Worktrees you were given:\n- alpha ({root}/alpha-wt/{slug})\n- beta ({root}/beta-wt/{slug})"
        )),
        "{brief}"
    );
}

#[test]
fn a_project_named_twice_is_cut_one_worktree() {
    let mut f = Fixture::new(&["alpha"]);
    ok(f.start(&["QBL-1"], &["alpha", "alpha"]));
    assert_eq!(f.git_worktrees("alpha"), 1);
}

// ── A ticket started again: nothing is cut twice ────────────────────────────

#[test]
fn a_ticket_an_agent_is_already_on_is_not_started_again() {
    let mut f = Fixture::new(&["alpha"]);
    ok(f.start(&["QBL-1"], &["alpha"]));

    let refused = err(f.start(&["QBL-1"], &["alpha"]));
    assert!(
        refused.contains("QBL-1 already has an agent working on it: session `QBL-1`"),
        "{refused}"
    );
    // Still one worktree, one session, one agent.
    assert_eq!(f.git_worktrees("alpha"), 1);
    assert_eq!(f.sub_agents().len(), 1);
    assert_eq!(f.backend.agents().len(), 1);
}

#[test]
fn the_coordinator_being_linked_to_its_tickets_does_not_count_as_an_agent_on_them() {
    // It is linked to all three, and works on none.
    let mut f = Fixture::new(&["alpha"]);
    ok(f.start(&["QBL-2"], &["alpha"]));
    assert_eq!(f.sub_agents().len(), 1);
}

#[test]
fn a_worktree_left_by_an_earlier_start_is_worked_in_again_not_cut_twice() {
    let mut f = Fixture::new(&["alpha"]);
    ok(f.start(&["QBL-1"], &["alpha"]));
    // Its agent's session was closed; the worktree stays.
    let first = f.sub_agents()[0].id.clone();
    let path = f.sub_agents()[0].path.clone();
    if let Some(p) = f.ws.data.projects.iter_mut().find(|p| p.id == first) {
        p.closed_at = Some(1);
    }

    let again = ok(f.start(&["QBL-1"], &["alpha"]));
    assert_eq!(again["created"], serde_json::json!([]));
    assert_eq!(again["reused"][0]["path"], path.as_str());
    assert_eq!(again["agent_session"]["root"], path.as_str());
    assert_eq!(f.git_worktrees("alpha"), 1);
    assert_eq!(f.worktrees(), [format!("alpha: {BRANCH_1}")]);
}

#[test]
fn a_branch_kept_from_a_removed_worktree_is_checked_out_again() {
    // Removing a task's worktree keeps its branch. `git worktree add -b` on
    // that name is refused, which read as "this task cannot be started".
    let mut f = Fixture::new(&["alpha"]);
    git(&f.root.join("alpha"), &["branch", BRANCH_1]);
    ok(f.start(&["QBL-1"], &["alpha"]));
    assert_eq!(f.worktrees(), [format!("alpha: {BRANCH_1}")]);
}

// ── A start that cannot finish leaves nothing ───────────────────────────────

/// Nothing the coordinator started exists: no session, no worktree, no branch.
fn assert_nothing_left(f: &Fixture, repos: &[&str]) {
    assert!(f.sub_agents().is_empty(), "a session was left behind");
    assert!(f.worktrees().is_empty(), "{:?}", f.worktrees());
    assert!(f.backend.agents().is_empty(), "an agent was started");
    for repo in repos {
        assert_eq!(f.git_worktrees(repo), 0, "{repo}");
        assert_eq!(f.branches(repo), ["main"], "{repo}");
    }
    // Only what the fixture began with.
    assert_eq!(f.ws.data.projects.len(), repos.len() + 2);
}

#[test]
fn a_project_okena_does_not_have_fails_the_start() {
    let mut f = Fixture::new(&["alpha"]);
    let refused = err(f.start(&["QBL-1"], &["alpha", "nope"]));
    assert_eq!(refused, "project not found: nope");
    assert_nothing_left(&f, &["alpha"]);
}

#[test]
fn a_task_that_cannot_be_read_fails_the_start_and_is_named() {
    let mut f = Fixture::new(&["alpha"]);
    let refused = err(f.start(&["QBL-1", "QBL-9"], &["alpha"]));
    assert!(
        refused.starts_with("QBL-9 was not started: it could not be read"),
        "{refused}"
    );
    assert_nothing_left(&f, &["alpha"]);
}

#[test]
fn a_directory_that_is_no_repository_fails_the_start_instead_of_starting_without_a_worktree() {
    // What happened before: the project was skipped, and the agent started at
    // the projects root with no worktree, beside every other sub-agent.
    let mut f = Fixture::new(&["alpha"]);
    let refused = err(f.start(&["QBL-1"], &["plain"]));
    assert!(
        refused.starts_with("QBL-1 was not started: `plain`"),
        "{refused}"
    );
    assert!(refused.contains("is not a git repository"), "{refused}");
    assert!(refused.contains("as `projects`"), "{refused}");
    assert_nothing_left(&f, &["alpha"]);
}

#[test]
fn a_worktree_is_not_a_project_to_start_in() {
    let mut f = Fixture::new(&["alpha"]);
    ok(f.start(&["QBL-1"], &["alpha"]));
    let worktree =
        f.ws.data
            .projects
            .iter()
            .find(|p| p.worktree_info.is_some())
            .map(|p| p.id.clone())
            .expect("a worktree");
    let refused = err(f.start(&["QBL-2"], &[worktree.as_str()]));
    assert!(
        refused.contains("is a worktree or an agent session"),
        "{refused}"
    );
    assert_eq!(f.sub_agents().len(), 1);
}

#[test]
fn an_agent_okena_cannot_start_fails_the_start() {
    // What happened before: the name was run as a command, the terminal
    // exited at once, and an empty session was reported as started.
    let mut f = Fixture::new(&["alpha"]);
    let refused = err(f.start_as(&["QBL-1"], &["alpha"], Some("general-purpose")));
    assert_eq!(
        refused,
        "QBL-1 was not started: `general-purpose` is not an agent okena can start. Leave \
         `agent` out to run the configured one (`claude`), or pass one of: claude, copilot, codex"
    );
    assert_nothing_left(&f, &["alpha"]);
}

#[test]
fn no_agent_to_start_fails_the_start() {
    let mut f = Fixture::new(&["alpha"]);
    f.settings.harness.agent_command = None;
    let refused = err(f.start(&["QBL-1"], &["alpha"]));
    assert!(
        refused.contains("no agent is configured to start"),
        "{refused}"
    );
    assert_nothing_left(&f, &["alpha"]);
}

#[test]
fn the_configured_agent_runs_as_configured_when_asked_for_by_name() {
    let mut f = Fixture::new(&["alpha"]);
    f.settings.harness.agent_command = Some("/opt/bin/claude".into());
    ok(f.start_as(&["QBL-1"], &["alpha"], Some("claude")));
    let ran = f.backend.spawned.lock().expect("spawned lock").clone();
    assert!(
        ran.iter()
            .any(|(_, shell)| matches!(shell, ShellType::Custom { path, .. } if path == "/opt/bin/claude")),
        "{ran:?}"
    );
}

#[test]
fn a_worktree_that_cannot_be_cut_takes_back_the_ones_that_were() {
    // beta's target directory is taken by something okena does not know.
    let mut f = Fixture::new(&["alpha", "beta"]);
    let slug = BRANCH_1.replace('/', "-");
    let squatter = f.root.join("beta-wt").join(&slug);
    std::fs::create_dir_all(&squatter).expect("create squatter");
    std::fs::write(squatter.join("file"), "x").expect("write squatter");

    let refused = err(f.start(&["QBL-1"], &["alpha", "beta"]));
    assert!(
        refused.starts_with("QBL-1 was not started: beta:"),
        "{refused}"
    );
    assert!(
        refused.ends_with("What the start had created was removed."),
        "{refused}"
    );
    // alpha's worktree was cut first, and is gone again with its branch.
    assert_nothing_left(&f, &["alpha", "beta"]);
    assert!(!f.root.join("alpha-wt").join(&slug).exists());
}

#[cfg(unix)]
#[test]
fn a_worktree_git_could_not_check_out_does_not_leave_its_branch() {
    // `git worktree add -b` makes the branch first. Where the checkout then
    // fails, the branch stayed — and the task could not be started again.
    use std::os::unix::fs::PermissionsExt;
    let mut f = Fixture::new(&["alpha", "beta"]);
    let parent = f.root.join("beta-wt");
    std::fs::create_dir_all(&parent).expect("create beta-wt");
    let lock = |mode| {
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(mode))
            .expect("set permissions")
    };
    lock(0o555);
    let refused = err(f.start(&["QBL-1"], &["alpha", "beta"]));
    lock(0o755);

    assert!(
        refused.starts_with("QBL-1 was not started: beta:"),
        "{refused}"
    );
    assert_nothing_left(&f, &["alpha", "beta"]);

    // With the directory writable again the same start goes through.
    ok(f.start(&["QBL-1"], &["alpha", "beta"]));
    assert_eq!(f.sub_agents().len(), 1);
}

#[test]
fn an_agent_that_cannot_be_spawned_takes_back_its_session_and_worktrees() {
    let mut f = Fixture::new(&["alpha", "beta"]);
    f.backend.refuse_agents = true;
    let refused = err(f.start(&["QBL-1"], &["alpha", "beta"]));
    assert!(
        refused.starts_with("QBL-1 was not started: the agent could not be started:"),
        "{refused}"
    );
    assert_nothing_left(&f, &["alpha", "beta"]);
}

#[test]
fn a_start_that_fails_keeps_the_worktree_it_found() {
    // Found from an earlier start, not cut by this one: not this one's to remove.
    let mut f = Fixture::new(&["alpha"]);
    ok(f.start(&["QBL-1"], &["alpha"]));
    let first = f.sub_agents()[0].id.clone();
    if let Some(p) = f.ws.data.projects.iter_mut().find(|p| p.id == first) {
        p.closed_at = Some(1);
    }
    f.backend.refuse_agents = true;
    err(f.start(&["QBL-1"], &["alpha"]));
    assert_eq!(f.git_worktrees("alpha"), 1);
    assert_eq!(f.branches("alpha"), [BRANCH_1, "main"]);
}

#[test]
fn a_session_okena_does_not_have_cannot_start_agents() {
    let mut f = Fixture::new(&["alpha"]);
    f.ws.data.projects.retain(|p| p.id != COORDINATOR);
    let refused = err(f.start(&["QBL-1"], &["alpha"]));
    assert!(refused.contains("is not one okena has"), "{refused}");
    assert!(f.worktrees().is_empty());
}

// ── A start a person makes is as it was ─────────────────────────────────────

#[test]
fn a_start_a_person_makes_still_reports_a_plain_directory_and_goes_ahead() {
    let mut f = Fixture::new(&["alpha"]);
    let req = StartWork {
        provider: "linear".into(),
        task_external_id: "QBL-1".into(),
        project_ids: vec!["plain".into()],
        agent_root: None,
        branch: None,
        agent_command: None,
        model: None,
        note: None,
        coordinate: false,
        also: Vec::new(),
        siblings: Vec::new(),
        branches: Default::default(),
        hand_picked: false,
        context: Vec::new(),
        started_by: None,
    };
    let started = ok(start_work_with(
        &mut f.ws,
        WindowId::Main,
        &mut f.focus,
        req,
        &provider(),
        &f.backend,
        &f.terminals,
        &f.settings,
        &mut TestCx,
    ));
    assert_eq!(started["skipped"][0]["reason"], "not a git repository");
    assert!(started["agent_session"].is_object());
    // Nobody's sub-agent.
    assert!(f.sub_agents().is_empty());
}
