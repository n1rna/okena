//! Create a task.
//!
//! Shown in place of the board, the way drafting a spec replaces the Specs
//! view: filling this in is a different job from scanning the list, and
//! splitting the width between them served neither.
//!
//! okena models a breakdown — epic, feature, story, task, defect — that no
//! provider has natively. Linear carries it as a label, and the daemon resolves
//! or creates that label; the form only has to say which kind is meant.

use super::HarnessPane;
use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms};
use crate::views::components::SimpleInput;
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::tasks::{Task, TaskKind};
use std::collections::BTreeMap;

/// A team or project a task can be filed in.
#[derive(Clone, Debug, serde::Deserialize)]
pub(crate) struct TaskContainer {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub key: String,
}

/// State of the "New task" form.
///
/// Top-level only. Sub-tasks are made by an agent through okena's MCP server,
/// which is the whole point of the breakdown flow — a form for typing them one
/// at a time was the slower half of the same job.
pub(crate) struct NewTaskForm {
    pub kind: TaskKind,
    /// `None` until the teams have loaded, or when there is only one.
    pub container_id: Option<String>,
    pub containers: Vec<TaskContainer>,
    pub creating: bool,
    /// A drafting agent is starting. Kept apart from `creating` so the
    /// launcher can say which of the two is in flight.
    pub drafting: bool,
    pub error: Option<String>,
}

impl HarnessPane {
    /// Open the form.
    pub(super) fn open_new_task(&mut self, cx: &mut Context<Self>) {
        // The form takes the detail panel, so nothing is selected while it is
        // open: a highlighted row whose detail you cannot see reads as a bug,
        // and closing the form would then restore a selection you had stopped
        // thinking about.
        self.tasks.selected = None;
        self.tasks.new_task = Some(NewTaskForm {
            kind: TaskKind::Task,
            container_id: None,
            containers: Vec::new(),
            creating: false,
            drafting: false,
            error: None,
        });
        self.tasks
            .new_task_title
            .update(cx, |i, cx| i.set_value("", cx));
        self.tasks
            .new_task_body
            .update(cx, |i, cx| i.set_value("", cx));
        self.load_containers(cx);
        cx.notify();
    }

    pub(super) fn close_new_task(&mut self, cx: &mut Context<Self>) {
        self.tasks.new_task = None;
        cx.notify();
    }

    /// Read the teams the user can file in.
    fn load_containers(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let provider = self.tasks.provider.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::TaskContainers { provider })
                    .and_then(|v| v.ok_or_else(|| "Missing teams".to_string()))
                    .map(|v| {
                        v.get("containers")
                            .and_then(|c| {
                                serde_json::from_value::<Vec<TaskContainer>>(c.clone()).ok()
                            })
                            .unwrap_or_default()
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    let Some(form) = this.tasks.new_task.as_mut() else {
                        return;
                    };
                    match result {
                        Ok(containers) => {
                            // With one team there is no choice to make, so it
                            // is chosen rather than presented.
                            if containers.len() == 1 {
                                form.container_id = Some(containers[0].id.clone());
                            }
                            form.containers = containers;
                        }
                        // Not fatal: a provider that cannot enumerate teams can
                        // still create sub-tasks, and the form says so.
                        Err(e) => form.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Create the task, then close the form and refresh the board.
    pub(super) fn submit_new_task(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.tasks.new_task.as_ref() else {
            return;
        };
        if form.creating {
            return;
        }
        let title = self
            .tasks
            .new_task_title
            .read(cx)
            .value()
            .trim()
            .to_string();
        if title.is_empty() {
            if let Some(form) = self.tasks.new_task.as_mut() {
                form.error = Some("Give the task a title first.".into());
            }
            cx.notify();
            return;
        }
        let container_id = form.container_id.clone();
        if container_id.is_none() {
            if let Some(form) = self.tasks.new_task.as_mut() {
                form.error = Some("Choose a team for the new task.".into());
            }
            cx.notify();
            return;
        }
        let kind = form.kind;
        let description = self.tasks.new_task_body.read(cx).value().trim().to_string();
        let provider = self.tasks.provider.clone();

        if let Some(form) = self.tasks.new_task.as_mut() {
            form.creating = true;
            form.error = None;
        }
        cx.notify();

        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::TaskCreate {
                        provider,
                        title,
                        description,
                        kind: kind.wire_name().to_string(),
                        parent_external_id: None,
                        container_id,
                    })
                    .and_then(|v| v.ok_or_else(|| "Missing created task".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<Task>(v)
                            .map_err(|e| format!("Unexpected task: {e}"))
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(task) => {
                            // The parent's cached children no longer include
                            // everything; drop it so the detail refetches.
                            if let Some(parent) = task.parent_id.as_ref() {
                                this.tasks.children.remove(parent);
                            }
                            this.tasks.new_task = None;
                            // Select it: you almost always want to look at what
                            // you just made, and it may not be assigned to you,
                            // in which case the refresh below will not list it.
                            this.tasks.selected = Some(task.id.external_id.clone());
                            this.refresh_tasks(cx);
                        }
                        Err(e) => {
                            if let Some(form) = this.tasks.new_task.as_mut() {
                                form.creating = false;
                                form.error = Some(e);
                            }
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Start `agent` on breaking `task` into sub-tasks.
    ///
    /// A free-form session rather than a task session: it is not doing the
    /// work, it is deciding what the work is, so it gets no worktrees. okena's
    /// MCP server is what makes it useful — the agent reads the existing
    /// children and writes new ones back through `okena_create_subtask`,
    /// rather than handing the user a list to retype.
    pub(super) fn break_down_with_agent(
        &mut self,
        task: &Task,
        agent: String,
        project_ids: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let vars = breakdown_vars(task);

        if self.tasks.breaking_down.is_some() {
            return;
        }
        self.tasks.breaking_down = Some(task.id.external_id.clone());
        cx.notify();

        let project_ids: Vec<String> = project_ids.iter().map(|id| self.daemon_id(id)).collect();
        let client = self.client.clone();
        let name = breakdown_name(task);
        // The session is linked to the task so the agent's `okena_whoami`
        // resolves it, and so the MCP tools default to the right parent.
        let link = okena_core::tasks::TaskRef::from(task);
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                // Two round trips rather than one: rendering has to happen
                // where the store registry is, and starting a session with a
                // half-rendered brief would be worse than a moment's wait.
                let goal = render_brief(&client, "break-down", vars)?;
                client
                    .post_action(ActionRequest::AgentStartSession {
                        goal,
                        name,
                        root: String::new(),
                        // Context only: named in the brief, never given
                        // worktrees — a breakdown decides the work, it does
                        // not do it.
                        project_ids,
                        agent_command: Some(agent),
                        task_draft: None,
                        task: Some(link),
                    })
                    .and_then(|v| v.ok_or_else(|| "Missing session result".to_string()))
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.tasks.breaking_down = None;
                    // Deliberately not opening it: you asked for a breakdown
                    // while reading the task, and yanking you into a terminal
                    // loses the place you were reading. The launcher becomes
                    // the way in once the session appears.
                    if let Err(e) = result {
                        this.report_error(e, cx);
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// The form, shaped as the board's right-hand panel.
    ///
    /// It sits where a task's detail sits rather than replacing the whole
    /// view: filling it in is a small job, and taking the board away to do it
    /// hid the list you are adding to.
    pub(super) fn render_new_task_form(&self, share: f32, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let Some(form) = self.tasks.new_task.as_ref() else {
            return div().into_any_element();
        };

        let mut kinds = h_flex().gap(px(6.0)).flex_wrap();
        for kind in TaskKind::all() {
            let selected = form.kind == kind;
            kinds = kinds.child(
                div()
                    .id(SharedString::from(format!(
                        "new-task-kind-{}",
                        kind.wire_name()
                    )))
                    .cursor_pointer()
                    .px(px(12.0))
                    .py(px(5.0))
                    .rounded(px(4.0))
                    .when(selected, |d| {
                        d.bg(with_alpha(t.button_primary_bg, 0.2))
                            .text_color(rgb(t.text_primary))
                    })
                    .when(!selected, |d| {
                        d.bg(rgb(t.bg_secondary))
                            .text_color(rgb(t.text_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                    })
                    .text_size(ui_text_md(cx))
                    .child(kind.label())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            if let Some(form) = this.tasks.new_task.as_mut() {
                                form.kind = kind;
                            }
                            cx.notify();
                        }),
                    ),
            );
        }

        let mut body = v_flex()
            .id("new-task-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            // Matching the detail panel it stands in for, so the right-hand
            // column does not shift as you open and close the form.
            .gap(px(14.0))
            .px(px(16.0))
            .py(px(14.0))
            .child(
                v_flex()
                    .gap(px(4.0))
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(ui_text(15.0, cx))
                                    .text_color(rgb(t.text_primary))
                                    .child("New task"),
                            )
                            .child(self.small_button(
                                "new-task-cancel",
                                "Cancel",
                                cx.listener(move |this, _, _window, cx| {
                                    this.close_new_task(cx);
                                }),
                                cx,
                            )),
                    )
                    .child(self.task_form_hint(
                        "Fill in what you know. An agent can draft the rest \
                         from the title, or you can file it as it stands.",
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .gap(px(5.0))
                    .child(self.task_form_label("Title", cx))
                    .child(
                        okena_ui::input::input_container(&t, None)
                            .w_full()
                            .px(px(8.0))
                            .py(px(6.0))
                            .child(
                                SimpleInput::new(&self.tasks.new_task_title)
                                    .text_size(ui_text(13.0, cx)),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap(px(5.0))
                    .child(self.task_form_label("Description", cx))
                    .child(
                        // Shorter than the full-page form was: in a panel
                        // this shares the height with everything below it.
                        self.multiline_field("new-task-body", &self.tasks.new_task_body, 110.0, cx),
                    ),
            )
            .child(
                v_flex()
                    .gap(px(6.0))
                    .child(self.task_form_label("Kind", cx))
                    .child(kinds),
            );

        // Only where there is a choice: a single team is chosen rather than
        // presented.
        if form.containers.len() > 1 {
            let mut teams = h_flex().gap(px(6.0)).flex_wrap();
            for container in &form.containers {
                let selected = form.container_id.as_deref() == Some(container.id.as_str());
                let id = container.id.clone();
                let label = if container.key.is_empty() {
                    container.name.clone()
                } else {
                    format!("{} · {}", container.key, container.name)
                };
                teams = teams.child(
                    div()
                        .id(SharedString::from(format!(
                            "new-task-team-{}",
                            container.id
                        )))
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .when(selected, |d| {
                            d.bg(with_alpha(t.button_primary_bg, 0.2))
                                .text_color(rgb(t.text_primary))
                        })
                        .when(!selected, |d| {
                            d.bg(rgb(t.bg_secondary))
                                .text_color(rgb(t.text_secondary))
                                .hover(|s| s.bg(rgb(t.bg_hover)))
                        })
                        .text_size(ui_text_ms(cx))
                        .child(label)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                if let Some(form) = this.tasks.new_task.as_mut() {
                                    form.container_id = Some(id.clone());
                                }
                                cx.notify();
                            }),
                        ),
                );
            }
            body = body.child(
                v_flex()
                    .gap(px(6.0))
                    .child(self.task_form_label("Team", cx))
                    .child(teams),
            );
        }

        if let Some(err) = form.error.clone() {
            body = body.child(self.error_banner(err, cx));
        }

        let creating = form.creating;
        let drafting = form.drafting;
        // Two ways out of the form, and the launcher is the one that makes the
        // fields optional: "File it" takes what is typed, an agent takes the
        // title and works the rest out.
        let mut options =
            crate::views::agent_session::launch_options(self.tasks.default_agent.as_deref(), &t);
        options.push(crate::views::agent_session::no_agent_option("File it", &t));
        body = body.child(
            okena_ui::agent_launcher::AgentLauncher::new("new-task-launcher", "Draft the task")
                .subtitle("An agent writes the description and any sub-tasks")
                .options(options)
                .preferred(self.tasks.default_agent.clone())
                .busy(match (creating, drafting) {
                    (true, _) => Some("Creating…"),
                    (_, true) => Some("Starting…"),
                    _ => None,
                })
                .on_launch(
                    cx.listener(move |this, command: &SharedString, _window, cx| {
                        // The empty command is "File it": the same direct create
                        // the form always had.
                        if command.is_empty() {
                            this.submit_new_task(cx);
                        } else {
                            this.draft_task_with_agent(command.to_string(), cx);
                        }
                    }),
                ),
        );

        v_flex()
            .id("new-task-form")
            .w(relative(share))
            .min_w_0()
            .h_full()
            .border_l_1()
            .border_color(rgb(t.border))
            .child(body)
            .into_any_element()
    }

    fn task_form_label(&self, label: &'static str, cx: &App) -> AnyElement {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_secondary))
            .child(label)
            .into_any_element()
    }

    fn task_form_hint(&self, text: &'static str, cx: &App) -> AnyElement {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(text)
            .into_any_element()
    }

    /// Hand the form's fields to an agent and let it write the task.
    ///
    /// Only a title is required, and only because an agent with nothing to go
    /// on would be inventing the work rather than drafting it. Everything else
    /// is what the agent is for.
    pub(super) fn draft_task_with_agent(&mut self, agent: String, cx: &mut Context<Self>) {
        let Some(form) = self.tasks.new_task.as_ref() else {
            return;
        };
        if form.creating || form.drafting {
            return;
        }
        let title = self
            .tasks
            .new_task_title
            .read(cx)
            .value()
            .trim()
            .to_string();
        if title.is_empty() {
            if let Some(form) = self.tasks.new_task.as_mut() {
                form.error = Some("Give the agent a title to work from.".into());
            }
            cx.notify();
            return;
        }
        let kind = form.kind;
        let description = self.tasks.new_task_body.read(cx).value().trim().to_string();
        let container = form
            .container_id
            .as_deref()
            .and_then(|id| form.containers.iter().find(|c| c.id == id))
            .map(|c| {
                if c.key.is_empty() {
                    c.name.clone()
                } else {
                    format!("{} ({})", c.name, c.key)
                }
            })
            .unwrap_or_else(|| "your default team".to_string());

        let vars = BTreeMap::from([
            ("title".to_string(), title.clone()),
            ("kind".to_string(), kind.label().to_string()),
            ("container".to_string(), container),
            // No parent: this flow files a top-level task. A child is drafted
            // from its parent's own breakdown launcher, which knows the id.
            ("parent".to_string(), String::new()),
            (
                // Raw, even when empty: the `no-description-yet` partial words
                // the case where nothing has been written.
                "description".to_string(),
                description.clone(),
            ),
        ]);

        if let Some(form) = self.tasks.new_task.as_mut() {
            form.drafting = true;
            form.error = None;
        }
        cx.notify();

        let client = self.client.clone();
        let name = format!("Draft: {title}");
        let draft = title.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                let goal = render_brief(&client, "task-create", vars)?;
                client
                    .post_action(ActionRequest::AgentStartSession {
                        goal,
                        name,
                        root: String::new(),
                        project_ids: Vec::new(),
                        agent_command: Some(agent),
                        task_draft: Some(draft),
                        task: None,
                    })
                    .and_then(|v| v.ok_or_else(|| "Missing session result".to_string()))
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    match result {
                        // The form has done its job; the placeholder row in
                        // the list is where the work is visible from here.
                        Ok(_) => this.close_new_task(cx),
                        Err(e) => {
                            if let Some(form) = this.tasks.new_task.as_mut() {
                                form.drafting = false;
                                form.error = Some(e);
                            }
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
}

/// The kind a child of `parent` most likely is.
///
/// What a breakdown brief is filled from — the task, and the ids the MCP
/// tools take.
///
/// One function for both ways in, so the brief the dialog opens with is the
/// one a one-click start would have sent. The prose itself is the `break-down`
/// template; only the facts are assembled here.
pub(super) fn breakdown_vars(task: &Task) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("key".to_string(), task.display_key.clone()),
        // `okena_create_subtask` takes the provider's id, not the human key.
        ("parent_id".to_string(), task.id.external_id.clone()),
        ("title".to_string(), task.title.clone()),
        ("kind".to_string(), task.kind.label().to_string()),
        ("url".to_string(), task.url.clone()),
        (
            // Raw, even when empty: the template says "Description:" and the
            // `no-description` partial says what to when there is none.
            "description".to_string(),
            task.description
                .clone()
                .unwrap_or_default()
                .trim()
                .to_string(),
        ),
        (
            "child_kind".to_string(),
            narrower_than(task.kind).label().to_string(),
        ),
    ])
}

/// Session name for a breakdown of `task`.
pub(super) fn breakdown_name(task: &Task) -> String {
    format!("{} breakdown", task.display_key)
}

/// Ask the daemon to render a launch brief.
///
/// Blocking, to be called inside `smol::unblock` beside the action that sends
/// the result. An incomplete render — a template asking for something the flow
/// does not fill — is returned as an error rather than sent: a brief with a
/// visible `{placeholder}` in it reaches the agent as nonsense, and the person
/// who wrote the template is the one who can fix it.
pub(super) fn render_brief(
    client: &okena_transport::remote_action::RemoteActionClient,
    flow: &str,
    vars: BTreeMap<String, String>,
) -> Result<String, String> {
    let value = client
        .post_action(ActionRequest::PromptRender {
            flow: flow.to_string(),
            vars,
        })
        .and_then(|v| v.ok_or_else(|| format!("no `{flow}` prompt came back")))?;
    let unknown: Vec<String> = value
        .get("unknown")
        .and_then(|u| serde_json::from_value(u.clone()).ok())
        .unwrap_or_default();
    if !unknown.is_empty() {
        return Err(format!(
            "the `{flow}` template asks for {} that okena does not fill here",
            unknown
                .iter()
                .map(|u| format!("`{{{u}}}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    value
        .get("text")
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("the `{flow}` prompt came back empty"))
}

/// One step down the breakdown, because that is what breaking something down
/// means. A task's children are tasks — there is nothing narrower — and a
/// defect's children are tasks rather than more defects.
pub(super) fn narrower_than(parent: TaskKind) -> TaskKind {
    match parent {
        TaskKind::Epic => TaskKind::Feature,
        TaskKind::Feature => TaskKind::Story,
        TaskKind::Story | TaskKind::Task | TaskKind::Defect => TaskKind::Task,
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::narrower_than;
    use okena_core::tasks::TaskKind;

    #[test]
    fn a_child_defaults_one_step_down_the_breakdown() {
        assert_eq!(narrower_than(TaskKind::Epic), TaskKind::Feature);
        assert_eq!(narrower_than(TaskKind::Feature), TaskKind::Story);
        assert_eq!(narrower_than(TaskKind::Story), TaskKind::Task);
    }

    #[test]
    fn the_narrowest_kinds_stay_where_they_are() {
        // There is nothing below a task, and a defect's children are work to
        // do rather than more defects.
        assert_eq!(narrower_than(TaskKind::Task), TaskKind::Task);
        assert_eq!(narrower_than(TaskKind::Defect), TaskKind::Task);
    }
}
