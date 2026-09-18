//! Configure and start a free-form agent session.
//!
//! The third way into an agent session, alongside starting work on a task and
//! drafting a spec. Those two derive everything — goal, working directory,
//! context — from the thing they are about; this one asks, because there is
//! nothing to derive it from.
//!
//! Everything the daemon needs is decided here rather than inferred later, so
//! what the user sees is exactly what gets started.

use crate::keybindings::Cancel;
use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms};
use crate::views::components::launch_pickers::{LaunchPickers, LaunchPickersEvent};
use crate::views::components::source_editor::BriefInput;
use crate::views::components::{SimpleInput, SimpleInputState};
use crate::workspace::focus::FocusManager;
use crate::workspace::state::{WindowId, Workspace};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_ui::agent_launcher::{AgentLauncher, Launch, LauncherStyle};
use okena_ui::modal::{modal_backdrop, modal_content};
use okena_workspace::requests::NewAgentPrefill;

pub enum NewAgentDialogEvent {
    Close,
}

pub struct NewAgentDialog {
    client: okena_transport::remote_action::RemoteActionClient,
    workspace: Entity<Workspace>,
    focus_manager: Entity<FocusManager>,
    window_id: WindowId,
    focus_handle: FocusHandle,
    goal_input: BriefInput,
    name_input: Entity<SimpleInputState>,
    root_input: Entity<SimpleInputState>,
    /// Projects the agent is pointed at and the context it is handed, as
    /// chips. Projects keep the order they were picked in, which is also the
    /// order they appear in the brief.
    pickers: Entity<LaunchPickers>,
    /// The configured agent, drawn as the launcher's default.
    default_agent: Option<String>,
    heading: Option<String>,
    /// Task the session is about, when a task's launcher sent us here.
    task: Option<okena_core::tasks::TaskRef>,
    /// Which card started it, when one filled the dialog in.
    purpose: Option<okena_core::harness::AgentPurpose>,
    starting: bool,
    error: Option<String>,
}

impl NewAgentDialog {
    pub fn new(
        client: okena_transport::remote_action::RemoteActionClient,
        workspace: Entity<Workspace>,
        focus_manager: Entity<FocusManager>,
        window_id: WindowId,
        default_agent: Option<String>,
        prefill: NewAgentPrefill,
        cx: &mut Context<Self>,
    ) -> Self {
        // An editor: a goal is a paragraph, and a breakdown's brief is several.
        let goal_input = BriefInput::new(
            "e.g. audit every crate for unwraps on user input and open a PR per crate",
        )
        .with_value(prefill.goal);
        let name_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Optional — derived from the goal")
                .default_value(prefill.name)
        });
        let root = prefill.root.clone();
        let root_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Optional — the selected project, or your projects root")
                .default_value(root)
        });
        let pickers = cx.new(|cx| {
            LaunchPickers::new(
                "new-agent",
                client.clone(),
                workspace.clone(),
                "Named in the brief with their paths. One project runs the agent inside \
                 it; several run it above them.",
                cx,
            )
        });
        // The launch button counts the projects.
        cx.subscribe(&pickers, |_, _, _: &LaunchPickersEvent, cx| cx.notify())
            .detach();
        if !prefill.project_ids.is_empty() || !prefill.context.is_empty() {
            // The pickers hold the ids this client shows: a remote daemon's
            // projects carry its connection's prefix.
            let connection = client.connection_id().to_string();
            let ids: Vec<String> = prefill
                .project_ids
                .iter()
                .map(|id| {
                    if connection == okena_transport::client::LOCAL_DAEMON_CONNECTION_ID {
                        id.clone()
                    } else {
                        okena_transport::client::make_prefixed_id(&connection, id)
                    }
                })
                .collect();
            let context = prefill.context.clone();
            pickers.update(cx, |pickers, cx| {
                pickers.set_project_ids(&ids, cx);
                pickers.preset_context(&context, cx);
            });
        }
        Self {
            client,
            workspace,
            focus_manager,
            window_id,
            focus_handle: cx.focus_handle(),
            goal_input,
            name_input,
            root_input,
            pickers,
            default_agent,
            heading: prefill.heading,
            task: prefill.task,
            purpose: prefill.purpose,
            starting: false,
            error: None,
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(NewAgentDialogEvent::Close);
    }

    /// Start the session with `agent_command`; empty opens a plain shell.
    fn start(&mut self, agent_command: String, model: Option<String>, cx: &mut Context<Self>) {
        if self.starting {
            return;
        }
        let goal = self.goal_input.value(cx).trim().to_string();
        if goal.is_empty() {
            self.error = Some("Describe what the agent should do first.".into());
            cx.notify();
            return;
        }
        let name = self.name_input.read(cx).value().trim().to_string();
        let root = self.root_input.read(cx).value().trim().to_string();
        let (project_ids, context) = {
            let pickers = self.pickers.read(cx);
            // As the daemon names them: posted straight through the client,
            // past the dispatcher that would otherwise strip a remote prefix.
            (pickers.daemon_project_ids(cx), pickers.context_refs(cx))
        };
        let task = self.task.clone();
        let purpose = self.purpose.clone();
        // A session about a task is started from where you were reading the
        // task; jumping into its terminal would lose that place. Its launcher
        // shows it once it appears.
        let focus_after = task.is_none();

        self.starting = true;
        self.error = None;
        cx.notify();

        let client = self.client.clone();
        let workspace = self.workspace.clone();
        let focus_manager = self.focus_manager.clone();
        let window_id = self.window_id;
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::AgentStartSession {
                        goal,
                        name,
                        root,
                        project_ids,
                        // An explicit empty string is the daemon's "no agent,
                        // just a shell".
                        agent_command: Some(agent_command),
                        model,
                        // This dialog configures a session about work that
                        // exists; drafting a task is its own flow.
                        task_draft: None,
                        task,
                        // Free-form unless a card (an extension's action)
                        // filled it in, which then lists it.
                        purpose,
                        context,
                    })
                    .and_then(|v| v.ok_or_else(|| "Missing session result".to_string()))
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.starting = false;
                    match result {
                        Ok(value) => {
                            // Focus the new session, so starting one lands you
                            // in it rather than back where you were.
                            if let Some(id) = value
                                .get("project_id")
                                .and_then(|v| v.as_str())
                                .filter(|_| focus_after)
                            {
                                let id = id.to_string();
                                okena_workspace::harness_state::set_active_harness(
                                    window_id, None, cx,
                                );
                                focus_manager.update(cx, |fm, cx| {
                                    workspace.update(cx, |ws, cx| {
                                        ws.set_focused_project_individual(fm, Some(id), cx);
                                    });
                                    cx.notify();
                                });
                            }
                            this.close(cx);
                        }
                        Err(e) => this.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn field_label(&self, label: &str, cx: &App) -> AnyElement {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_secondary))
            .child(label.to_string())
            .into_any_element()
    }

    fn field_hint(&self, hint: &str, cx: &App) -> AnyElement {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(hint.to_string())
            .into_any_element()
    }
}

impl EventEmitter<NewAgentDialogEvent> for NewAgentDialog {}

impl Focusable for NewAgentDialog {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for NewAgentDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        self.goal_input.ensure(window, cx);
        let focus_handle = self.focus_handle.clone();
        if !focus_handle.contains_focused(window, cx) {
            window.focus(&focus_handle, cx);
        }

        let project_count = self.pickers.read(cx).project_ids(cx).len();
        let launcher = AgentLauncher::new(
            "new-agent-launcher",
            match project_count {
                0 => "Start the session".to_string(),
                1 => "Start in 1 project".to_string(),
                n => format!("Start across {n} projects"),
            },
        )
        .style(LauncherStyle::Inline)
        .options({
            let mut options =
                crate::views::agent_session::launch_options(self.default_agent.as_deref(), &t);
            // Last, not first: a plain shell in a chosen directory is a
            // legitimate thing to want, but not the thing most people came for.
            options.push(crate::views::agent_session::no_agent_option(
                "Plain shell",
                &t,
            ));
            options
        })
        .preferred(self.default_agent.clone())
        .busy(self.starting.then_some("Starting…"))
        .brief(crate::views::launch_briefs::brief_for(
            &self.client,
            "agent-session",
            cx,
        ))
        .on_launch(cx.listener(|this, launch: &Launch, _window, cx| {
            this.start(launch.command.to_string(), launch.model.clone(), cx);
        }));

        modal_backdrop("new-agent-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("NewAgentDialog")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _, cx| this.close(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.close(cx)),
            )
            .child(
                // Laid out like the other launch dialogs (start work, projects
                // and context): a fixed header, a scrolling body, the launcher
                // alone in the footer.
                modal_content("new-agent-modal", &t)
                    .w(px(760.0))
                    .max_h(px(620.0))
                    .child(
                        h_flex()
                            .items_start()
                            .gap(px(8.0))
                            .px(px(16.0))
                            .py(px(12.0))
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(ui_text(14.0, cx))
                                            .text_color(rgb(t.text_primary))
                                            .child(
                                                self.heading
                                                    .clone()
                                                    .unwrap_or_else(|| "New agent".to_string()),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .text_size(ui_text_ms(cx))
                                            .text_color(rgb(t.text_muted))
                                            .child(
                                                "Starts an agent session with okena's MCP \
                                                 server wired in, so it can report status \
                                                 and register what it produces.",
                                            ),
                                    ),
                            )
                            // With the heading, as in the other launch dialogs:
                            // leaving and starting do not belong side by side.
                            .child(
                                div()
                                    .id("new-agent-cancel")
                                    .cursor_pointer()
                                    .flex_shrink_0()
                                    .px(px(10.0))
                                    .py(px(3.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(t.bg_secondary))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .text_size(ui_text_md(cx))
                                    .text_color(rgb(t.text_primary))
                                    .child("Cancel")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _window, cx| {
                                            cx.stop_propagation();
                                            this.close(cx);
                                        }),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .id("new-agent-body")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .gap(px(14.0))
                            .p(px(16.0))
                            .child(
                                v_flex()
                                    .gap(px(5.0))
                                    .child(self.field_label("Goal", cx))
                                    // A fixed box that scrolls rather than
                                    // grows: a goal handed over by a launcher
                                    // is a whole brief, and an input sized to
                                    // it would push the rest of the form out
                                    // of the dialog.
                                    .child(self.goal_input.render(180.0, cx))
                                    .child(self.field_hint(
                                        "What the agent is asked to do. This is its opening \
                                         prompt, so context beats brevity.",
                                        cx,
                                    )),
                            )
                            // Projects, then the context they rank first.
                            .child(self.pickers.clone())
                            .child(
                                v_flex()
                                    .gap(px(5.0))
                                    .child(self.field_label("Working directory", cx))
                                    .child(
                                        okena_ui::input::input_container(&t, None)
                                            .w_full()
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .child(
                                                SimpleInput::new(&self.root_input)
                                                    .text_size(ui_text(13.0, cx)),
                                            ),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap(px(5.0))
                                    .child(self.field_label("Session name", cx))
                                    .child(
                                        okena_ui::input::input_container(&t, None)
                                            .w_full()
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .child(
                                                SimpleInput::new(&self.name_input)
                                                    .text_size(ui_text(13.0, cx)),
                                            ),
                                    ),
                            )
                            .children(self.error.clone().map(|e| {
                                div()
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .rounded(px(4.0))
                                    .bg(with_alpha(t.error, 0.1))
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(t.error))
                                    .child(e)
                                    .into_any_element()
                            })),
                    )
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .child(launcher),
                    ),
            )
    }
}
