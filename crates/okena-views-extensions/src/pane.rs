//! An extension's own view: what it last returned, drawn natively, with its
//! actions.
//!
//! The pane holds only presentation state — how tables are arranged, which
//! tree items are open, the form of an action in progress. The extension's
//! data comes from the daemon's snapshot through [`ExtensionsState`], so a
//! refresh re-renders without losing any of it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::extension::{
    ApiExtension, ExtActionDef, ExtActionOutcome, ExtInput, ExtNode, ExtRunState, InputKind,
};
use okena_ui::button::{button, button_primary};
use okena_ui::modal::{modal_backdrop, modal_content};
use okena_ui::simple_input::{InputChangedEvent, SimpleInputState};
use okena_ui::theme::{ThemeColors, theme};
use okena_ui::tokens::{ui_text, ui_text_md, ui_text_ms, ui_text_sm};
use okena_workspace::extensions_state::{
    ClientExtension, ExtensionsState, extensions_entity, split_key,
};
use okena_workspace::request_broker::RequestBroker;
use okena_workspace::requests::WorkbenchRequest;
use okena_workspace::toast::ToastManager;

use crate::render;
use crate::table::TableState;

/// What happens around the action the user clicked.
pub(crate) struct PendingAction {
    pub action: ExtActionDef,
    pub items: Vec<String>,
    pub fields: Vec<(ExtInput, Field)>,
    /// The form was filled (or there was none); a destructive action asks here.
    pub confirming: bool,
}

pub(crate) enum Field {
    Text(Entity<SimpleInputState>),
    Toggle(bool),
    Select(usize),
}

pub(crate) struct Running {
    pub label: String,
    pub items: usize,
}

/// How far a tree has been opened by the user.
#[derive(Default)]
pub(crate) struct TreeState {
    /// Items the user toggled away from the extension's own `expanded`.
    pub toggled: HashMap<String, bool>,
    pub selected: Option<String>,
}

/// Something the pane asks the app to do that the pane cannot do alone.
pub enum ExtensionPaneEvent {
    /// Launch an agent session from an action's outcome.
    AgentLaunch {
        extension: Arc<ClientExtension>,
        outcome: Box<ExtActionOutcome>,
    },
    /// Open an agent session the extension started, by its project id.
    OpenSession { project_id: String },
}

impl EventEmitter<ExtensionPaneEvent> for ExtensionPane {}

pub struct ExtensionPane {
    pub(crate) key: String,
    request_broker: Entity<RequestBroker>,
    pub(crate) tables: HashMap<String, TableState>,
    pub(crate) filters: HashMap<String, Entity<SimpleInputState>>,
    pub(crate) trees: HashMap<String, TreeState>,
    /// Sections the user collapsed or opened, by node index.
    pub(crate) sections: HashMap<u32, bool>,
    pub(crate) pending: Option<PendingAction>,
    pub(crate) running: Option<Running>,
    pub(crate) last_result: Option<(bool, String)>,
    /// Refusals already dismissed, by timestamp.
    dismissed_refusals: HashSet<u64>,
    /// Badges for the items agent sessions were started from.
    pub(crate) agent_badges: crate::AgentBadgesFn,
    _extensions: Option<Subscription>,
}

impl ExtensionPane {
    pub fn new(
        key: String,
        request_broker: Entity<RequestBroker>,
        agent_badges: crate::AgentBadgesFn,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = extensions_entity(cx).map(|entity| {
            cx.observe(&entity, |this: &mut Self, _, cx| {
                this.on_extensions_changed(cx);
                cx.notify();
            })
        });
        Self {
            key,
            request_broker,
            tables: HashMap::new(),
            filters: HashMap::new(),
            trees: HashMap::new(),
            sections: HashMap::new(),
            pending: None,
            running: None,
            last_result: None,
            dismissed_refusals: HashSet::new(),
            agent_badges,
            _extensions: subscription,
        }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub(crate) fn extension(&self, cx: &App) -> Option<Arc<ClientExtension>> {
        extensions_entity(cx).and_then(|e| e.read(cx).get(&self.key))
    }

    /// Keeps each table's selection to rows that still exist.
    fn on_extensions_changed(&mut self, cx: &mut Context<Self>) {
        let Some(ext) = self.extension(cx) else {
            return;
        };
        let Some(view) = &ext.ext.view else {
            return;
        };
        for node in &view.nodes {
            if let ExtNode::Table { table } = node
                && let Some(state) = self.tables.get_mut(&table.id)
            {
                state.retain_rows(table);
            }
        }
    }

    /// The filter box of the table `id`, made on first use.
    pub(crate) fn filter_input(
        &mut self,
        id: &str,
        placeholder: &str,
        cx: &mut Context<Self>,
    ) -> Entity<SimpleInputState> {
        if let Some(input) = self.filters.get(id) {
            return input.clone();
        }
        let placeholder = placeholder.to_string();
        let input = cx.new(|cx| SimpleInputState::new(cx).placeholder(placeholder));
        let table_id = id.to_string();
        cx.subscribe(&input, move |this: &mut Self, input, _: &InputChangedEvent, cx| {
            let value = input.read(cx).value().to_string();
            this.tables.entry(table_id.clone()).or_default().filter = value;
            cx.notify();
        })
        .detach();
        self.filters.insert(id.to_string(), input.clone());
        input
    }

    // ─── Commands to the daemon ─────────────────────────────────────────────

    fn post(
        &self,
        action: ActionRequest,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Result<Option<serde_json::Value>, String>, &mut Context<Self>) + 'static,
    ) {
        let client = split_key(&self.key)
            .and_then(|(connection, _)| extensions_entity(cx)?.read(cx).client(connection));
        cx.spawn(async move |this, cx| {
            let result = match client {
                Some(client) => smol::unblock(move || client.post_action(action)).await,
                None => Err("the daemon running this extension is not connected".to_string()),
            };
            let _ = this.update(cx, |this, cx| {
                done(this, result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(ext) = self.extension(cx) else { return };
        self.post(ActionRequest::ExtensionRefresh { id: ext.ext.id.clone() }, cx, |_, result, cx| {
            if let Err(e) = result {
                ToastManager::error(format!("Refresh failed: {e}"), cx);
            }
        });
    }

    pub(crate) fn recheck(&mut self, cx: &mut Context<Self>) {
        let Some(ext) = self.extension(cx) else { return };
        self.post(ActionRequest::ExtensionRecheck { id: ext.ext.id.clone() }, cx, |_, result, cx| {
            if let Err(e) = result {
                ToastManager::error(format!("Re-check failed: {e}"), cx);
            }
        });
    }

    /// The Extensions page, with this extension's details open.
    pub(crate) fn open_settings(&mut self, cx: &mut Context<Self>) {
        let key = self.key.clone();
        self.request_broker.update(cx, |broker, cx| {
            broker.push_workbench_request(WorkbenchRequest::OpenExtensionsPage { open: Some(key) }, cx);
        });
    }

    pub(crate) fn open_session(&mut self, project_id: String, cx: &mut Context<Self>) {
        cx.emit(ExtensionPaneEvent::OpenSession { project_id });
    }

    /// The user clicked an action: ask for its inputs and, if it is
    /// destructive, for confirmation, then run it.
    pub(crate) fn start_action(&mut self, action_id: &str, items: Vec<String>, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        let Some(ext) = self.extension(cx) else { return };
        let Some(action) = ext.ext.action(action_id).cloned() else {
            ToastManager::error(format!("{} has no action `{action_id}`", ext.ext.name), cx);
            return;
        };
        let fields = action
            .inputs
            .iter()
            .map(|input| {
                let field = match input.kind {
                    InputKind::Toggle => Field::Toggle(input.default.as_deref() == Some("true")),
                    InputKind::Select => Field::Select(
                        input
                            .default
                            .as_ref()
                            .and_then(|d| input.options.iter().position(|o| o == d))
                            .unwrap_or(0),
                    ),
                    _ => {
                        let default = input.default.clone().unwrap_or_default();
                        let placeholder = input.placeholder.clone().unwrap_or_default();
                        let multiline = input.kind == InputKind::Multiline;
                        Field::Text(cx.new(|cx| {
                            let state = SimpleInputState::new(cx)
                                .placeholder(placeholder)
                                .default_value(default);
                            if multiline { state.multiline() } else { state }
                        }))
                    }
                };
                (input.clone(), field)
            })
            .collect::<Vec<_>>();
        if fields.is_empty() && !action.destructive {
            self.run(action, items, Vec::new(), cx);
        } else {
            self.pending = Some(PendingAction {
                confirming: fields.is_empty(),
                action,
                items,
                fields,
            });
            cx.notify();
        }
    }

    pub(crate) fn cancel_pending(&mut self, cx: &mut Context<Self>) {
        self.pending = None;
        cx.notify();
    }

    /// The form's Run (or the confirmation's): check required fields, then
    /// confirm a destructive action, then run.
    pub(crate) fn submit_pending(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending.as_mut() else { return };
        let mut inputs = Vec::new();
        for (input, field) in &pending.fields {
            let value = match field {
                Field::Text(state) => state.read(cx).value().trim().to_string(),
                Field::Toggle(on) => on.to_string(),
                Field::Select(i) => input.options.get(*i).cloned().unwrap_or_default(),
            };
            if input.required && value.is_empty() {
                ToastManager::warning(format!("{} is required", input.label), cx);
                return;
            }
            if input.kind == InputKind::Number && !value.is_empty() && value.parse::<f64>().is_err() {
                ToastManager::warning(format!("{} must be a number", input.label), cx);
                return;
            }
            inputs.push((input.key.clone(), value));
        }
        if pending.action.destructive && !pending.confirming {
            pending.confirming = true;
            cx.notify();
            return;
        }
        let Some(pending) = self.pending.take() else { return };
        self.run(pending.action, pending.items, inputs, cx);
    }

    fn run(
        &mut self,
        action: ExtActionDef,
        items: Vec<String>,
        inputs: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        let Some(ext) = self.extension(cx) else { return };
        self.running = Some(Running {
            label: action.label.clone(),
            items: items.len(),
        });
        self.last_result = None;
        cx.notify();
        let request = ActionRequest::ExtensionRunAction {
            id: ext.ext.id.clone(),
            action_id: action.id.clone(),
            items,
            inputs,
        };
        self.post(request, cx, move |this, result, cx| {
            this.running = None;
            let outcome = result.and_then(|value| {
                serde_json::from_value::<ExtActionOutcome>(value.unwrap_or_default())
                    .map_err(|e| format!("unexpected reply: {e}"))
            });
            match outcome {
                Ok(outcome) => {
                    let message = outcome
                        .message
                        .clone()
                        .unwrap_or_else(|| format!("{} finished", action.label));
                    if outcome.agent.is_some() {
                        cx.emit(ExtensionPaneEvent::AgentLaunch {
                            extension: ext.clone(),
                            outcome: Box::new(outcome.clone()),
                        });
                    }
                    if outcome.message.is_some() || outcome.agent.is_none() {
                        ToastManager::success(message.clone(), cx);
                        this.last_result = Some((true, message));
                    }
                }
                Err(e) => {
                    let message = format!("{} failed: {e}", action.label);
                    ToastManager::error(message.clone(), cx);
                    this.last_result = Some((false, message));
                }
            }
        });
    }

    /// The user's answer to an agent's destructive call.
    pub(crate) fn answer(&mut self, confirmation: &str, approve: bool, cx: &mut Context<Self>) {
        let Some(ext) = self.extension(cx) else { return };
        let request = ActionRequest::ExtensionConfirm {
            id: ext.ext.id.clone(),
            confirmation: confirmation.to_string(),
            approve,
        };
        self.post(request, cx, |_, result, cx| {
            if let Err(e) = result {
                ToastManager::error(e, cx);
            }
        });
    }

    pub(crate) fn dismiss_refusals(&mut self, ext: &ApiExtension, cx: &mut Context<Self>) {
        self.dismissed_refusals.extend(ext.refusals.iter().map(|r| r.at_ms));
        cx.notify();
    }

    // ─── Rendering ──────────────────────────────────────────────────────────

    fn render_header(&self, ext: &ClientExtension, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let e = &ext.ext;
        let title = e.view_title.clone().unwrap_or_else(|| e.name.clone());
        let subtitle = if ext.local {
            format!("{} {}", e.name, e.version)
        } else {
            format!("{} {} · on {}", e.name, e.version, ext.connection_name)
        };
        let refreshed = match (e.refreshing, e.refreshed_at_ms) {
            (true, _) => "Refreshing…".to_string(),
            (false, Some(at)) => format!("Updated {}", okena_ui::ago::format_ago(at, okena_ui::ago::now_millis())),
            (false, None) => String::new(),
        };
        h_flex()
            .px(px(16.0))
            .py(px(10.0))
            .gap(px(12.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(ui_text(15.0, cx))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(t.text_primary))
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_muted))
                            .child(subtitle),
                    ),
            )
            .when_some(self.running.as_ref(), |row, running| {
                row.child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_secondary))
                        .child(if running.items > 1 {
                            format!("Running {} on {} rows…", running.label, running.items)
                        } else {
                            format!("Running {}…", running.label)
                        }),
                )
            })
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(refreshed),
            )
            .child(
                button("ext-refresh", "Refresh", &t)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .child(
                button("ext-settings", "Settings", &t)
                    .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx))),
            )
            .into_any_element()
    }

    /// Why nothing (or not everything) is showing: missing tools, missing
    /// configuration, a failure, refusals, the last refresh's error.
    fn render_banners(&self, ext: &ApiExtension, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx);
        let mut out = Vec::new();
        match &ext.state {
            ExtRunState::MissingTools => {
                let mut panel = banner("ext-missing-tools", t.warning, &t)
                    .child(banner_title(
                        format!("{} needs tools that are missing", ext.name),
                        &t,
                        cx,
                    ))
                    .child(banner_text(
                        "Nothing runs until they are installed. Install them, then re-check.",
                        &t,
                        cx,
                    ));
                for tool in ext.tools.iter().filter(|tool| !tool.ok) {
                    panel = panel.child(
                        v_flex()
                            .gap(px(2.0))
                            .pt(px(6.0))
                            .child(
                                div()
                                    .text_size(ui_text_md(cx))
                                    .text_color(rgb(t.text_primary))
                                    .child(format!(
                                        "{} — {}",
                                        tool.name,
                                        tool.problem.clone().unwrap_or_default()
                                    )),
                            )
                            .when(!tool.install_hint.is_empty(), |d| {
                                d.child(render::code_line(format!("Install: {}", tool.install_hint), &t, cx))
                            }),
                    );
                }
                out.push(
                    panel
                        .child(
                            h_flex().pt(px(8.0)).child(
                                button_primary("ext-recheck", "Re-check", &t)
                                    .on_click(cx.listener(|this, _, _, cx| this.recheck(cx))),
                            ),
                        )
                        .into_any_element(),
                );
            }
            ExtRunState::NeedsConfig { missing } => {
                let labels: Vec<String> = missing
                    .iter()
                    .map(|key| {
                        ext.config_schema
                            .iter()
                            .find(|f| &f.key == key)
                            .map_or(key.clone(), |f| f.label.clone())
                    })
                    .collect();
                out.push(
                    banner("ext-needs-config", t.warning, &t)
                        .child(banner_title(format!("{} needs configuring", ext.name), &t, cx))
                        .child(banner_text(format!("Set: {}", labels.join(", ")), &t, cx))
                        .child(
                            h_flex().pt(px(8.0)).child(
                                button_primary("ext-configure", "Configure", &t)
                                    .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx))),
                            ),
                        )
                        .into_any_element(),
                );
            }
            ExtRunState::Failed { message } => out.push(
                banner("ext-failed", t.error, &t)
                    .child(banner_title(format!("{} failed to start", ext.name), &t, cx))
                    .child(banner_text(message.clone(), &t, cx))
                    .child(
                        h_flex().pt(px(8.0)).child(
                            button("ext-retry", "Try again", &t)
                                .on_click(cx.listener(|this, _, _, cx| this.recheck(cx))),
                        ),
                    )
                    .into_any_element(),
            ),
            ExtRunState::Disabled => out.push(
                banner("ext-disabled", t.text_muted, &t)
                    .child(banner_title(format!("{} is turned off", ext.name), &t, cx))
                    .child(banner_text("Turn it on from the Extensions page.", &t, cx))
                    .into_any_element(),
            ),
            _ => {}
        }

        for request in &ext.pending_confirmations {
            let target = match request.items.len() {
                0 => String::new(),
                1 => format!(" on {}", request.items[0]),
                n => format!(" on {n} items"),
            };
            let (yes, no) = (request.id.clone(), request.id.clone());
            out.push(
                banner("ext-pending-confirm", t.warning, &t)
                    .child(banner_title(
                        format!("An agent asks to run {}{target}", request.action_label),
                        &t,
                        cx,
                    ))
                    .child(banner_text(
                        "It is marked destructive, so it waits for you. Declining tells the agent no.",
                        &t,
                        cx,
                    ))
                    .child(
                        h_flex()
                            .pt(px(8.0))
                            .gap(px(8.0))
                            .child(
                                render::danger_button("ext-confirm-yes", format!("Run {}", request.action_label), &t)
                                    .on_click(cx.listener(move |this, _, _, cx| this.answer(&yes, true, cx))),
                            )
                            .child(
                                button("ext-confirm-no", "Decline", &t)
                                    .on_click(cx.listener(move |this, _, _, cx| this.answer(&no, false, cx))),
                            ),
                    )
                    .into_any_element(),
            );
        }

        let refusals: Vec<_> = ext
            .refusals
            .iter()
            .filter(|r| !self.dismissed_refusals.contains(&r.at_ms))
            .collect();
        if !refusals.is_empty() {
            let mut panel = banner("ext-refusals", t.error, &t).child(banner_title(
                "okena refused what this extension asked for",
                &t,
                cx,
            ));
            for refusal in refusals.iter().rev().take(3) {
                panel = panel.child(banner_text(refusal.message.clone(), &t, cx));
            }
            let ext_for_dismiss = ext.clone();
            out.push(
                panel
                    .child(
                        h_flex().pt(px(8.0)).child(
                            button("ext-refusals-dismiss", "Dismiss", &t).on_click(cx.listener(
                                move |this, _, _, cx| this.dismiss_refusals(&ext_for_dismiss, cx),
                            )),
                        ),
                    )
                    .into_any_element(),
            );
        }
        if let Some(error) = &ext.refresh_error {
            out.push(
                banner("ext-refresh-error", t.error, &t)
                    .child(banner_title("The last refresh failed", &t, cx))
                    .child(banner_text(error.clone(), &t, cx))
                    .into_any_element(),
            );
        }
        if let Some((ok, message)) = &self.last_result {
            out.push(
                banner("ext-last-result", if *ok { t.success } else { t.error }, &t)
                    .child(banner_text(message.clone(), &t, cx))
                    .into_any_element(),
            );
        }
        out
    }

    fn render_pending(&self, ext: &ApiExtension, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pending = self.pending.as_ref()?;
        let t = theme(cx);
        let action = &pending.action;
        let target = match pending.items.len() {
            0 => String::new(),
            1 => format!(" on {}", pending.items[0]),
            n => format!(" on {n} rows"),
        };
        let mut body = v_flex().gap(px(10.0)).p(px(16.0)).w(px(460.0)).child(
            div()
                .text_size(ui_text(15.0, cx))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(t.text_primary))
                .child(format!("{}{target}", action.label)),
        );
        if !action.description.is_empty() {
            body = body.child(banner_text(action.description.clone(), &t, cx));
        }
        if pending.confirming {
            body = body.child(
                div()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(if action.destructive { t.error } else { t.text_secondary }))
                    .child(if action.destructive {
                        format!(
                            "{} marks this action destructive. Run it{target}?",
                            ext.name
                        )
                    } else {
                        format!("Run it{target}?")
                    }),
            );
            if pending.items.len() > 1 {
                body = body.child(banner_text(pending.items.join(", "), &t, cx));
            }
        } else {
            for (index, (input, field)) in pending.fields.iter().enumerate() {
                body = body.child(render::form_field(index, input, field, &t, cx));
            }
        }
        let confirm_label = if pending.confirming && action.destructive {
            format!("Yes, {}", action.label)
        } else {
            action.label.clone()
        };
        let confirm = if pending.confirming && action.destructive {
            render::danger_button("ext-pending-run", confirm_label, &t)
        } else {
            button_primary("ext-pending-run", confirm_label, &t)
        };
        body = body.child(
            h_flex()
                .justify_end()
                .gap(px(8.0))
                .pt(px(4.0))
                .child(
                    button("ext-pending-cancel", "Cancel", &t)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_pending(cx))),
                )
                .child(confirm.on_click(cx.listener(|this, _, _, cx| this.submit_pending(cx)))),
        );
        Some(
            modal_backdrop("ext-pending-backdrop", &t)
                .items_start()
                .pt(px(80.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.cancel_pending(cx)),
                )
                .child(modal_content("ext-pending", &t).child(body))
                .into_any_element(),
        )
    }
}

fn banner(id: &'static str, accent: u32, t: &ThemeColors) -> Div {
    let _ = id;
    v_flex()
        .mx(px(16.0))
        .mt(px(12.0))
        .p(px(12.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(rgb(accent))
        .bg(rgb(t.bg_secondary))
}

fn banner_title(text: impl Into<SharedString>, t: &ThemeColors, cx: &App) -> Div {
    div()
        .text_size(ui_text_md(cx))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(t.text_primary))
        .child(text.into())
}

fn banner_text(text: impl Into<SharedString>, t: &ThemeColors, cx: &App) -> Div {
    div()
        .text_size(ui_text_ms(cx))
        .text_color(rgb(t.text_secondary))
        .child(text.into())
}

impl Render for ExtensionPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let Some(ext) = self.extension(cx) else {
            return v_flex()
                .size_full()
                .bg(rgb(t.bg_primary))
                .items_center()
                .justify_center()
                .child(okena_ui::empty_state::empty_state(
                    "This extension is no longer installed.",
                    &t,
                    cx,
                ))
                .into_any_element();
        };
        let header = self.render_header(&ext, cx);
        let banners = self.render_banners(&ext.ext, cx);
        let pending = self.render_pending(&ext.ext, cx);
        let body = match &ext.ext.view {
            Some(view) if !matches!(ext.ext.state, ExtRunState::Disabled) => {
                let view = view.clone();
                let badges = (self.agent_badges)(&ext, cx);
                render::node(self, &ext, &view, view.root, &badges, &mut HashSet::new(), cx)
            }
            _ if matches!(ext.ext.state, ExtRunState::Starting) => {
                render::loading("Starting…", &t, cx)
            }
            _ => div().into_any_element(),
        };
        v_flex()
            .relative()
            .size_full()
            .bg(rgb(t.bg_primary))
            .child(header)
            .child(
                div()
                    .id("ext-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(v_flex().children(banners).child(div().p(px(16.0)).child(body))),
            )
            .children(pending)
            .into_any_element()
    }
}

/// Reads the list outside a render, for callers without a pane.
pub fn extension_by_key(key: &str, cx: &App) -> Option<Arc<ClientExtension>> {
    extensions_entity(cx).and_then(|e: Entity<ExtensionsState>| e.read(cx).get(key))
}
