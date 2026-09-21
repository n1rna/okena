//! Adding, renaming, re-scoping and deleting a space.
//!
//! One dialog rather than four, because three of them share the same hard
//! part: choosing which task backend connection a space reads and which of
//! that connection's groupings, labels and statuses it is scoped to. Splitting
//! them would mean writing that picker twice.
//!
//! Every change goes to the daemon, which owns both `settings.json` (the
//! spaces) and `workspace.json` (what is in them). The dialog never edits
//! either — it asks, and the answer comes back on the next snapshot.

use crate::keybindings::Cancel;
use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms};
use crate::views::components::SimpleInput;
use crate::views::harness::task_filter::{
    Facets, LABELS_HEADING, STATUS_HEADING, collect_facets,
};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::spaces::SpaceData;
use okena_core::tasks::{Task, TaskScope};
use okena_core::connections::{Connection, kind_display_name};
use okena_ui::modal::{modal_backdrop, modal_content, modal_header};
use okena_ui::simple_input::SimpleInputState;
use okena_transport::remote_action::RemoteActionClient;

/// Which job the dialog is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpaceDialogMode {
    /// Add a space: a name, a connection and its filters.
    Add,
    /// Rename an existing space. Never offered on Default.
    Rename { space: SpaceData },
    /// Change a space's connection and filters.
    EditTasks { space: SpaceData },
    /// Confirm deleting a space, after naming what is in it.
    Delete { space: SpaceData },
}

impl SpaceDialogMode {
    fn title(&self) -> &'static str {
        match self {
            Self::Add => "Add a space",
            Self::Rename { .. } => "Rename space",
            Self::EditTasks { .. } => "Tasks in this space",
            Self::Delete { .. } => "Delete space",
        }
    }

    fn subtitle(&self) -> &'static str {
        match self {
            Self::Add => {
                "A space has its own projects, agents, tasks and roots. \
                 Nothing in it shows anywhere else."
            }
            Self::Rename { .. } => "What this space is called on its dot and in the menu.",
            Self::EditTasks { .. } => {
                "One connection, and the filters it is limited to. The Tasks \
                 filter bar can narrow inside them but cannot reach past them."
            }
            Self::Delete { .. } => {
                "The projects and agents below are removed from okena and \
                 their agents stop. Files on disk are not touched."
            }
        }
    }

    fn space(&self) -> Option<&SpaceData> {
        match self {
            Self::Add => None,
            Self::Rename { space } | Self::EditTasks { space } | Self::Delete { space } => {
                Some(space)
            }
        }
    }

    fn confirm_label(&self) -> &'static str {
        match self {
            Self::Add => "Add space",
            Self::Rename { .. } => "Rename",
            Self::EditTasks { .. } => "Save",
            Self::Delete { .. } => "Delete",
        }
    }

    /// Whether this mode picks a connection and filters.
    fn picks_tasks(&self) -> bool {
        matches!(self, Self::Add | Self::EditTasks { .. })
    }
}

pub enum SpaceDialogEvent {
    Close,
}

/// What the form submits, given what is in it.
///
/// Free of the view so the rules — a space needs a name, editing tasks does
/// not, a delete needs nothing but the id — are testable without a window.
fn submission_for(
    mode: &SpaceDialogMode,
    name: &str,
    connection: Option<&str>,
    scope: &TaskScope,
) -> Result<ActionRequest, String> {
    let name = name.trim().to_string();
    let connection = connection
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string);
    match mode {
        SpaceDialogMode::Add => {
            if name.is_empty() {
                return Err("A space needs a name.".into());
            }
            Ok(ActionRequest::SpaceCreate {
                name,
                connection,
                tasks: scope.clone(),
            })
        }
        SpaceDialogMode::Rename { space } => {
            if name.is_empty() {
                return Err("A space needs a name.".into());
            }
            Ok(ActionRequest::SpaceRename {
                space_id: space.id.clone(),
                name,
            })
        }
        SpaceDialogMode::EditTasks { space } => Ok(ActionRequest::SpaceSetConnection {
            space_id: space.id.clone(),
            connection,
        }),
        SpaceDialogMode::Delete { space } => Ok(ActionRequest::SpaceDelete {
            space_id: space.id.clone(),
        }),
    }
}

/// What a delete will remove, as the daemon reports it.
#[derive(Clone, Debug, Default, serde::Deserialize)]
struct SpaceContents {
    #[serde(default)]
    projects: Vec<String>,
    #[serde(default)]
    agents: Vec<String>,
}

pub struct SpaceDialog {
    focus_handle: FocusHandle,
    client: RemoteActionClient,
    mode: SpaceDialogMode,
    name: Entity<SimpleInputState>,
    /// A name for a connection being added from here.
    new_connection_name: Entity<SimpleInputState>,
    connections: Vec<Connection>,
    /// The connection the space will read. `None` until one is chosen, which
    /// for Add is "no task backend yet".
    connection: Option<String>,
    /// The filters, as the form has them.
    scope: TaskScope,
    /// What the chosen connection can be filtered by, read from its own tasks.
    facets: Facets,
    facets_for: Option<String>,
    loading_facets: bool,
    contents: Option<SpaceContents>,
    busy: bool,
    error: Option<String>,
}

impl EventEmitter<SpaceDialogEvent> for SpaceDialog {}

impl SpaceDialog {
    pub fn new(client: RemoteActionClient, mode: SpaceDialogMode, cx: &mut Context<Self>) -> Self {
        let existing = mode.space().cloned();
        let name = cx.new(|cx| SimpleInputState::new(cx).placeholder("Client A"));
        if let Some(space) = &existing {
            let value = space.name.clone();
            name.update(cx, |input, cx| input.set_value(&value, cx));
        }
        let new_connection_name =
            cx.new(|cx| SimpleInputState::new(cx).placeholder("Acme Linear"));

        let mut dialog = Self {
            focus_handle: cx.focus_handle(),
            client,
            connection: existing.as_ref().and_then(|s| s.connection.clone()),
            scope: existing.as_ref().map(|s| s.tasks.clone()).unwrap_or_default(),
            mode,
            name,
            new_connection_name,
            connections: Vec::new(),
            facets: Facets::default(),
            facets_for: None,
            loading_facets: false,
            contents: None,
            busy: false,
            error: None,
        };
        dialog.load_connections(cx);
        if matches!(dialog.mode, SpaceDialogMode::Delete { .. }) {
            dialog.load_contents(cx);
        }
        dialog
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(SpaceDialogEvent::Close);
    }

    fn handle_cancel(&mut self, _: &Cancel, _window: &mut Window, cx: &mut Context<Self>) {
        self.close(cx);
    }

    fn load_connections(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::TaskConnections)
                    .map(|v| {
                        v.and_then(|v| serde_json::from_value::<Vec<Connection>>(v).ok())
                            .unwrap_or_default()
                    })
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(list) => {
                            // A space with no connection yet takes the first
                            // one going, which is what "reuse an existing
                            // connection" means when there is only one.
                            if this.connection.is_none()
                                && matches!(this.mode, SpaceDialogMode::Add)
                            {
                                this.connection = list.first().map(|c| c.id.clone());
                            }
                            this.connections = list;
                            this.load_facets(cx);
                        }
                        Err(e) => this.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Read the chosen connection's tasks to learn what it can be filtered by.
    ///
    /// Unscoped on purpose: the form offers everything the connection has, and
    /// what the user picks becomes the scope. Asking with the scope already
    /// applied would make a filter impossible to widen once set.
    fn load_facets(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self.connection.clone() else {
            self.facets = Facets::default();
            self.facets_for = None;
            return;
        };
        if self.facets_for.as_deref() == Some(provider.as_str()) || !self.mode.picks_tasks() {
            return;
        }
        self.loading_facets = true;
        self.facets_for = Some(provider.clone());
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::TasksList {
                        provider,
                        scope: TaskScope::default(),
                    })
                    .map(|v| {
                        v.and_then(|v| serde_json::from_value::<Vec<Task>>(v["tasks"].clone()).ok())
                            .unwrap_or_default()
                    })
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.loading_facets = false;
                    match result {
                        Ok(tasks) => this.facets = collect_facets(&tasks),
                        // Not an error the user has to act on: a connection
                        // that is not signed in yet simply offers no filters.
                        Err(_) => this.facets = Facets::default(),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn load_contents(&mut self, cx: &mut Context<Self>) {
        let Some(space_id) = self.mode.space().map(|s| s.id.clone()) else {
            return;
        };
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::SpaceContents { space_id })
                    .map(|v| {
                        v.and_then(|v| serde_json::from_value::<SpaceContents>(v).ok())
                            .unwrap_or_default()
                    })
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(c) => this.contents = Some(c),
                        Err(e) => this.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Add a connection from inside the form, so a space can be created
    /// against a second account without a trip to Settings first.
    fn add_connection(&mut self, kind: &'static str, cx: &mut Context<Self>) {
        let typed = self
            .new_connection_name
            .read(cx)
            .value()
            .trim()
            .to_string();
        let name = if typed.is_empty() {
            kind_display_name(kind)
        } else {
            typed
        };
        let client = self.client.clone();
        self.busy = true;
        self.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client.post_action(ActionRequest::TaskConnectionAdd {
                    kind: kind.to_string(),
                    name,
                })
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.busy = false;
                    match result {
                        Ok(v) => {
                            if let Some(added) = v
                                .and_then(|v| serde_json::from_value::<Connection>(v).ok())
                            {
                                // Selected straight away: adding one from here
                                // is how you say which one the space reads.
                                this.connection = Some(added.id.clone());
                                this.scope = TaskScope::default();
                                this.new_connection_name
                                    .update(cx, |i, cx| i.set_value("", cx));
                            }
                            this.load_connections(cx);
                        }
                        Err(e) => this.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// The action this form submits, or the reason it cannot.
    fn submission(&self, cx: &App) -> Result<ActionRequest, String> {
        submission_for(
            &self.mode,
            self.name.read(cx).value(),
            self.connection.as_deref(),
            &self.scope,
        )
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let action = match self.submission(cx) {
            Ok(a) => a,
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return;
            }
        };
        // Editing tasks is two changes — the connection and the filters — and
        // both must land, so the filters follow the connection.
        let follow_up = match &self.mode {
            SpaceDialogMode::EditTasks { space } => Some(ActionRequest::SpaceSetFilters {
                space_id: space.id.clone(),
                tasks: self.scope.clone(),
            }),
            _ => None,
        };
        let client = self.client.clone();
        self.busy = true;
        self.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client.post_action(action)?;
                if let Some(next) = follow_up {
                    client.post_action(next)?;
                }
                Ok::<(), String>(())
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.busy = false;
                    match result {
                        Ok(()) => this.close(cx),
                        Err(e) => this.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
}

impl Focusable for SpaceDialog {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SpaceDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let mode = self.mode.clone();

        let mut body = v_flex().gap(px(14.0)).p(px(16.0));

        if matches!(mode, SpaceDialogMode::Add | SpaceDialogMode::Rename { .. }) {
            body = body.child(self.render_field(
                "Name",
                okena_ui::input::input_container(&t, None)
                    .w_full()
                    .px(px(8.0))
                    .py(px(5.0))
                    .child(SimpleInput::new(&self.name).text_size(ui_text(13.0, cx)))
                    .into_any_element(),
                cx,
            ));
        }

        if mode.picks_tasks() {
            body = body.child(self.render_connection_picker(cx));
            body = body.child(self.render_filters(cx));
        }

        if let SpaceDialogMode::Delete { space } = &mode {
            body = body.child(self.render_contents(space, cx));
        }

        if let Some(error) = self.error.clone() {
            body = body.child(
                div()
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(4.0))
                    .bg(with_alpha(t.error, 0.1))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.error))
                    .child(error),
            );
        }

        let destructive = matches!(mode, SpaceDialogMode::Delete { .. });
        let confirm_label = if self.busy {
            "Working…"
        } else {
            mode.confirm_label()
        };

        modal_backdrop("space-dialog-backdrop", &t)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.close(cx)),
            )
            .child(
                modal_content("space-dialog", &t)
                    .w(px(520.0))
                    .max_h(px(640.0))
                    .track_focus(&self.focus_handle)
                    .key_context("SpaceDialog")
                    .on_action(cx.listener(Self::handle_cancel))
                    // The backdrop closes; the panel must not.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(modal_header(
                        mode.title(),
                        Some(mode.subtitle()),
                        &t,
                        cx,
                        cx.listener(|this, _, _window, cx| this.close(cx)),
                    ))
                    .child(
                        div()
                            .id("space-dialog-body")
                            .flex_1()
                            .overflow_y_scroll()
                            .child(body),
                    )
                    .child(
                        h_flex()
                            .justify_end()
                            .gap(px(8.0))
                            .px(px(16.0))
                            .py(px(12.0))
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .child(
                                div()
                                    .id("space-dialog-cancel")
                                    .cursor_pointer()
                                    .px(px(12.0))
                                    .py(px(6.0))
                                    .rounded(px(4.0))
                                    .border_1()
                                    .border_color(rgb(t.border))
                                    .text_size(ui_text_md(cx))
                                    .text_color(rgb(t.text_secondary))
                                    .child("Cancel")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _window, cx| this.close(cx)),
                                    ),
                            )
                            .child(
                                div()
                                    .id("space-dialog-confirm")
                                    .cursor_pointer()
                                    .px(px(14.0))
                                    .py(px(6.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(if destructive {
                                        t.error
                                    } else {
                                        t.button_primary_bg
                                    }))
                                    .text_size(ui_text_md(cx))
                                    .text_color(rgb(t.button_primary_fg))
                                    .child(confirm_label)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _window, cx| this.submit(cx)),
                                    ),
                            ),
                    ),
            )
    }
}

impl SpaceDialog {
    fn render_field(
        &self,
        label: &'static str,
        control: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        v_flex()
            .gap(px(4.0))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(label),
            )
            .child(control)
            .into_any_element()
    }

    /// Reuse an existing connection, or add a new one.
    ///
    /// Several spaces may share one — that is the normal case for two views of
    /// the same account — so an already-used connection is offered like any
    /// other rather than hidden.
    fn render_connection_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let active = self.connection.clone();
        let chips: Vec<AnyElement> = self
            .connections
            .iter()
            .map(|c| {
                let id = c.id.clone();
                let selected = active.as_deref() == Some(id.as_str());
                div()
                    .id(SharedString::from(format!("space-conn-{id}")))
                    .cursor_pointer()
                    .px(px(10.0))
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(rgb(if selected { t.border_active } else { t.border }))
                    .when(selected, |d| d.bg(with_alpha(t.button_primary_bg, 0.15)))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(if selected {
                        t.text_primary
                    } else {
                        t.text_secondary
                    }))
                    .child(c.name.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            if this.connection.as_deref() == Some(id.as_str()) {
                                return;
                            }
                            this.connection = Some(id.clone());
                            // Another connection's ids mean nothing here.
                            this.scope = TaskScope::default();
                            this.load_facets(cx);
                            cx.notify();
                        }),
                    )
                    .into_any_element()
            })
            .collect();

        let add = |kind: &'static str, label: &'static str, cx: &mut Context<Self>| {
            div()
                .id(SharedString::from(format!("space-add-conn-{kind}")))
                .cursor_pointer()
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(rgb(t.border))
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_secondary))
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| this.add_connection(kind, cx)),
                )
        };

        v_flex()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child("Task backend connection"),
            )
            .when(chips.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child("No connections yet — add one below."),
                )
            })
            .child(h_flex().gap(px(6.0)).flex_wrap().children(chips))
            .child(
                h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .child(
                        okena_ui::input::input_container(&t, None)
                            .flex_1()
                            .min_w_0()
                            .px(px(8.0))
                            .py(px(4.0))
                            .child(
                                SimpleInput::new(&self.new_connection_name)
                                    .text_size(ui_text(13.0, cx)),
                            ),
                    )
                    .child(add("linear", "Add Linear", cx))
                    .child(add(
                        crate::views::harness::AZURE_DEVOPS,
                        "Add Azure DevOps",
                        cx,
                    )),
            )
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(
                        "A new connection is signed in on its row in Settings → Tasks.",
                    ),
            )
            .into_any_element()
    }

    /// The filters this space is scoped to, offered from what the connection
    /// actually reports: its groupings, its labels and its statuses.
    fn render_filters(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        if self.loading_facets {
            return div()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child("Reading what this connection can be filtered by…")
                .into_any_element();
        }
        if self.facets.is_empty() {
            return div()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(
                    "This connection reports nothing to filter on yet. The space \
                     will show all of its tasks.",
                )
                .into_any_element();
        }

        let mut sections = v_flex().gap(px(10.0));
        for (axis, values) in self.facets.axes.clone() {
            let heading = axis.label().to_string();
            let chips: Vec<AnyElement> = values
                .iter()
                .map(|v| {
                    let selected = self.scope.group_selected(&axis, &v.id);
                    let axis = axis.clone();
                    let id = v.id.clone();
                    self.facet_chip(
                        format!("facet-{}-{}", axis.wire_name(), v.id),
                        &v.name,
                        selected,
                        cx,
                        move |this, cx| {
                            this.scope.toggle_group(axis.clone(), &id);
                            cx.notify();
                        },
                    )
                })
                .collect();
            sections = sections.child(self.facet_section(&heading, chips, cx));
        }
        if !self.facets.labels.is_empty() {
            let chips: Vec<AnyElement> = self
                .facets
                .labels
                .iter()
                .map(|v| {
                    let selected = self.scope.labels.contains(&v.id);
                    let id = v.id.clone();
                    self.facet_chip(
                        format!("facet-label-{}", v.id),
                        &v.name,
                        selected,
                        cx,
                        move |this, cx| {
                            this.scope.toggle_label(&id);
                            cx.notify();
                        },
                    )
                })
                .collect();
            sections = sections.child(self.facet_section(LABELS_HEADING, chips, cx));
        }
        if !self.facets.statuses.is_empty() {
            let chips: Vec<AnyElement> = self
                .facets
                .statuses
                .iter()
                .map(|v| {
                    let selected = self.scope.statuses.contains(&v.id);
                    let id = v.id.clone();
                    self.facet_chip(
                        format!("facet-status-{}", v.id),
                        &v.name,
                        selected,
                        cx,
                        move |this, cx| {
                            this.scope.toggle_status(&id);
                            cx.notify();
                        },
                    )
                })
                .collect();
            sections = sections.child(self.facet_section(STATUS_HEADING, chips, cx));
        }

        v_flex()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(if self.scope.is_empty() {
                        "Filters — none, so the space shows every task on this connection"
                            .to_string()
                    } else {
                        format!("Filters — {} selected", self.scope.selected_count())
                    }),
            )
            .child(sections)
            .into_any_element()
    }

    fn facet_section(
        &self,
        heading: &str,
        chips: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        v_flex()
            .gap(px(4.0))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(heading.to_uppercase()),
            )
            .child(h_flex().gap(px(5.0)).flex_wrap().children(chips))
            .into_any_element()
    }

    fn facet_chip(
        &self,
        id: String,
        label: &str,
        selected: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let t = theme(cx);
        div()
            .id(SharedString::from(id))
            .cursor_pointer()
            .px(px(8.0))
            .py(px(2.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(rgb(if selected { t.border_active } else { t.border }))
            .when(selected, |d| d.bg(with_alpha(t.button_primary_bg, 0.15)))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(if selected {
                t.text_primary
            } else {
                t.text_secondary
            }))
            .child(label.to_string())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| on_click(this, cx)),
            )
            .into_any_element()
    }

    /// What the delete will remove, named.
    fn render_contents(&self, space: &SpaceData, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let Some(held) = self.contents.clone() else {
            return div()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child("Reading what is in this space…")
                .into_any_element();
        };
        let mut body = v_flex().gap(px(8.0)).child(
            div()
                .text_size(ui_text(13.0, cx))
                .text_color(rgb(t.text_primary))
                .child(format!("Delete “{}”?", space.name)),
        );
        if held.projects.is_empty() && held.agents.is_empty() {
            body = body.child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child("It holds no projects or agents."),
            );
        }
        for (heading, names) in [("Projects", held.projects), ("Agents", held.agents)] {
            if names.is_empty() {
                continue;
            }
            body = body.child(
                v_flex()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(heading),
                    )
                    .children(names.into_iter().map(|n| {
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_secondary))
                            .child(n)
                    })),
            );
        }
        body.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{SpaceDialogMode, submission_for};
    use okena_core::api::ActionRequest;
    use okena_core::spaces::SpaceData;
    use okena_core::tasks::{GroupAxis, TaskScope};

    fn space(id: &str) -> SpaceData {
        SpaceData::new(id, id)
    }

    #[test]
    fn adding_a_space_carries_its_name_connection_and_filters() {
        let mut scope = TaskScope::default();
        scope.toggle_group(GroupAxis::Project, "alpha");
        let action = submission_for(
            &SpaceDialogMode::Add,
            "  Client A  ",
            Some("linear-2"),
            &scope,
        )
        .expect("submits");
        match action {
            ActionRequest::SpaceCreate {
                name,
                connection,
                tasks,
            } => {
                assert_eq!(name, "Client A");
                assert_eq!(connection.as_deref(), Some("linear-2"));
                assert_eq!(tasks, scope);
            }
            other => panic!("expected a create, got {other:?}"),
        }
    }

    #[test]
    fn a_space_with_no_connection_chosen_is_added_without_one() {
        let action =
            submission_for(&SpaceDialogMode::Add, "Client A", None, &TaskScope::default())
                .expect("submits");
        assert!(
            matches!(action, ActionRequest::SpaceCreate { connection: None, .. }),
            "{action:?}"
        );
        // A blank id counts as none rather than as a connection called "".
        let action =
            submission_for(&SpaceDialogMode::Add, "Client A", Some("  "), &TaskScope::default())
                .expect("submits");
        assert!(
            matches!(action, ActionRequest::SpaceCreate { connection: None, .. }),
            "{action:?}"
        );
    }

    #[test]
    fn a_nameless_space_is_refused_before_it_reaches_the_daemon() {
        for mode in [
            SpaceDialogMode::Add,
            SpaceDialogMode::Rename {
                space: space("client-a"),
            },
        ] {
            let e = submission_for(&mode, "   ", None, &TaskScope::default())
                .expect_err("refused");
            assert!(e.contains("needs a name"), "{e}");
        }
    }

    #[test]
    fn renaming_carries_the_id_and_the_new_name() {
        let action = submission_for(
            &SpaceDialogMode::Rename {
                space: space("client-a"),
            },
            "Acme",
            None,
            &TaskScope::default(),
        )
        .expect("submits");
        match action {
            ActionRequest::SpaceRename { space_id, name } => {
                assert_eq!(space_id, "client-a");
                assert_eq!(name, "Acme");
            }
            other => panic!("expected a rename, got {other:?}"),
        }
    }

    #[test]
    fn editing_tasks_needs_no_name() {
        // The form does not show a name field in this mode, so an empty one
        // must not block the save.
        let action = submission_for(
            &SpaceDialogMode::EditTasks {
                space: space("client-a"),
            },
            "",
            Some("linear"),
            &TaskScope::default(),
        )
        .expect("submits");
        assert!(
            matches!(
                action,
                ActionRequest::SpaceSetConnection { ref space_id, ref connection }
                    if space_id == "client-a" && connection.as_deref() == Some("linear")
            ),
            "{action:?}"
        );
    }

    #[test]
    fn deleting_asks_for_nothing_but_the_space() {
        let action = submission_for(
            &SpaceDialogMode::Delete {
                space: space("client-a"),
            },
            "",
            None,
            &TaskScope::default(),
        )
        .expect("submits");
        assert!(
            matches!(action, ActionRequest::SpaceDelete { ref space_id } if space_id == "client-a"),
            "{action:?}"
        );
    }

    #[test]
    fn every_mode_says_what_its_button_does() {
        // A confirm button reading "Add space" on a delete would be a very
        // expensive typo.
        assert_eq!(SpaceDialogMode::Add.confirm_label(), "Add space");
        assert_eq!(
            SpaceDialogMode::Delete {
                space: space("client-a")
            }
            .confirm_label(),
            "Delete"
        );
        assert!(SpaceDialogMode::Add.picks_tasks());
        assert!(
            !SpaceDialogMode::Rename {
                space: space("client-a")
            }
            .picks_tasks(),
            "renaming does not touch the task backend"
        );
    }
}
