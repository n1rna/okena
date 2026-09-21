//! Tasks settings — connecting okena to a task manager.
//!
//! The credential lives on the daemon, which is the only thing that talks to a
//! provider, so everything here goes through an action rather than touching a
//! key directly. The panel never sees a stored key: it can set one and clear
//! one, and reads back only whether a provider is connected and as whom.

use super::SettingsPanel;
use super::components::{section_container, section_header, settings_input_row};
use crate::settings::{SettingsState, settings_entity};
use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms};
use crate::views::components::SimpleInput;
use crate::views::harness::{AZURE_DEVOPS, notify_task_auth_changed, provider_hint};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::tasks::{TaskAuthState, TaskAuthStatusResponse, TaskProviderStatus};

/// Default name for a new connection when none is typed: the backend's own.
fn okena_tasks_kind_name(kind: &str) -> String {
    match kind {
        AZURE_DEVOPS => "Azure DevOps".to_string(),
        _ => "Linear".to_string(),
    }
}

impl SettingsPanel {
    /// Read every provider's auth state. Cheap and local — the daemon answers
    /// from the stored credential without calling out.
    pub(super) fn refresh_task_providers(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.action_client.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::TasksAuthStatus)
                    .and_then(|v| v.ok_or_else(|| "Missing auth status".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<TaskAuthStatusResponse>(v)
                            .map_err(|e| format!("Unexpected auth status: {e}"))
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(status) => this.tasks_status = Some(status),
                        Err(e) => this.tasks_error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Store a key for `provider`, after the daemon verifies it.
    fn connect_provider(&mut self, provider: String, cx: &mut Context<Self>) {
        if self.tasks_busy {
            return;
        }
        let key = self.tasks_api_key_input.read(cx).value().trim().to_string();
        if key.is_empty() {
            self.tasks_error = Some("Paste an API key or token first.".into());
            cx.notify();
            return;
        }
        let organization_url = (provider == AZURE_DEVOPS)
            .then(|| self.tasks_org_url_input.read(cx).value().trim().to_string());
        if organization_url.as_deref() == Some("") {
            self.tasks_error = Some("Enter your organization URL first.".into());
            cx.notify();
            return;
        }
        let Some(client) = self.action_client.clone() else {
            self.tasks_error = Some("The local daemon is unavailable.".into());
            cx.notify();
            return;
        };
        self.tasks_busy = true;
        self.tasks_error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client.post_action(ActionRequest::TasksConnectApiKey {
                    provider,
                    api_key: key,
                    organization_url,
                })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.tasks_busy = false;
                    match result {
                        Ok(_) => {
                            // Clear it on success: the panel has no reason to
                            // keep a credential on screen once it is stored.
                            this.tasks_api_key_input
                                .update(cx, |i, cx| i.set_value("", cx));
                            this.refresh_task_providers(cx);
                            // So an open Tasks view loads without being
                            // reopened, even if Settings has closed by now.
                            notify_task_auth_changed(cx);
                        }
                        Err(e) => this.tasks_error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn disconnect_provider(&mut self, provider: String, cx: &mut Context<Self>) {
        let Some(client) = self.action_client.clone() else {
            return;
        };
        self.tasks_busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client.post_action(ActionRequest::TasksDisconnect { provider })
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.tasks_busy = false;
                    match result {
                        Err(e) => this.tasks_error = Some(e),
                        Ok(_) => notify_task_auth_changed(cx),
                    }
                    this.refresh_task_providers(cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Add a connection to `kind`, named from the field beside the buttons.
    ///
    /// A connection exists before it is signed in: adding it makes the row,
    /// and the key field on that row is where the login happens. That is what
    /// lets someone add a second Linear account without disturbing the first.
    fn add_connection(&mut self, kind: &'static str, cx: &mut Context<Self>) {
        let Some(client) = self.action_client.clone() else {
            return;
        };
        let typed = self.tasks_new_name_input.read(cx).value().trim().to_string();
        let name = if typed.is_empty() {
            okena_tasks_kind_name(kind)
        } else {
            typed
        };
        self.tasks_busy = true;
        self.tasks_error = None;
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
                    this.tasks_busy = false;
                    match result {
                        Err(e) => this.tasks_error = Some(e),
                        Ok(_) => {
                            this.tasks_new_name_input
                                .update(cx, |i, cx| i.set_value("", cx));
                            notify_task_auth_changed(cx);
                        }
                    }
                    this.refresh_task_providers(cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Forget a connection and its credential. The daemon refuses while a
    /// space still reads it, and says which spaces are in the way.
    fn remove_connection(&mut self, connection_id: String, cx: &mut Context<Self>) {
        let Some(client) = self.action_client.clone() else {
            return;
        };
        self.tasks_busy = true;
        self.tasks_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client.post_action(ActionRequest::TaskConnectionRemove { connection_id })
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.tasks_busy = false;
                    match result {
                        Err(e) => this.tasks_error = Some(e),
                        Ok(_) => notify_task_auth_changed(cx),
                    }
                    this.refresh_task_providers(cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Commit the name being edited.
    fn commit_connection_rename(&mut self, cx: &mut Context<Self>) {
        let Some(connection_id) = self.tasks_renaming.take() else {
            return;
        };
        let name = self.tasks_rename_input.read(cx).value().trim().to_string();
        cx.notify();
        if name.is_empty() {
            return;
        }
        let Some(client) = self.action_client.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client.post_action(ActionRequest::TaskConnectionRename { connection_id, name })
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if let Err(e) = result {
                        this.tasks_error = Some(e);
                    }
                    this.refresh_task_providers(cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Chips choosing the one provider the harness reads.
    fn render_provider_choice(
        &self,
        providers: &[TaskProviderStatus],
        active: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        // Before the daemon answers, offer the providers this build ships, so
        // the choice is not blank for the first frame.
        let mut choices: Vec<(String, String)> = if providers.is_empty() {
            vec![
                ("linear".into(), "Linear".into()),
                (AZURE_DEVOPS.into(), "Azure DevOps".into()),
            ]
        } else {
            providers
                .iter()
                .map(|p| (p.provider.clone(), p.display_name.clone()))
                .collect()
        };
        // A space may still name a connection the daemon has not answered for
        // yet (first paint, or one just added). Show it rather than leaving the
        // selection looking unset.
        if !choices.iter().any(|(id, _)| id == active) {
            choices.push((active.to_string(), active.to_string()));
        }

        let chips: Vec<AnyElement> = choices
            .into_iter()
            .map(|(id, name)| {
                let is_selected = id == active;
                div()
                    .id(SharedString::from(format!("tasks-provider-{id}")))
                    .cursor_pointer()
                    .px(px(10.0))
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(rgb(if is_selected {
                        t.border_active
                    } else {
                        t.border
                    }))
                    .when(is_selected, |d| d.bg(with_alpha(t.button_primary_bg, 0.15)))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(if is_selected {
                        t.text_primary
                    } else {
                        t.text_secondary
                    }))
                    .child(name)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            let id = id.clone();
                            // A key typed for one provider is not the other's.
                            this.tasks_api_key_input
                                .update(cx, |i, cx| i.set_value("", cx));
                            this.tasks_error = None;
                            settings_entity(cx).update(cx, |state: &mut SettingsState, cx| {
                                state.set_space_connection(id, cx);
                            });
                        }),
                    )
                    .into_any_element()
            })
            .collect();

        v_flex()
            .px(px(12.0))
            .py(px(8.0))
            .gap(px(6.0))
            .child(
                v_flex()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(ui_text(13.0, cx))
                            .text_color(rgb(t.text_primary))
                            .child("This space's connection"),
                    )
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(
                                "The space showing reads this one. Another connection \
                                 stays connected, but its tasks are not shown.",
                            ),
                    ),
            )
            .child(h_flex().gap(px(6.0)).flex_wrap().children(chips))
            .into_any_element()
    }

    fn render_provider(
        &self,
        p: &TaskProviderStatus,
        active: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let (color, status) = match &p.auth {
            TaskAuthState::Connected { account } => (
                t.success,
                match account {
                    Some(a) => format!("Connected as {a}"),
                    None => "Connected".to_string(),
                },
            ),
            // Distinct from disconnected: the key is there but the provider
            // rejected it, so the fix is a new key rather than a first one.
            TaskAuthState::Expired => (t.error, "Key rejected — reconnect".to_string()),
            TaskAuthState::Disconnected => (t.text_muted, "Not connected".to_string()),
            TaskAuthState::Unknown => (t.text_muted, "Unknown".to_string()),
        };
        let connected = matches!(p.auth, TaskAuthState::Connected { .. });
        let id = p.provider.clone();
        let kind = p.kind();
        let busy = self.tasks_busy;
        let renaming = self.tasks_renaming.as_deref() == Some(id.as_str());

        v_flex()
            .gap(px(8.0))
            .px(px(12.0))
            .py(px(10.0))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        v_flex()
                            .gap(px(2.0))
                            .child(if renaming {
                                okena_ui::input::input_container(&t, None)
                                    .w(px(180.0))
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .child(
                                        SimpleInput::new(&self.tasks_rename_input)
                                            .text_size(ui_text(13.0, cx)),
                                    )
                                    .into_any_element()
                            } else {
                                let for_rename = id.clone();
                                let current = p.display_name.clone();
                                div()
                                    .id(SharedString::from(format!("tasks-name-{id}")))
                                    .cursor_pointer()
                                    .text_size(ui_text(13.0, cx))
                                    .text_color(rgb(t.text_primary))
                                    .child(p.display_name.clone())
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _window, cx| {
                                            // Click the name to rename it.
                                            this.tasks_rename_input.update(cx, |i, cx| {
                                                i.set_value(&current, cx)
                                            });
                                            this.tasks_renaming = Some(for_rename.clone());
                                            cx.notify();
                                        }),
                                    )
                                    .into_any_element()
                            })
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .bg(with_alpha(color, 0.15))
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(color))
                                    .child(status),
                            ),
                    )
                    .when(renaming, |d| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("tasks-rename-save-{id}")))
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(4.0))
                                .rounded(px(4.0))
                                .bg(rgb(t.button_primary_bg))
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.button_primary_fg))
                                .child("Save")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _window, cx| {
                                        this.commit_connection_rename(cx);
                                    }),
                                ),
                        )
                    })
                    .when(!renaming, |d| {
                        let id = id.clone();
                        d.child(
                            div()
                                .id(SharedString::from(format!("tasks-remove-{id}")))
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(4.0))
                                .rounded(px(4.0))
                                .hover(|s| s.bg(with_alpha(t.error, 0.1)))
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_muted))
                                .child("Remove")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _window, cx| {
                                        this.remove_connection(id.clone(), cx);
                                    }),
                                ),
                        )
                    })
                    .when(connected, |d| {
                        let id = id.clone();
                        d.child(
                            div()
                                .id(SharedString::from(format!("tasks-disconnect-{id}")))
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(4.0))
                                .rounded(px(4.0))
                                .hover(|s| s.bg(with_alpha(t.error, 0.1)))
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.error))
                                .child("Disconnect")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _window, cx| {
                                        this.disconnect_provider(id.clone(), cx);
                                    }),
                                ),
                        )
                    }),
            )
            // The key field is only offered where it would do something: a
            // connected provider needs a disconnect, not a second key, and only
            // the active provider is worth connecting from here.
            .when(!connected && active, |d| {
                let id = id.clone();
                d.child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_secondary))
                        // By backend, not by connection id: the second Linear
                        // account's id is `linear-2`, and it needs Linear's
                        // instructions all the same.
                        .child(provider_hint(kind)),
                )
                .when(kind == AZURE_DEVOPS, |d| {
                    d.child(
                        okena_ui::input::input_container(&t, None)
                            .w_full()
                            .px(px(8.0))
                            .py(px(5.0))
                            .child(
                                SimpleInput::new(&self.tasks_org_url_input)
                                    .text_size(ui_text(13.0, cx)),
                            ),
                    )
                })
                .child(
                    h_flex()
                        .gap(px(8.0))
                        .items_center()
                        .child(
                            okena_ui::input::input_container(&t, None)
                                .flex_1()
                                .min_w_0()
                                .px(px(8.0))
                                .py(px(5.0))
                                .child(
                                    SimpleInput::new(&self.tasks_api_key_input)
                                        .text_size(ui_text(13.0, cx)),
                                ),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("tasks-connect-{id}")))
                                .cursor_pointer()
                                .flex_shrink_0()
                                .px(px(12.0))
                                .py(px(5.0))
                                .rounded(px(4.0))
                                .bg(rgb(t.button_primary_bg))
                                .hover(|s| s.bg(rgb(t.button_primary_hover)))
                                .text_size(ui_text_md(cx))
                                .text_color(rgb(t.button_primary_fg))
                                .child(if busy { "Verifying…" } else { "Connect" })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _window, cx| {
                                        this.connect_provider(id.clone(), cx);
                                    }),
                                ),
                        ),
                )
            })
            .into_any_element()
    }

    /// Name a new connection and pick which backend it talks to.
    ///
    /// okena holds any number of them, of either kind: a second Linear account
    /// or a second Azure DevOps organization is a new connection with its own
    /// login, and several spaces may share one.
    fn render_add_connection(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let button = |kind: &'static str, label: &'static str, cx: &mut Context<Self>| {
            div()
                .id(SharedString::from(format!("tasks-add-{kind}")))
                .cursor_pointer()
                .flex_shrink_0()
                .px(px(10.0))
                .py(px(5.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(rgb(t.border))
                .hover(|s| s.bg(with_alpha(t.button_primary_bg, 0.1)))
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_secondary))
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        this.add_connection(kind, cx);
                    }),
                )
        };
        v_flex()
            .px(px(12.0))
            .py(px(10.0))
            .gap(px(6.0))
            .border_t_1()
            .border_color(rgb(t.border))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(
                        "Add another account or organization. Name it so you can tell \
                         it apart, then sign it in on its own row.",
                    ),
            )
            .child(
                h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .child(
                        okena_ui::input::input_container(&t, None)
                            .flex_1()
                            .min_w_0()
                            .px(px(8.0))
                            .py(px(5.0))
                            .child(
                                SimpleInput::new(&self.tasks_new_name_input)
                                    .text_size(ui_text(13.0, cx)),
                            ),
                    )
                    .child(button("linear", "Add Linear", cx))
                    .child(button(AZURE_DEVOPS, "Add Azure DevOps", cx)),
            )
            .into_any_element()
    }

    pub(super) fn render_tasks(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);
        // Read on first paint of the page rather than when the panel opens, so
        // a user who never visits Tasks makes no call at all.
        if self.tasks_status.is_none() && self.action_client.is_some() && !self.tasks_busy {
            self.refresh_task_providers(cx);
        }

        let providers = self
            .tasks_status
            .as_ref()
            .map(|s| s.providers.clone())
            .unwrap_or_default();

        let mut body = v_flex()
            .child(section_header("Task manager", &t, cx))
            .child(settings_input_row(
                "tasks-intro",
                "Connections",
                "okena reads your assigned work from a task manager. Each login is \
                 a connection, and the space showing reads exactly one of them — \
                 tasks from two are never merged. A key is verified once and then \
                 stored by the daemon: the app never holds it, and never sends it \
                 anywhere else.",
                &t,
                cx,
                false,
            ));

        let active = settings_entity(cx)
            .read(cx)
            .settings
            .active_space()
            .connection_id()
            .to_string();
        body = body.child(
            section_container(&t).child(self.render_provider_choice(&providers, &active, cx)),
        );

        let mut container = section_container(&t);
        if providers.is_empty() {
            container = container.child(
                div()
                    .px(px(12.0))
                    .py(px(10.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(if self.action_client.is_some() {
                        "Loading providers…"
                    } else {
                        "The local daemon is unavailable, so providers cannot be configured."
                    }),
            );
        }
        for p in &providers {
            container = container.child(self.render_provider(p, p.provider == active, cx));
        }
        container = container.child(self.render_add_connection(cx));
        body = body.child(container);

        if let Some(err) = self.tasks_error.clone() {
            body = body.child(
                div()
                    .mx(px(12.0))
                    .mt(px(8.0))
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(4.0))
                    .bg(with_alpha(t.error, 0.1))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.error))
                    .child(err),
            );
        }
        body
    }
}
