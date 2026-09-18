//! The agent-session panel — one view of a coding-agent session, shown wherever
//! a session appears.
//!
//! There used to be two of these: a sidebar beside a session's terminal and a
//! card in the Agents overview, separately written and already diverging. This
//! is the single one, rendered at two densities so it fits both a full-height
//! sidebar and a narrow lane.
//!
//! It is an entity rather than a render helper because its hosts are different
//! types — the window and the harness pane — and the actions it offers (focus a
//! worktree, restart the agent, delete the workspace) need a workspace, a focus
//! manager and a daemon client of their own.

mod detect;
mod launch;
mod model;
mod render;

pub use detect::{AGENT_COMMANDS, detect_agent};
pub use launch::{launch_option, launch_options, launcher_session, no_agent_option};
pub use model::{
    AgentSessionInfo, AgentSessionKind, RelatedAgent, RelatedWorkspace, SessionActivity,
    session_kind,
};

use crate::workspace::focus::FocusManager;
use crate::workspace::state::{WindowId, Workspace};
use gpui::*;
use okena_terminal::TerminalsRegistry;

/// Where the panel is being shown, and so how much it draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelDensity {
    /// A lane in the Agents overview, where several sessions sit side by side:
    /// its own header with Info/Terminal tabs, and no destructive action —
    /// tearing down a workspace from a card you are scanning past is too easy
    /// to do by accident.
    Compact,
    /// Inside a project column, in place of its terminal. No header of its own:
    /// the column already names the session and owns the toggle that got you
    /// here, and a second header would just repeat it.
    Embedded,
}

impl PanelDensity {
    /// Whether the panel draws its own header. Only false where the host
    /// already has one.
    fn has_header(self) -> bool {
        !matches!(self, PanelDensity::Embedded)
    }

    /// Whether the destructive teardown is offered here.
    ///
    /// The column is the one place a session is shown on its own, full height,
    /// having been deliberately opened — the overview's lanes are for scanning
    /// across sessions, which is the wrong place to offer deleting one.
    fn allows_delete(self) -> bool {
        matches!(self, PanelDensity::Embedded)
    }
}

/// What the delete card takes down with the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DeleteChoice {
    /// Off keeps each checkout as an ordinary worktree, terminals closed.
    pub remove_worktrees: bool,
    /// Only offered while worktrees go: git cannot delete a branch a kept
    /// worktree has checked out.
    pub delete_branches: bool,
}

impl Default for DeleteChoice {
    fn default() -> Self {
        Self {
            remove_worktrees: true,
            delete_branches: false,
        }
    }
}

/// What a finished delete took that the user may not have expected: work it
/// discarded and branches that were never merged. One line each, empty when
/// there is nothing to say.
fn teardown_notes(result: &serde_json::Value) -> Vec<String> {
    let mut notes = Vec::new();
    let discarded: Vec<&str> = result
        .get("discarded")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|n| n.as_str()).collect())
        .unwrap_or_default();
    if !discarded.is_empty() {
        notes.push(format!(
            "Discarded uncommitted changes in: {}",
            discarded.join(", ")
        ));
    }
    let unmerged: Vec<&str> = result
        .get("deleted_branches")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter(|b| b.get("merged").and_then(|m| m.as_bool()) == Some(false))
                .filter_map(|b| b.get("branch")?.as_str())
                .collect()
        })
        .unwrap_or_default();
    if !unmerged.is_empty() {
        notes.push(format!(
            "Deleted unmerged branches: {}",
            unmerged.join(", ")
        ));
    }
    notes
}

/// What a compact panel is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PanelTab {
    #[default]
    Info,
    Terminal,
}

/// Everything an info panel needs from its host — an agent session's, or a
/// project's. One context for both, since a column hosts whichever its project
/// calls for and the window hands it the same handles either way.
#[derive(Clone)]
pub struct InfoPanelContext {
    pub client: okena_transport::remote_action::RemoteActionClient,
    /// Lets the panel open a worktree's diff without knowing which window it
    /// is in.
    pub request_broker: Entity<okena_workspace::request_broker::RequestBroker>,
    pub workspace: Entity<Workspace>,
    pub focus_manager: Entity<FocusManager>,
    pub window_id: WindowId,
    pub terminals: TerminalsRegistry,
    /// Says when the client's daemon connection comes back, which may bring a
    /// daemon that knows more actions. `None` where there is no manager.
    pub remote_manager: Option<Entity<okena_remote_client::RemoteConnectionManager>>,
}

pub struct AgentSessionPanel {
    client: okena_transport::remote_action::RemoteActionClient,
    request_broker: Entity<okena_workspace::request_broker::RequestBroker>,
    workspace: Entity<Workspace>,
    focus_manager: Entity<FocusManager>,
    window_id: WindowId,
    terminals: TerminalsRegistry,
    /// The session this panel describes.
    project_id: String,
    density: PanelDensity,
    /// Info or the live terminal. Only meaningful at compact density; the full
    /// sidebar sits beside a terminal already.
    tab: PanelTab,
    /// Two-step delete: the first click arms, the second confirms. The choices
    /// on the card while armed, fresh each time it opens.
    pending_delete: Option<DeleteChoice>,
    /// The embedded terminal, keyed by id so it is rebuilt only when the
    /// session's terminal actually changes — rebuilding each frame would drop
    /// scrollback and selection.
    terminal: Option<EmbeddedTerminal>,
    /// Resolves the transport a session's terminal rides on. `None` in hosts
    /// that never show the terminal tab.
    remote_manager: Option<Entity<okena_remote_client::RemoteConnectionManager>>,
    terminal_focus: FocusHandle,
    /// An instruction on its way, so a double click does not send it twice.
    sending: bool,
    /// When this panel last asked for each filed task's state, answered or
    /// still on the way. A task is asked about again only once that is older
    /// than [`crate::views::known_tasks::STATE_STALE_AFTER`], however often the
    /// panel renders.
    task_states_requested: std::collections::HashMap<okena_core::tasks::TaskId, std::time::Instant>,
    /// The generation of the client's connection, kept current so a daemon
    /// that refused `TaskGetMany` is asked again once it has reconnected.
    connection_generation: u64,
}

struct EmbeddedTerminal {
    terminal_id: String,
    terminal: std::sync::Arc<okena_terminal::terminal::Terminal>,
    content: Entity<okena_views_terminal::layout::terminal_pane::TerminalContent>,
}

impl AgentSessionPanel {
    pub fn new(
        project_id: String,
        density: PanelDensity,
        ctx: InfoPanelContext,
        cx: &mut Context<Self>,
    ) -> Self {
        // Tasks this agent filed show their current state, whoever loaded it:
        // this panel's own fetch, or the Tasks view.
        let known_tasks = crate::views::known_tasks::entity(cx);
        cx.observe(&known_tasks, |_, _, cx| cx.notify()).detach();
        let connection_generation = ctx.remote_manager.as_ref().map_or(0, |rm| {
            rm.read(cx).connected_generation(ctx.client.connection_id())
        });
        // A reconnect reads every task's state afresh, on a daemon that may
        // now know the batch. Only then: the manager notifies on far more.
        if let Some(rm) = &ctx.remote_manager {
            cx.observe(rm, |this, rm, cx| {
                let generation = rm
                    .read(cx)
                    .connected_generation(this.client.connection_id());
                if generation != this.connection_generation {
                    this.connection_generation = generation;
                    this.task_states_requested.clear();
                    cx.notify();
                }
            })
            .detach();
        }
        Self {
            client: ctx.client,
            request_broker: ctx.request_broker,
            workspace: ctx.workspace,
            focus_manager: ctx.focus_manager,
            window_id: ctx.window_id,
            terminals: ctx.terminals,
            project_id,
            density,
            tab: PanelTab::default(),
            pending_delete: None,
            terminal: None,
            remote_manager: None,
            terminal_focus: cx.focus_handle(),
            sending: false,
            task_states_requested: Default::default(),
            connection_generation,
        }
    }

    /// Let this panel show the session's live terminal.
    pub fn with_terminals(
        mut self,
        remote_manager: Option<Entity<okena_remote_client::RemoteConnectionManager>>,
    ) -> Self {
        self.remote_manager = remote_manager;
        self
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// Point an existing panel at a different session.
    ///
    /// Reused rather than rebuilt so the sidebar keeps its scroll position when
    /// focus moves between sessions; the per-session state that must not carry
    /// over is reset here.
    pub fn set_project(&mut self, project_id: String, cx: &mut Context<Self>) {
        if self.project_id == project_id {
            return;
        }
        self.project_id = project_id;
        // Both belong to the session that just left: an armed delete would
        // otherwise point at the newly-shown one.
        self.pending_delete = None;
        self.terminal = None;
        self.sending = false;
        // Showing a session again reads its tasks' states afresh, starting
        // now rather than at the next frame.
        self.task_states_requested.clear();
        if let Some(info) = self.info(cx) {
            self.fetch_task_states(&info.assets, cx);
        }
        cx.notify();
    }

    /// Ask the provider for the state of the tasks this agent filed.
    ///
    /// The panel asks for itself rather than relying on the Tasks view, which
    /// may never have been opened. A task is asked about when it first shows,
    /// and again once its answer is older than the staleness window, so a task
    /// moved to Done while the panel stays open reads as done within a minute.
    /// One batch per provider, through the session's own client, so a remote
    /// session's tasks are read by the machine whose agent filed them.
    ///
    /// Called from render as well as `set_project`: render is where a new
    /// asset first reaches the panel, and the request times make a call that
    /// has nothing due free.
    fn fetch_task_states(
        &mut self,
        assets: &[okena_core::session_assets::SessionAsset],
        cx: &mut Context<Self>,
    ) {
        use crate::views::known_tasks::{
            STATE_STALE_AFTER, batch_unsupported, is_batch_unknown, record_batch_unsupported,
            tasks_to_fetch,
        };
        let connection_id = self.client.connection_id().to_string();
        let generation = self.connection_generation;
        // A daemon that predates the batch would refuse every one of them.
        if batch_unsupported(&connection_id, generation, cx) {
            return;
        }
        let now = std::time::Instant::now();
        let batches = tasks_to_fetch(assets, &self.task_states_requested, now, STATE_STALE_AFTER);
        for (provider, ids) in batches {
            for id in &ids {
                self.task_states_requested.insert(
                    okena_core::tasks::TaskId::new(provider.clone(), id.clone()),
                    now,
                );
            }
            let client = self.client.clone();
            let connection_id = connection_id.clone();
            cx.spawn(async move |this, cx| {
                let result = smol::unblock(move || {
                    client
                        .post_action(okena_core::api::ActionRequest::TaskGetMany {
                            provider: provider.clone(),
                            task_external_ids: ids,
                        })
                        .and_then(|v| v.ok_or_else(|| "the answer had no tasks".to_string()))
                        .and_then(|v| {
                            serde_json::from_value::<Vec<okena_core::tasks::Task>>(
                                v["tasks"].clone(),
                            )
                            .map_err(|e| format!("unexpected tasks: {e}"))
                        })
                        .map_err(|e| (provider, e))
                })
                .await;
                match result {
                    Ok(tasks) => {
                        cx.update(|cx| crate::views::known_tasks::remember(&tasks, cx));
                    }
                    // Not a failure to retry: this daemon will refuse the batch
                    // until it is replaced. Said once per connection, and no
                    // wake-up — the reconnect that may fix it notifies instead.
                    Err((_, e)) if is_batch_unknown(&e) => {
                        let news = cx.update(|cx| {
                            record_batch_unsupported(&connection_id, generation, cx)
                        });
                        if news {
                            log::warn!(
                                "[tasks] the daemon on connection {connection_id} cannot read tasks in bulk; filed tasks show no state until it reconnects"
                            );
                        }
                        return;
                    }
                    // Asked again after the same window as a success: a row
                    // without a state is better than a request every frame.
                    Err((provider, e)) => {
                        log::warn!(
                            "[tasks] could not read the state of filed {provider} tasks: {e}"
                        );
                    }
                }
                // One wake-up when the answer goes stale, so a panel nobody is
                // interacting with still re-renders and asks again. Not a
                // loop: the next fetch schedules the next wake-up.
                smol::Timer::after(STATE_STALE_AFTER).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
    }

    /// Type `text` into the agent and submit it.
    ///
    /// Through the daemon rather than straight into the terminal: it knows
    /// which terminal runs the agent, submits the way an agent's prompt needs,
    /// and clears the "waiting on you" flag in the same step.
    fn send_instruction(&mut self, text: String, cx: &mut Context<Self>) {
        let text = text.trim().to_string();
        if text.is_empty() || self.sending {
            return;
        }
        self.sending = true;
        cx.notify();
        let client = self.client.clone();
        let daemon_id =
            okena_transport::client::strip_prefix(&self.project_id, client.connection_id());
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client.post_action(okena_core::api::ActionRequest::AgentSendInstruction {
                    project_id: daemon_id,
                    text,
                })
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.sending = false;
                    match result {
                        Ok(_) => {}
                        Err(error) => crate::views::panels::toast::ToastManager::error(
                            format!("Could not reach the agent: {error}"),
                            cx,
                        ),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Everything shown, read fresh from the workspace mirror each frame.
    fn info(&self, cx: &App) -> Option<AgentSessionInfo> {
        AgentSessionInfo::collect(self.workspace.read(cx), &self.terminals, &self.project_id)
    }

    /// Show what changed in a worktree.
    fn open_diff(
        &mut self,
        project_id: &str,
        mode: Option<okena_core::types::DiffMode>,
        cx: &mut Context<Self>,
    ) {
        crate::views::components::project_nav::open_diff(
            &self.request_broker,
            project_id,
            mode,
            cx,
        );
    }

    /// Focus a project in the terminal workspace, leaving any harness view.
    fn open_project(&mut self, project_id: String, cx: &mut Context<Self>) {
        crate::views::components::project_nav::focus_project(
            &self.workspace,
            &self.focus_manager,
            self.window_id,
            &project_id,
            cx,
        );
        cx.notify();
    }

    /// Start the agent again from its brief, in its own pane.
    ///
    /// The daemon replaces whatever the agent's pane runs with the session's
    /// launch — the agent command, prompt and MCP config chosen when the
    /// session was started. The session's other terminals are left alone.
    fn restart_agent(&mut self, cx: &mut Context<Self>) {
        self.post_agent_action(
            |project_id| okena_core::api::ActionRequest::AgentStart { project_id },
            "Could not start the agent",
            cx,
        );
    }

    /// Send one of the agent controls, toasting the error when it fails.
    fn post_agent_action(
        &mut self,
        action: fn(String) -> okena_core::api::ActionRequest,
        failure: &'static str,
        cx: &mut Context<Self>,
    ) {
        let client = self.client.clone();
        let daemon_id =
            okena_transport::client::strip_prefix(&self.project_id, client.connection_id());
        cx.spawn(async move |_this, cx| {
            let result = smol::unblock(move || client.post_action(action(daemon_id))).await;
            if let Err(error) = result {
                cx.update(|cx| {
                    crate::views::panels::toast::ToastManager::error(
                        format!("{failure}: {error}"),
                        cx,
                    );
                });
            }
        })
        .detach();
    }

    /// Restart the agent and resume its conversation.
    ///
    /// Unlike "Start agent", which begins again from the brief, this brings
    /// back the conversation the agent was having — what it had read, decided
    /// and been told. It is also how an agent picks up a rebuilt okena: the
    /// restarted process reconnects okena's tools on the current build.
    fn resume_agent(&mut self, cx: &mut Context<Self>) {
        self.post_agent_action(
            |project_id| okena_core::api::ActionRequest::AgentRestart { project_id },
            "Could not restart the agent",
            cx,
        );
    }

    /// Stop the agent without tearing anything down.
    ///
    /// Ends the agent's process only; its pane stays, marked stopped, and the
    /// session's other terminals keep running. The session project, its
    /// worktrees and any work on disk all stay — "Start agent" brings it back.
    ///
    /// Separate from deleting because they are different intents that were
    /// previously only reachable as one: stopping a runaway agent should not
    /// require destroying the checkouts it was working in.
    fn stop_agent(&mut self, cx: &mut Context<Self>) {
        self.post_agent_action(
            |project_id| okena_core::api::ActionRequest::AgentStop { project_id },
            "Could not stop the agent",
            cx,
        );
    }

    /// Tear down the session, and with `choice` its worktrees and branches.
    ///
    /// Only ever called from the confirmation step. Removing worktrees always
    /// forces: what that discarded is reported afterwards rather than asked
    /// about first.
    fn delete_workspace(&mut self, choice: DeleteChoice, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let daemon_id =
            okena_transport::client::strip_prefix(&self.project_id, client.connection_id());
        self.pending_delete = None;
        cx.notify();

        cx.spawn(async move |_this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(okena_core::api::ActionRequest::TaskDeleteWorkspace {
                        project_id: daemon_id,
                        force: true,
                        remove_worktrees: choice.remove_worktrees,
                        delete_branches: choice.remove_worktrees && choice.delete_branches,
                    })
                    .and_then(|v| v.ok_or_else(|| "Missing delete result".to_string()))
            })
            .await;

            cx.update(|cx| match result {
                // Anything that survived must be surfaced: silently leaving it
                // would look like the workspace was fully torn down.
                Ok(value) => {
                    let notes = teardown_notes(&value);
                    if !notes.is_empty() {
                        crate::views::panels::toast::ToastManager::warning(notes.join(" · "), cx);
                    }
                    let failures: Vec<String> = value
                        .get("failed")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|f| {
                                    let name = f.get("project")?.as_str()?;
                                    let err = f.get("error")?.as_str()?;
                                    Some(format!("{name}: {err}"))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    if !failures.is_empty() {
                        crate::views::panels::toast::ToastManager::error(
                            format!("Some parts were kept — {}", failures.join(" · ")),
                            cx,
                        );
                    }
                }
                Err(error) => crate::views::panels::toast::ToastManager::error(
                    format!("Could not delete the workspace: {error}"),
                    cx,
                ),
            });
        })
        .detach();
    }
}

// ── The terminal tab ─────────────────────────────────────────────────────────
//
// Only reachable at compact density, where the panel replaces a terminal the
// user would otherwise have to leave the overview to see.

impl AgentSessionPanel {
    /// Resolve the transport carrying this session's terminals.
    fn transport(
        &self,
        cx: &App,
    ) -> Option<std::sync::Arc<dyn okena_terminal::terminal::TerminalTransport>> {
        let connection_id = self
            .workspace
            .read(cx)
            .project(&self.project_id)?
            .connection_id
            .clone()?;
        let backend = self
            .remote_manager
            .as_ref()?
            .read(cx)
            .backend_for(&connection_id)?;
        Some(backend.transport())
    }

    /// Build the embedded terminal, or drop it when the session's terminal has.
    ///
    /// Keyed by terminal id: rebuilding on every frame would throw away the
    /// scrollback and any selection the user is part-way through.
    pub(super) fn sync_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(terminal_id) =
            AgentSessionInfo::visible_terminal_id(self.workspace.read(cx), &self.project_id)
        else {
            self.terminal = None;
            return;
        };
        if self
            .terminal
            .as_ref()
            .is_some_and(|t| t.terminal_id == terminal_id)
        {
            return;
        }
        let Some(transport) = self.transport(cx) else {
            self.terminal = None;
            return;
        };

        let (layout_path, project_path) = {
            let ws = self.workspace.read(cx);
            match ws.project(&self.project_id) {
                Some(p) => (
                    p.layout
                        .as_ref()
                        .and_then(|l| l.find_terminal_path(&terminal_id))
                        .unwrap_or_default(),
                    p.path.clone(),
                ),
                None => (Vec::new(), String::new()),
            }
        };

        let terminal =
            okena_views_terminal::overlays::terminal_overlay_utils::get_or_create_terminal(
                &terminal_id,
                &transport,
                &self.terminals,
                &project_path,
            );
        // A local broker keeps TerminalContent self-contained: this panel has
        // no overlay manager observing requests, so a file-viewer request from
        // a Ctrl+click would otherwise reach nothing.
        let request_broker = cx.new(|_| okena_workspace::request_broker::RequestBroker::new());
        let content =
            okena_views_terminal::overlays::terminal_overlay_utils::create_terminal_content(
                cx,
                self.terminal_focus.clone(),
                self.project_id.clone(),
                layout_path,
                self.workspace.clone(),
                request_broker,
                terminal.clone(),
            );

        self.terminal = Some(EmbeddedTerminal {
            terminal_id,
            terminal,
            content,
        });
    }

    pub(super) fn render_terminal(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        use gpui::prelude::*;
        let t = crate::theme::theme(cx);
        let Some(term) = &self.terminal else {
            return gpui_component::v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(crate::ui::tokens::ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child("No terminal in this session."),
                )
                .into_any_element();
        };
        let terminal = term.terminal.clone();
        div()
            .id(SharedString::from(format!(
                "agent-term-{}",
                self.project_id
            )))
            .flex_1()
            .min_h_0()
            .bg(rgb(t.bg_primary))
            .track_focus(&self.terminal_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.terminal_focus, cx);
                }),
            )
            // Forwarded here rather than relying on the panel's own handling:
            // without this the agent cannot be answered, which is the point of
            // showing its terminal instead of a link to it.
            // A listener rather than a bare closure: key encoding reads the
            // terminal view settings (Option-as-Meta among them), which need a
            // context to resolve.
            .on_key_down(cx.listener(move |_this, event, _window, cx| {
                okena_views_terminal::overlays::terminal_overlay_utils::handle_terminal_key_input(
                    &terminal, event, cx,
                );
            }))
            .child(
                AnyView::from(term.content.clone()).cached(StyleRefinement::default().size_full()),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod teardown_notes_tests {
    use super::teardown_notes;

    #[test]
    fn names_discarded_work_and_only_the_unmerged_branches() {
        let result = serde_json::json!({
            "discarded": ["okena (a)", "web (b)"],
            "deleted_branches": [
                { "branch": "feat/merged", "project": "okena (a)", "merged": true },
                { "branch": "feat/open", "project": "web (b)", "merged": false },
            ],
        });
        assert_eq!(
            teardown_notes(&result),
            [
                "Discarded uncommitted changes in: okena (a), web (b)",
                "Deleted unmerged branches: feat/open",
            ]
        );
    }

    #[test]
    fn a_clean_delete_and_an_older_daemon_say_nothing() {
        let clean = serde_json::json!({
            "discarded": [],
            "deleted_branches": [{ "branch": "feat/x", "project": "p", "merged": true }],
        });
        assert!(teardown_notes(&clean).is_empty());
        // A daemon that predates these fields answers with neither.
        assert!(teardown_notes(&serde_json::json!({ "removed": [], "failed": [] })).is_empty());
    }
}
