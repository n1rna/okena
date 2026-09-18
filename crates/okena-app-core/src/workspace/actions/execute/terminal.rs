//! Terminal action handlers — create / split / close / focus and direct PTY I/O.

// Handlers take the workspace, focus manager, terminals registry and cx as
// distinct dependencies; bundling them into a context struct would obscure
// more than it clarifies here.
#![allow(clippy::too_many_arguments)]

use super::{ActionResult, ensure_terminal, find_terminal_path, spawn_uninitialized_terminals};
use crate::workspace::focus::FocusManager;
use crate::workspace::persistence::AppSettings;
use crate::workspace::state::Workspace;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::Term;
use okena_core::keys::SpecialKey;
use okena_core::types::SplitDirection;
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::TerminalBackend;
use okena_terminal::shell_config::ShellType;
use okena_terminal::terminal::Terminal;
use okena_terminal::terminal::TerminalSize;
use okena_workspace::context::WorkspaceCx;
use okena_workspace::state::LayoutNode;

fn with_ensured_terminal(
    ws: &Workspace,
    terminal_id: &str,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    f: impl FnOnce(&Terminal) -> ActionResult,
) -> ActionResult {
    match ensure_terminal(terminal_id, terminals, backend, ws, settings) {
        Some(term) => f(&term),
        None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
    }
}

pub(super) fn create(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    // Open in the focused terminal's cwd (when one is focused in this project),
    // else the project path.
    let inherit_cwd = focus_manager
        .focused_terminal_state()
        .filter(|f| f.project_id == project_id)
        .and_then(|f| super::inherited_cwd(ws, terminals, &project_id, &f.layout_path));
    ws.add_terminal(focus_manager, &project_id, cx);
    spawn_uninitialized_terminals(
        ws,
        &project_id,
        backend,
        terminals,
        settings,
        inherit_cwd,
        cx,
    )
}

pub(super) fn split(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    path: Vec<usize>,
    direction: SplitDirection,
    shell_type: Option<okena_terminal::shell_config::ShellType>,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    // Inherit the split source terminal's live cwd (captured before the layout
    // mutation invalidates `path`) so the new pane opens in the same directory.
    let inherit_cwd = super::inherited_cwd(ws, terminals, &project_id, &path);
    ws.split_terminal(focus_manager, &project_id, &path, direction, cx);
    super::apply_requested_shell(ws, &project_id, shell_type);
    spawn_uninitialized_terminals(
        ws,
        &project_id,
        backend,
        terminals,
        settings,
        inherit_cwd,
        cx,
    )
}

/// Switch a terminal's shell: kill the old PTY, reset the layout node to
/// uninitialized with the requested shell, then respawn it. Reuses
/// `spawn_uninitialized_terminals` so the new PTY goes through the same
/// shell-default resolution + shell-wrapper/on_create hook application as any
/// freshly created terminal — keeping daemon shell-switch behavior identical to
/// the old in-process GUI path.
pub(super) fn switch_shell(
    ws: &mut Workspace,
    project_id: String,
    terminal_id: String,
    shell: ShellType,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let path = match find_terminal_path(ws, &project_id, &terminal_id) {
        Some(p) => p,
        None => return ActionResult::Err(format!("terminal not found: {}", terminal_id)),
    };
    // No-op if the shell is unchanged (mirrors the old GUI guard).
    if ws.get_terminal_shell(&project_id, &path).as_ref() == Some(&shell) {
        return ActionResult::Ok(None);
    }
    if terminals.lock().contains_key(&terminal_id) {
        ws.remember_closing_terminal_owner(&project_id, &terminal_id);
    }
    backend.kill(&terminal_id);
    terminals.lock().remove(&terminal_id);
    ws.set_terminal_shell(&project_id, &path, shell, cx);
    ws.clear_terminal_id(&project_id, &path, cx);
    // Shell-switch respawns the pane in place; keep the project path (the old
    // terminal's cwd is gone with its PTY).
    spawn_uninitialized_terminals(ws, &project_id, backend, terminals, settings, None, cx)
}

/// Restart a session's agent, resuming its conversation.
///
/// The agent's own pane is torn down — its tmux session with it, so the old
/// process cannot keep running beside the resumed one — and respawned in
/// place as the agent's resume command. A stopped agent is resumed in its
/// pane. The session's other terminals, and whichever pane has focus, are
/// not touched.
///
/// The session's own `default_shell` is left as the original launch: it still
/// carries the conversation id every later restart resumes by, and "Start
/// agent" still means a fresh start from the brief.
pub(super) fn restart_agent(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let Some(project) = ws.project(&project_id) else {
        return ActionResult::Err(format!("project not found: {project_id}"));
    };
    let Some(launch) = project.default_shell.clone() else {
        return ActionResult::Err("this session was not started with an agent".into());
    };
    let shares_dir = ws.projects().iter().any(|p| {
        p.id != project.id
            && p.path == project.path
            && p.worktree_info.is_none()
            && p.is_any_agent_session()
    });
    if !super::agent_resume::resumable(&launch, shares_dir) {
        return ActionResult::Err(if shares_dir {
            "this session shares its directory with another agent and was started before \
             okena named conversations, so resuming could pick up the wrong one — start it \
             again instead"
                .into()
        } else {
            "okena does not know how to resume this agent".into()
        });
    }
    let Some(resume) = super::agent_resume::resume_shell(&launch, settings) else {
        return ActionResult::Err("okena does not know how to resume this agent".into());
    };
    respawn_agent(
        ws,
        focus_manager,
        &project_id,
        resume,
        backend,
        terminals,
        settings,
        cx,
    )
}

/// Start a session's agent again from its brief, in its own pane.
///
/// A running agent is replaced: a new terminal id means a new tmux session,
/// where reusing the old one would reattach to the old process. The pane
/// inherits the session's launch — the agent command, brief and MCP config
/// chosen when the session was started.
pub(super) fn start_agent(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    match ws.project(&project_id) {
        None => return ActionResult::Err(format!("project not found: {project_id}")),
        Some(project) if project.default_shell.is_none() => {
            return ActionResult::Err("this session was not started with an agent".into());
        }
        Some(_) => {}
    }
    respawn_agent(
        ws,
        focus_manager,
        &project_id,
        ShellType::Default,
        backend,
        terminals,
        settings,
        cx,
    )
}

/// Stop a session's agent, leaving its pane in place as a stopped agent.
///
/// Only the agent's process ends — its tmux session with it. The session's
/// other terminals keep running, and Start or Resume bring the agent back in
/// the same pane.
pub(super) fn stop_agent(
    ws: &mut Workspace,
    project_id: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let Some(path) = agent_pane(ws, &project_id) else {
        return ActionResult::Err("this session has no agent terminal".into());
    };
    stop_agent_process(ws, &project_id, &path, backend, terminals, cx);
    ActionResult::Ok(None)
}

/// The path to a session's agent pane.
fn agent_pane(ws: &Workspace, project_id: &str) -> Option<Vec<usize>> {
    ws.project(project_id)?
        .layout
        .as_ref()?
        .agent_terminal_path()
}

/// End the agent running in the pane at `path`, if one is.
fn stop_agent_process(
    ws: &mut Workspace,
    project_id: &str,
    path: &[usize],
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut impl WorkspaceCx,
) {
    let running = ws
        .project(project_id)
        .and_then(|p| p.layout.as_ref())
        .and_then(|l| l.get_at_path(path))
        .and_then(|node| match node {
            LayoutNode::Terminal { terminal_id, .. } => terminal_id.clone(),
            _ => None,
        });
    let Some(terminal_id) = running else {
        return;
    };
    if terminals.lock().contains_key(&terminal_id) {
        ws.remember_closing_terminal_owner(project_id, &terminal_id);
    }
    backend.kill(&terminal_id);
    terminals.lock().remove(&terminal_id);
    ws.clear_terminal_id(project_id, path, cx);
}

/// Run `shell` as the session's agent, in the agent's own pane.
///
/// Whatever the pane runs now is ended first. A session whose agent pane was
/// closed gets a new one.
#[allow(clippy::too_many_arguments)]
fn respawn_agent(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: &str,
    shell: ShellType,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let path = match agent_pane(ws, project_id) {
        Some(path) => {
            stop_agent_process(ws, project_id, &path, backend, terminals, cx);
            path
        }
        None => {
            ws.add_terminal(focus_manager, project_id, cx);
            let Some(layout) = ws
                .data
                .projects
                .iter_mut()
                .find(|p| p.id == project_id)
                .and_then(|p| p.layout.as_mut())
            else {
                return ActionResult::Err("could not add a terminal for the agent".into());
            };
            let Some(path) = layout.find_uninitialized_terminal_path() else {
                return ActionResult::Err("could not add a terminal for the agent".into());
            };
            if let Some(LayoutNode::Terminal { agent, .. }) = layout.get_at_path_mut(&path) {
                *agent = true;
            }
            path
        }
    };
    ws.set_terminal_shell(project_id, &path, shell, cx);
    super::spawn_agent_terminal(ws, project_id, backend, terminals, settings, cx)
}

/// Close an agent session: stop its agent and move it to the Agents history.
///
/// Every terminal in the session goes, as with Stop — their tmux sessions and
/// the agent processes with them — and the session is stamped closed. That is
/// all: its record, worktrees and branches are left exactly as they are, so a
/// closed session still shows everything it had and can be reopened.
pub(super) fn close_agent(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let Some(project) = ws.project(&project_id) else {
        return ActionResult::Err(format!("project not found: {project_id}"));
    };
    if !project.is_any_agent_session() {
        return ActionResult::Err("only an agent session can be closed".into());
    }
    if let Some(terminal_ids) = project.layout.as_ref().map(|l| l.collect_terminal_ids()) {
        close_agent_terminals(
            ws,
            focus_manager,
            &project_id,
            &terminal_ids,
            backend,
            terminals,
            cx,
        );
    }
    let now = super::tasks::now_millis();
    ws.with_project(&project_id, cx, |p| {
        p.closed_at = Some(now);
        true
    });
    ActionResult::Ok(None)
}

/// Reopen a closed agent session: start its agent again and bring it back to
/// the Agents list.
///
/// Resumes its conversation unless `fresh`, which starts a new one from the
/// brief — the session's own `default_shell` — for an agent okena cannot
/// resume. The closed mark is cleared only once the agent has started, so a
/// refused reopen leaves the session in the history where it was.
pub(super) fn reopen_agent(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    fresh: bool,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let Some(project) = ws.project(&project_id) else {
        return ActionResult::Err(format!("project not found: {project_id}"));
    };
    // Its conversation is filed under this directory, and the agent would
    // have nowhere to run: say why instead of spawning into a missing cwd.
    if !std::path::Path::new(&project.path).is_dir() {
        return ActionResult::Err(format!(
            "Its worktree was removed: {} no longer exists",
            project.path
        ));
    }
    // Either way the agent comes back in its own pane, created afresh when
    // closing took the session's layout with it.
    let result = if fresh {
        start_agent(
            ws,
            focus_manager,
            project_id.clone(),
            backend,
            terminals,
            settings,
            cx,
        )
    } else {
        restart_agent(
            ws,
            focus_manager,
            project_id.clone(),
            backend,
            terminals,
            settings,
            cx,
        )
    };
    if matches!(result, ActionResult::Ok(_)) {
        ws.with_project(&project_id, cx, |p| p.closed_at.take().is_some());
    }
    result
}

/// Kill `terminal_ids` and drop the session's whole layout — the tree at
/// once, so a pane whose terminal never started goes too rather than being
/// spawned the next time the session is shown. Reopening gives the agent a
/// new pane of its own.
fn close_agent_terminals(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: &str,
    terminal_ids: &[String],
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut impl WorkspaceCx,
) {
    for terminal_id in terminal_ids {
        if terminals.lock().contains_key(terminal_id) {
            ws.remember_closing_terminal_owner(project_id, terminal_id);
        }
        backend.kill(terminal_id);
        terminals.lock().remove(terminal_id);
    }
    ws.close_terminal_and_focus_sibling(focus_manager, project_id, &[], cx);
}

pub(super) fn close(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    terminal_id: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let path = find_terminal_path(ws, &project_id, &terminal_id);
    match path {
        Some(path) => {
            if terminals.lock().contains_key(&terminal_id) {
                ws.remember_closing_terminal_owner(&project_id, &terminal_id);
            }
            backend.kill(&terminal_id);
            terminals.lock().remove(&terminal_id);
            ws.close_terminal_and_focus_sibling(focus_manager, &project_id, &path, cx);
            ActionResult::Ok(None)
        }
        None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
    }
}

pub(super) fn close_many(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    terminal_ids: Vec<String>,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let mut last_err = None;
    for terminal_id in &terminal_ids {
        let path = find_terminal_path(ws, &project_id, terminal_id);
        match path {
            Some(path) => {
                if terminals.lock().contains_key(terminal_id) {
                    ws.remember_closing_terminal_owner(&project_id, terminal_id);
                }
                backend.kill(terminal_id);
                terminals.lock().remove(terminal_id);
                ws.close_terminal_and_focus_sibling(focus_manager, &project_id, &path, cx);
            }
            None => {
                last_err = Some(format!("terminal not found: {}", terminal_id));
            }
        }
    }
    match last_err {
        Some(e) => ActionResult::Err(e),
        None => ActionResult::Ok(None),
    }
}

pub(super) fn focus(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    terminal_id: String,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let path = find_terminal_path(ws, &project_id, &terminal_id);
    match path {
        Some(path) => {
            ws.set_focused_terminal(focus_manager, project_id, path, cx);
            ActionResult::Ok(None)
        }
        None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
    }
}

pub(super) fn send_text(
    ws: &mut Workspace,
    terminal_id: String,
    text: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
) -> ActionResult {
    with_ensured_terminal(ws, &terminal_id, backend, terminals, settings, |term| {
        term.send_input(&text);
        ActionResult::Ok(None)
    })
}

pub(super) fn send_bytes(
    ws: &mut Workspace,
    terminal_id: String,
    data: Vec<u8>,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
) -> ActionResult {
    with_ensured_terminal(ws, &terminal_id, backend, terminals, settings, |term| {
        term.send_bytes(&data);
        ActionResult::Ok(None)
    })
}

pub(super) fn run_command(
    ws: &mut Workspace,
    terminal_id: String,
    command: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
) -> ActionResult {
    with_ensured_terminal(ws, &terminal_id, backend, terminals, settings, |term| {
        term.send_input(&format!("{}\r", command));
        ActionResult::Ok(None)
    })
}

pub(super) fn send_special_key(
    ws: &mut Workspace,
    terminal_id: String,
    key: SpecialKey,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
) -> ActionResult {
    with_ensured_terminal(ws, &terminal_id, backend, terminals, settings, |term| {
        term.send_bytes(&key.to_bytes());
        ActionResult::Ok(None)
    })
}

pub(super) fn resize(
    ws: &mut Workspace,
    terminal_id: String,
    cols: u16,
    rows: u16,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
) -> ActionResult {
    with_ensured_terminal(ws, &terminal_id, backend, terminals, settings, |term| {
        let size = TerminalSize {
            cols,
            rows,
            cell_width: 8.0,
            cell_height: 16.0,
        };
        term.resize(size);
        ActionResult::Ok(None)
    })
}

pub(super) fn update_split_sizes(
    ws: &mut Workspace,
    project_id: String,
    path: Vec<usize>,
    sizes: Vec<f32>,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    ws.update_split_sizes(&project_id, &path, sizes, cx);
    ActionResult::Ok(None)
}

pub(super) fn toggle_minimized(
    ws: &mut Workspace,
    project_id: String,
    terminal_id: String,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    ws.toggle_terminal_minimized_by_id(&project_id, &terminal_id, cx);
    ActionResult::Ok(None)
}

pub(super) fn set_fullscreen(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    terminal_id: Option<String>,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    match terminal_id {
        Some(tid) => ws.set_fullscreen_terminal(focus_manager, project_id, tid, cx),
        None => ws.exit_fullscreen(focus_manager, cx),
    }
    ActionResult::Ok(None)
}

pub(super) fn rename(
    ws: &mut Workspace,
    project_id: String,
    terminal_id: String,
    name: String,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    ws.rename_terminal(&project_id, &terminal_id, name, cx);
    ActionResult::Ok(None)
}

pub(super) fn read_content(
    ws: &mut Workspace,
    terminal_id: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
) -> ActionResult {
    with_ensured_terminal(ws, &terminal_id, backend, terminals, settings, |term| {
        let content = term.with_content(screen_text);
        ActionResult::Ok(Some(serde_json::json!({"content": content})))
    })
}

/// alacritty keeps a cell's zero-width marks beside `cell.c`, so the base char
/// alone loses the accent of decomposed text.
fn screen_text<T: EventListener>(term: &Term<T>) -> String {
    let grid = term.grid();
    let screen_lines = grid.screen_lines();
    let cols = grid.columns();
    let mut lines = Vec::with_capacity(screen_lines);

    for row in 0..screen_lines as i32 {
        let mut line = String::with_capacity(cols);
        for col in 0..cols {
            let cell = &grid[Point::new(Line(row), Column(col))];
            line.push(cell.c);
            line.extend(cell.zerowidth().into_iter().flatten());
        }
        let trimmed = line.trim_end().to_string();
        lines.push(trimmed);
    }

    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }

    lines.join("\n")
}

pub(super) fn export_buffer(terminal_id: String, backend: &dyn TerminalBackend) -> ActionResult {
    match backend.capture_buffer(&terminal_id) {
        Some(path) => {
            // capture_buffer wrote a daemon-side temp file; read it back and
            // drop it — the real export is the client's own copy.
            let bytes = std::fs::read(&path).unwrap_or_default();
            let _ = std::fs::remove_file(&path);
            let content = String::from_utf8_lossy(&bytes).to_string();
            ActionResult::Ok(Some(serde_json::json!({ "content": content })))
        }
        None => {
            ActionResult::Err("buffer capture unavailable (requires a tmux session backend)".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::screen_text;
    use alacritty_terminal::event::{Event, EventListener};
    use alacritty_terminal::term::test::TermSize;
    use alacritty_terminal::term::{Config, Term};
    use alacritty_terminal::vte::ansi::Processor;

    struct NoopListener;

    impl EventListener for NoopListener {
        fn send_event(&self, _event: Event) {}
    }

    fn screen(output: &str) -> String {
        let mut term = Term::new(Config::default(), &TermSize::new(10, 3), NoopListener);
        let mut processor: Processor = Processor::new();
        processor.advance(&mut term, output.as_bytes());
        screen_text(&term)
    }

    #[test]
    fn a_screen_read_keeps_a_combining_mark_with_its_base_char() {
        assert_eq!(screen("e\u{0301}x"), "e\u{0301}x");
    }

    #[test]
    fn a_screen_read_keeps_a_mark_standing_over_a_blank_cell() {
        assert_eq!(screen(" \u{0301}x"), " \u{0301}x");
    }
}
