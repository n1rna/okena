//! The Extensions page for extensions installed from git: install from a
//! URL (or a local folder), approve what an extension asks for, turn it on
//! and off, configure it, update it, reload it, remove it.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::extension::{
    ConfigFieldKind, ExtConfigField, ExtInstallPreview, ExtPermissions, ExtRunState, ExtSource,
};
use okena_ui::button::{button, button_primary};
use okena_ui::input::input_container;
use okena_ui::simple_input::{SimpleInput, SimpleInputState};
use okena_ui::theme::{ThemeColors, theme, with_alpha};
use okena_ui::toggle::toggle_switch;
use okena_ui::tokens::{ui_text, ui_text_md, ui_text_ms, ui_text_sm};
use okena_workspace::extensions_state::{ClientExtension, extensions_entity, split_key};
use okena_workspace::toast::ToastManager;

use crate::render::{badge, code_line, danger_button, tone_color};

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    Git,
    Local,
}

/// An install or update waiting for the user's approval.
struct Review {
    connection: String,
    preview: ExtInstallPreview,
    /// Set for an update of this extension id.
    updating: Option<String>,
}

enum ConfigValue {
    Text(Entity<SimpleInputState>),
    Toggle(bool),
    Select(usize),
}

struct ConfigForm {
    fields: Vec<(ExtConfigField, ConfigValue)>,
}

pub struct ExtensionsManager {
    source_kind: SourceKind,
    url: Entity<SimpleInputState>,
    git_ref: Entity<SimpleInputState>,
    path: Entity<SimpleInputState>,
    local_path: Entity<SimpleInputState>,
    /// The daemon to install on; the local one unless the user picks another.
    target: Option<String>,
    review: Option<Review>,
    /// What is in progress, for the user to wait on.
    busy: Option<String>,
    error: Option<String>,
    /// The extension whose details are open, by key.
    expanded: Option<String>,
    configs: HashMap<String, ConfigForm>,
    confirm_remove: Option<String>,
    /// Horizontal space kept on each side. Settings lays sections out edge
    /// to edge and wants its gutter; the Extensions page pads its own column.
    inset: Pixels,
    _extensions: Option<Subscription>,
}

impl ExtensionsManager {
    /// `open` expands that extension (by key), e.g. from its view's Settings button.
    pub fn new(open: Option<String>, cx: &mut Context<Self>) -> Self {
        let input = |placeholder: &str, cx: &mut Context<Self>| {
            let placeholder = placeholder.to_string();
            cx.new(|cx| SimpleInputState::new(cx).placeholder(placeholder))
        };
        let subscription = extensions_entity(cx).map(|e| cx.observe(&e, |_, _, cx| cx.notify()));
        let mut this = Self {
            source_kind: SourceKind::Git,
            url: input("e.g. https://github.com/acme/okena-extensions.git", cx),
            git_ref: input("Optional: a branch, tag or commit (default branch if empty)", cx),
            path: input("For a library: the extension's folder, e.g. extensions/cli-table", cx),
            local_path: input("The folder holding extension.toml, e.g. /code/my-ext", cx),
            target: None,
            review: None,
            busy: None,
            error: None,
            expanded: None,
            configs: HashMap::new(),
            confirm_remove: None,
            inset: px(16.0),
            _extensions: subscription,
        };
        if let Some(key) = open {
            this.expand(key, cx);
        }
        this
    }

    fn target_connection(&self, cx: &App) -> Option<String> {
        self.target.clone().or_else(|| {
            extensions_entity(cx)?
                .read(cx)
                .connections()
                .first()
                .map(|c| c.id.clone())
        })
    }

    fn post(
        &self,
        connection: &str,
        action: ActionRequest,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Result<Option<serde_json::Value>, String>, &mut Context<Self>) + 'static,
    ) {
        let client = extensions_entity(cx).and_then(|e| e.read(cx).client(connection));
        cx.spawn(async move |this, cx| {
            let result = match client {
                Some(client) => smol::unblock(move || client.post_action(action)).await,
                None => Err("that daemon is not connected".to_string()),
            };
            let _ = this.update(cx, |this, cx| {
                done(this, result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn set_busy(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.busy = Some(message.into());
        self.error = None;
        cx.notify();
    }

    // ─── Install ────────────────────────────────────────────────────────────

    fn source(&self, cx: &App) -> Result<ExtSource, String> {
        let value = |input: &Entity<SimpleInputState>| input.read(cx).value().trim().to_string();
        match self.source_kind {
            SourceKind::Git => {
                let url = value(&self.url);
                if url.is_empty() {
                    return Err("Enter the repository's git URL".into());
                }
                let optional = |s: String| (!s.is_empty()).then_some(s);
                Ok(ExtSource::Git {
                    url,
                    git_ref: optional(value(&self.git_ref)),
                    path: optional(value(&self.path)),
                    commit: String::new(),
                })
            }
            SourceKind::Local => {
                let path = value(&self.local_path);
                if path.is_empty() {
                    return Err("Enter the extension's folder".into());
                }
                Ok(ExtSource::Local { path })
            }
        }
    }

    fn review_install(&mut self, cx: &mut Context<Self>) {
        let source = match self.source(cx) {
            Ok(source) => source,
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return;
            }
        };
        let Some(connection) = self.target_connection(cx) else {
            self.error = Some("No daemon is connected".into());
            cx.notify();
            return;
        };
        self.set_busy("Fetching the extension…", cx);
        let conn = connection.clone();
        self.post(&connection, ActionRequest::ExtensionPreview { source }, cx, move |this, result, _| {
            this.busy = None;
            match result.and_then(|v| {
                serde_json::from_value::<ExtInstallPreview>(v.unwrap_or_default()).map_err(|e| e.to_string())
            }) {
                Ok(preview) => {
                    this.review = Some(Review {
                        connection: conn,
                        preview,
                        updating: None,
                    })
                }
                Err(e) => this.error = Some(e),
            }
        });
    }

    fn approve(&mut self, cx: &mut Context<Self>) {
        let Some(review) = self.review.take() else { return };
        let preview = review.preview;
        let approved = preview.permissions.clone();
        let commit = match &preview.source {
            ExtSource::Git { commit, .. } if !commit.is_empty() => Some(commit.clone()),
            _ => None,
        };
        let name = preview.name.clone();
        let (action, busy) = match &review.updating {
            Some(id) => (
                ActionRequest::ExtensionUpdate {
                    id: id.clone(),
                    commit,
                    approved: Some(approved),
                },
                format!("Updating {name}…"),
            ),
            None => (
                ActionRequest::ExtensionInstall {
                    source: preview.source.clone(),
                    commit,
                    approved,
                },
                if preview.prebuilt {
                    format!("Installing {name}…")
                } else {
                    format!("Building {name} from source… this can take a few minutes")
                },
            ),
        };
        self.set_busy(busy, cx);
        let updating = review.updating.is_some();
        self.post(&review.connection, action, cx, move |this, result, cx| {
            this.busy = None;
            match result {
                Ok(_) => ToastManager::success(
                    if updating { format!("Updated {name}") } else { format!("Installed {name}") },
                    cx,
                ),
                Err(e) => this.error = Some(e),
            }
        });
    }

    // ─── Installed extensions ───────────────────────────────────────────────

    /// Set the horizontal space kept on each side.
    pub fn set_inset(&mut self, inset: Pixels, cx: &mut Context<Self>) {
        self.inset = inset;
        cx.notify();
    }

    /// Show the extension `key` with its details open, whatever was open
    /// before. How an extension's own view sends you to its settings.
    pub fn open(&mut self, key: String, cx: &mut Context<Self>) {
        if self.expanded.as_deref() != Some(key.as_str()) {
            self.expand(key, cx);
        }
    }

    fn expand(&mut self, key: String, cx: &mut Context<Self>) {
        if self.expanded.as_deref() == Some(key.as_str()) {
            self.expanded = None;
            cx.notify();
            return;
        }
        self.expanded = Some(key.clone());
        self.confirm_remove = None;
        self.load_config(&key, cx);
        cx.notify();
    }

    /// Reads the extension's saved configuration from its daemon and builds
    /// the form from its schema.
    fn load_config(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some((connection, id)) = split_key(key).map(|(c, i)| (c.to_string(), i.to_string())) else {
            return;
        };
        let key = key.to_string();
        self.post(&connection, ActionRequest::GetSettings, cx, move |this, result, cx| {
            let saved = result
                .ok()
                .flatten()
                .and_then(|s| s.get("extension_settings")?.get(&id).cloned())
                .unwrap_or_default();
            let Some(ext) = extensions_entity(cx).and_then(|e| e.read(cx).get(&key)) else {
                return;
            };
            let fields = ext
                .ext
                .config_schema
                .iter()
                .map(|field| {
                    let value = saved.get(&field.key).or(field.default.as_ref()).cloned();
                    let control = match field.kind {
                        ConfigFieldKind::Bool => {
                            ConfigValue::Toggle(value.and_then(|v| v.as_bool()).unwrap_or(false))
                        }
                        ConfigFieldKind::Select => ConfigValue::Select(
                            value
                                .and_then(|v| v.as_str().map(str::to_string))
                                .and_then(|v| field.options.iter().position(|o| *o == v))
                                .unwrap_or(0),
                        ),
                        _ => {
                            let text = match value {
                                Some(serde_json::Value::String(s)) => s,
                                Some(serde_json::Value::Null) | None => String::new(),
                                Some(other) => other.to_string(),
                            };
                            ConfigValue::Text(cx.new(|cx| SimpleInputState::new(cx).default_value(text)))
                        }
                    };
                    (field.clone(), control)
                })
                .collect();
            this.configs.insert(key, ConfigForm { fields });
        });
    }

    fn save_config(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some((connection, id)) = split_key(key).map(|(c, i)| (c.to_string(), i.to_string())) else {
            return;
        };
        let Some(form) = self.configs.get(key) else { return };
        let mut values = serde_json::Map::new();
        for (field, control) in &form.fields {
            let value = match control {
                ConfigValue::Toggle(on) => serde_json::Value::Bool(*on),
                ConfigValue::Select(i) => field
                    .options
                    .get(*i)
                    .map_or(serde_json::Value::Null, |o| serde_json::Value::String(o.clone())),
                ConfigValue::Text(input) => {
                    let text = input.read(cx).value().trim().to_string();
                    if field.kind == ConfigFieldKind::Number {
                        if text.is_empty() {
                            serde_json::Value::Null
                        } else {
                            match text.parse::<f64>() {
                                Ok(n) => serde_json::json!(n),
                                Err(_) => {
                                    self.error = Some(format!("{} must be a number", field.label));
                                    cx.notify();
                                    return;
                                }
                            }
                        }
                    } else {
                        serde_json::Value::String(text)
                    }
                }
            };
            values.insert(field.key.clone(), value);
        }
        let patch = serde_json::json!({ "extension_settings": { id.clone(): values } });
        self.post(&connection, ActionRequest::SetSettings { patch }, cx, move |this, result, cx| {
            match result {
                Ok(_) => ToastManager::success("Configuration saved", cx),
                Err(e) => this.error = Some(format!("Saving the configuration failed: {e}")),
            }
        });
    }

    fn set_enabled(&mut self, key: &str, enabled: bool, cx: &mut Context<Self>) {
        let Some((connection, id)) = split_key(key).map(|(c, i)| (c.to_string(), i.to_string())) else {
            return;
        };
        let client = extensions_entity(cx).and_then(|e| e.read(cx).client(&connection));
        cx.spawn(async move |this, cx| {
            // The daemon's own set, so its built-in extensions stay as they are.
            let result = smol::unblock(move || -> Result<(), String> {
                let client = client.ok_or("that daemon is not connected")?;
                let settings = client.post_action(ActionRequest::GetSettings)?.unwrap_or_default();
                let mut set: Vec<String> = settings
                    .get("enabled_extensions")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                set.retain(|e| e != &id);
                if enabled {
                    set.push(id);
                }
                client.post_action(ActionRequest::SetSettings {
                    patch: serde_json::json!({ "enabled_extensions": set }),
                })?;
                Ok(())
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                if let Err(e) = result {
                    this.error = Some(e);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn check_updates(&mut self, cx: &mut Context<Self>) {
        let connections: Vec<String> = extensions_entity(cx)
            .map(|e| e.read(cx).connections().iter().map(|c| c.id.clone()).collect())
            .unwrap_or_default();
        self.set_busy("Checking for updates…", cx);
        let pending = Arc::new(std::sync::atomic::AtomicUsize::new(connections.len()));
        for connection in connections {
            let pending = pending.clone();
            self.post(&connection, ActionRequest::ExtensionCheckUpdates, cx, move |this, result, _| {
                if let Err(e) = result {
                    this.error = Some(e);
                }
                if pending.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                    this.busy = None;
                }
            });
        }
    }

    /// Update (or, for a local folder, reload): fetch what it would change,
    /// ask again if it wants more permissions, then install it.
    fn update(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some((connection, id)) = split_key(key).map(|(c, i)| (c.to_string(), i.to_string())) else {
            return;
        };
        self.set_busy("Fetching the update…", cx);
        let conn = connection.clone();
        self.post(
            &connection,
            ActionRequest::ExtensionPreviewUpdate { id: id.clone() },
            cx,
            move |this, result, cx| {
                this.busy = None;
                let preview = match result.and_then(|v| {
                    serde_json::from_value::<ExtInstallPreview>(v.unwrap_or_default()).map_err(|e| e.to_string())
                }) {
                    Ok(preview) => preview,
                    Err(e) => {
                        this.error = Some(e);
                        return;
                    }
                };
                let asks_more = preview.added_permissions.as_ref().is_some_and(|p| !p.is_empty());
                this.review = Some(Review {
                    connection: conn,
                    preview,
                    updating: Some(id),
                });
                if !asks_more {
                    this.approve(cx);
                }
            },
        );
    }

    fn remove(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some((connection, id)) = split_key(key).map(|(c, i)| (c.to_string(), i.to_string())) else {
            return;
        };
        self.confirm_remove = None;
        self.set_busy("Removing…", cx);
        self.post(&connection, ActionRequest::ExtensionRemove { id: id.clone() }, cx, move |this, result, cx| {
            this.busy = None;
            match result {
                Ok(_) => ToastManager::success(format!("Removed {id}"), cx),
                Err(e) => this.error = Some(e),
            }
        });
    }

    // ─── Rendering ──────────────────────────────────────────────────────────

    fn render_installed(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let list: Vec<Arc<ClientExtension>> = extensions_entity(cx)
            .map(|e| e.read(cx).list().to_vec())
            .unwrap_or_default();
        let updates = list.iter().filter(|e| e.ext.update.is_some()).count();
        let caption = match (list.len(), updates) {
            (0, _) => "None yet — install one below.".to_string(),
            (n, 0) => format!("{n} installed"),
            (n, u) => format!("{n} installed · {u} with an update"),
        };
        let mut cards = v_flex().gap(px(8.0));
        if list.is_empty() {
            cards = cards.child(
                card(&t)
                    .p(px(20.0))
                    .items_center()
                    .gap(px(8.0))
                    .child(icon_tile("icons/puzzle.svg", t.text_muted))
                    .child(
                        div()
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(t.text_muted))
                            .child("No extensions are installed from git yet."),
                    ),
            );
        }
        for ext in &list {
            cards = cards.child(self.render_row(ext, cx));
        }
        v_flex()
            .gap(px(10.0))
            .child(heading(
                "Installed from git",
                caption,
                Some(
                    button("ext-check-updates", "Check for updates", &t)
                        .border_1()
                        .border_color(rgb(t.border))
                        .on_click(cx.listener(|this, _, _, cx| this.check_updates(cx)))
                        .into_any_element(),
                ),
                &t,
                cx,
            ))
            .child(cards)
            .into_any_element()
    }

    fn render_row(&mut self, ext: &ClientExtension, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let key = ext.key();
        let e = &ext.ext;
        let expanded = self.expanded.as_deref() == Some(key.as_str());
        let (state_label, state_tone) = match &e.state {
            ExtRunState::Disabled => ("off", okena_core::extension::Tone::Neutral),
            ExtRunState::Starting => ("starting", okena_core::extension::Tone::Info),
            ExtRunState::Ready => ("running", okena_core::extension::Tone::Success),
            ExtRunState::MissingTools => ("missing tools", okena_core::extension::Tone::Warning),
            ExtRunState::NeedsConfig { .. } => ("needs configuring", okena_core::extension::Tone::Warning),
            ExtRunState::Failed { .. } => ("failed", okena_core::extension::Tone::Danger),
            ExtRunState::Unknown => ("?", okena_core::extension::Tone::Neutral),
        };
        let source = source_line(&e.source);
        let toggle_key = key.clone();
        let enabled = e.enabled;
        let expand_key = key.clone();
        let tile_color = if enabled { t.border_active } else { t.text_muted };

        let mut badges = h_flex().gap(px(6.0)).items_center().child(badge(
            &okena_core::extension::ExtBadge {
                label: state_label.into(),
                tone: state_tone,
            },
            &t,
            cx,
        ));
        if !ext.local {
            badges = badges.child(badge(
                &okena_core::extension::ExtBadge {
                    label: format!("on {}", ext.connection_name),
                    tone: okena_core::extension::Tone::Info,
                },
                &t,
                cx,
            ));
        }

        let mut actions = h_flex().flex_shrink_0().items_center().gap(px(8.0));
        if let Some(update) = &e.update {
            let key = key.clone();
            let label = if update.version.is_empty() {
                "Update".to_string()
            } else if update.added_permissions.is_empty() {
                format!("Update to {}", update.version)
            } else {
                format!("Update to {} (asks for more)", update.version)
            };
            actions = actions.child(
                button_primary(SharedString::from(format!("ext-update-{key}")), label, &t)
                    .text_size(ui_text_ms(cx))
                    .py(px(4.0))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.update(&key, cx)
                    })),
            );
        }
        if matches!(e.source, ExtSource::Local { .. }) {
            let key = key.clone();
            actions = actions.child(
                button(SharedString::from(format!("ext-reload-{key}")), "Rebuild & reload", &t)
                    .text_size(ui_text_ms(cx))
                    .py(px(4.0))
                    .border_1()
                    .border_color(rgb(t.border))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.update(&key, cx)
                    })),
            );
        }
        actions = actions
            .child(
                toggle_switch(SharedString::from(format!("ext-enabled-{key}")), enabled, &t).on_click(
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.set_enabled(&toggle_key, !enabled, cx)
                    }),
                ),
            )
            .child(
                svg()
                    .path(if expanded {
                        "icons/chevron-down.svg"
                    } else {
                        "icons/chevron-right.svg"
                    })
                    .size(px(12.0))
                    .text_color(rgb(t.text_muted)),
            );

        let header = h_flex()
            .id(SharedString::from(format!("ext-row-{key}")))
            .cursor_pointer()
            .px(px(14.0))
            .py(px(12.0))
            .gap(px(12.0))
            .items_center()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .child(icon_tile("icons/puzzle.svg", tile_color))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.0))
                    .child(
                        h_flex()
                            .gap(px(8.0))
                            .items_center()
                            .child(
                                div()
                                    .text_size(ui_text(13.0, cx))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(t.text_primary))
                                    .child(e.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(e.version.clone()),
                            )
                            .child(badges),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_muted))
                            .child(source),
                    ),
            )
            .child(actions)
            .on_click(cx.listener(move |this, _, _, cx| this.expand(expand_key.clone(), cx)));

        let mut row = card(&t)
            .when(expanded, |d| d.border_color(with_alpha(t.border_active, 0.5)))
            .child(header);
        if expanded {
            row = row.child(
                div()
                    .border_t_1()
                    .border_color(rgb(t.border))
                    .pt(px(12.0))
                    .child(self.render_details(ext, cx)),
            );
        }
        row.into_any_element()
    }

    fn render_details(&mut self, ext: &ClientExtension, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let key = ext.key();
        let e = &ext.ext;
        let mut body = v_flex().pl(px(58.0)).pr(px(14.0)).pb(px(14.0)).gap(px(12.0));
        if !e.description.is_empty() {
            body = body.child(note(e.description.clone(), &t, cx));
        }
        body = body.child(permissions_block("Approved permissions", &e.permissions, &t, cx));
        if !e.requires.is_empty() {
            let mut tools = v_flex().gap(px(4.0)).child(label("Required tools", &t, cx));
            for tool in &e.requires {
                let status = e.tools.iter().find(|s| s.name == tool.name);
                let (text, color) = match status {
                    Some(s) if s.ok => (
                        format!("{} {}", tool.name, s.version.clone().unwrap_or_default()),
                        t.success,
                    ),
                    Some(s) => (
                        format!("{} — {}", tool.name, s.problem.clone().unwrap_or_default()),
                        t.warning,
                    ),
                    None => (tool.name.clone(), t.text_secondary),
                };
                tools = tools.child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(color))
                        .child(text),
                );
                if status.is_some_and(|s| !s.ok) && !tool.install_hint.is_empty() {
                    tools = tools.child(code_line(format!("Install: {}", tool.install_hint), &t, cx));
                }
            }
            body = body.child(tools);
        }
        if !e.config_schema.is_empty() {
            body = body.child(self.render_config(&key, &t, cx));
        }
        let remove_key = key.clone();
        let confirming = self.confirm_remove.as_deref() == Some(key.as_str());
        body = body.child(if confirming {
            h_flex()
                .gap(px(8.0))
                .child(note(format!("Remove {} and delete its data?", e.name), &t, cx))
                .child(
                    button(SharedString::from(format!("ext-remove-cancel-{key}")), "Keep", &t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirm_remove = None;
                            cx.notify();
                        })),
                )
                .child(
                    danger_button("ext-remove-confirm", "Remove".into(), &t)
                        .on_click(cx.listener(move |this, _, _, cx| this.remove(&remove_key, cx))),
                )
                .into_any_element()
        } else {
            h_flex()
                .child(
                    button(SharedString::from(format!("ext-remove-{key}")), "Remove…", &t).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.confirm_remove = Some(remove_key.clone());
                            cx.notify();
                        }),
                    ),
                )
                .into_any_element()
        });
        body.into_any_element()
    }

    fn render_config(&mut self, key: &str, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        let mut block = v_flex().gap(px(8.0)).child(label("Configuration", t, cx));
        let Some(form) = self.configs.get(key) else {
            return block.child(note("Loading…", t, cx)).into_any_element();
        };
        for (index, (field, control)) in form.fields.iter().enumerate() {
            let key_for = key.to_string();
            let control: AnyElement = match control {
                ConfigValue::Text(input) => input_container(t, None)
                    .px(px(8.0))
                    .py(px(4.0))
                    .child(SimpleInput::new(input).text_size(ui_text_md(cx)))
                    .into_any_element(),
                ConfigValue::Toggle(on) => {
                    let on = *on;
                    toggle_switch(SharedString::from(format!("ext-cfg-{key}-{index}")), on, t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(form) = this.configs.get_mut(&key_for)
                                && let Some((_, ConfigValue::Toggle(v))) = form.fields.get_mut(index)
                            {
                                *v = !on;
                                cx.notify();
                            }
                        }))
                        .into_any_element()
                }
                ConfigValue::Select(chosen) => h_flex()
                    .gap(px(4.0))
                    .flex_wrap()
                    .children(field.options.iter().enumerate().map(|(i, option)| {
                        let key_for = key_for.clone();
                        let active = i == *chosen;
                        div()
                            .id(SharedString::from(format!("ext-cfg-{key}-{index}-{i}")))
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(rgb(if active { t.border_active } else { t.border }))
                            .when(active, |d| d.bg(rgb(t.bg_selection)))
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_primary))
                            .child(option.clone())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(form) = this.configs.get_mut(&key_for)
                                    && let Some((_, ConfigValue::Select(v))) = form.fields.get_mut(index)
                                {
                                    *v = i;
                                    cx.notify();
                                }
                            }))
                    }))
                    .into_any_element(),
            };
            block = block.child(
                v_flex()
                    .gap(px(3.0))
                    .child(label(
                        if field.required { format!("{} *", field.label) } else { field.label.clone() },
                        t,
                        cx,
                    ))
                    .when(!field.description.is_empty(), |d| d.child(note(field.description.clone(), t, cx)))
                    .child(control),
            );
        }
        let save_key = key.to_string();
        block
            .child(
                h_flex().child(
                    button_primary(SharedString::from(format!("ext-cfg-save-{key}")), "Save", t)
                        .on_click(cx.listener(move |this, _, _, cx| this.save_config(&save_key, cx))),
                ),
            )
            .into_any_element()
    }

    fn render_install(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let connections: Vec<(String, String)> = extensions_entity(cx)
            .map(|e| {
                e.read(cx)
                    .connections()
                    .iter()
                    .map(|c| (c.id.clone(), if c.local { "This machine".into() } else { c.name.clone() }))
                    .collect()
            })
            .unwrap_or_default();
        let target = self.target_connection(cx);
        let kind = self.source_kind;
        let segment = |id: &'static str, text: &'static str, active: bool| {
            div()
                .id(id)
                .cursor_pointer()
                .px(px(12.0))
                .py(px(4.0))
                .rounded(px(6.0))
                .text_size(ui_text_ms(cx))
                .map(|d| {
                    if active {
                        d.bg(rgb(t.bg_secondary))
                            .shadow_sm()
                            .text_color(rgb(t.text_primary))
                    } else {
                        d.text_color(rgb(t.text_muted))
                            .hover(|s| s.text_color(rgb(t.text_secondary)))
                    }
                })
                .child(text)
        };
        let tabs = h_flex()
            .p(px(3.0))
            .gap(px(2.0))
            .rounded(px(8.0))
            .bg(rgb(t.bg_primary))
            .border_1()
            .border_color(rgb(t.border))
            .child(segment("ext-src-git", "Git repository", kind == SourceKind::Git).on_click(
                cx.listener(|this, _, _, cx| {
                    this.source_kind = SourceKind::Git;
                    cx.notify();
                }),
            ))
            .child(segment("ext-src-local", "Local folder", kind == SourceKind::Local).on_click(
                cx.listener(|this, _, _, cx| {
                    this.source_kind = SourceKind::Local;
                    cx.notify();
                }),
            ));
        let field = |caption: &str, input: &Entity<SimpleInputState>| {
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(5.0))
                .child(label(caption.to_string(), &t, cx))
                .child(
                    input_container(&t, None)
                        .bg(rgb(t.bg_primary))
                        .rounded(px(6.0))
                        .px(px(10.0))
                        .py(px(6.0))
                        .child(SimpleInput::new(input).text_size(ui_text_md(cx))),
                )
        };
        let mut form = v_flex().p(px(16.0)).gap(px(14.0)).child(tabs);
        form = match kind {
            SourceKind::Git => form.child(field("Repository URL", &self.url)).child(
                h_flex()
                    .gap(px(12.0))
                    .items_start()
                    .child(field("Ref", &self.git_ref))
                    .child(field("Folder in the repository", &self.path)),
            ),
            SourceKind::Local => form.child(field("Folder", &self.local_path)).child(note(
                "Built from source here. Rebuild & reload picks up your changes without reinstalling.",
                &t,
                cx,
            )),
        };
        let mut targets = h_flex().gap(px(6.0)).items_center();
        if connections.len() > 1 {
            targets = targets.child(label("Install on".to_string(), &t, cx));
            for (id, name) in connections {
                let active = target.as_deref() == Some(id.as_str());
                let pick = id.clone();
                targets = targets.child(
                    div()
                        .id(SharedString::from(format!("ext-target-{id}")))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(rgb(if active { t.border_active } else { t.border }))
                        .when(active, |d| d.bg(with_alpha(t.border_active, 0.12)))
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_primary))
                        .child(name)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.target = Some(pick.clone());
                            cx.notify();
                        })),
                );
            }
        }
        form = form.child(
            h_flex()
                .justify_between()
                .items_center()
                .child(targets)
                .child(
                    button_primary("ext-review", "Review…", &t)
                        .px(px(16.0))
                        .when(self.busy.is_some(), |b| b.opacity(0.5))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.busy.is_none() {
                                this.review_install(cx)
                            }
                        })),
                ),
        );
        v_flex()
            .gap(px(10.0))
            .child(heading(
                "Install an extension",
                "From a git repository, or a local folder while you develop one. You approve what it may do before it installs.",
                None,
                &t,
                cx,
            ))
            .child(card(&t).child(form))
            .into_any_element()
    }

    /// What the user approves: the permissions and tools, before anything installs.
    fn render_review(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let review = self.review.as_ref()?;
        let t = theme(cx);
        let p = &review.preview;
        let updating = review.updating.is_some();
        let mut card = v_flex()
            .p(px(14.0))
            .gap(px(10.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(rgb(t.border_active))
            .bg(rgb(t.bg_secondary))
            .child(
                div()
                    .text_size(ui_text(15.0, cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_primary))
                    .child(if updating {
                        format!("Update {} to {}", p.name, p.version)
                    } else {
                        format!("Install {} {}", p.name, p.version)
                    }),
            )
            .child(note(source_line(&p.source), &t, cx));
        if !p.description.is_empty() {
            card = card.child(note(p.description.clone(), &t, cx));
        }
        if let Some(added) = p.added_permissions.as_ref().filter(|a| !a.is_empty()) {
            card = card.child(permissions_block("This update asks for more", added, &t, cx));
        }
        card = card.child(permissions_block(
            if updating { "All it may do" } else { "It asks to" },
            &p.permissions,
            &t,
            cx,
        ));
        if !p.requires.is_empty() {
            let mut tools = v_flex().gap(px(4.0)).child(label("It needs these tools installed", &t, cx));
            for tool in &p.requires {
                tools = tools.child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_primary))
                        .child(match &tool.min_version {
                            Some(min) => format!("{} {min} or newer — {}", tool.name, tool.install_hint),
                            None => format!("{} — {}", tool.name, tool.install_hint),
                        }),
                );
            }
            card = card.child(tools);
        }
        card = card.child(note(
            if p.prebuilt {
                "Uses the prebuilt extension.wasm from the repository.".to_string()
            } else {
                "Built from source on this machine.".to_string()
            },
            &t,
            cx,
        ));
        let blocked = p.build_problem.clone();
        if let Some(problem) = &blocked {
            card = card.child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.error))
                    .child(problem.clone()),
            );
        }
        card = card.child(
            h_flex()
                .justify_end()
                .gap(px(8.0))
                .child(button("ext-review-cancel", "Cancel", &t).on_click(cx.listener(|this, _, _, cx| {
                    this.review = None;
                    cx.notify();
                })))
                .child(
                    button_primary(
                        "ext-review-approve",
                        if updating { "Approve and update" } else { "Approve and install" },
                        &t,
                    )
                    .when(blocked.is_some(), |b| b.opacity(0.4))
                    .when(blocked.is_none(), |b| {
                        b.on_click(cx.listener(|this, _, _, cx| this.approve(cx)))
                    }),
                ),
        );
        Some(card.into_any_element())
    }
}

/// A section's title, a line saying what is in it, and what acts on all of it.
pub fn heading(
    title: impl Into<SharedString>,
    caption: impl Into<SharedString>,
    trailing: Option<AnyElement>,
    t: &ThemeColors,
    cx: &App,
) -> Div {
    h_flex()
        .w_full()
        .items_end()
        .justify_between()
        .gap(px(12.0))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(ui_text(14.0, cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(t.text_primary))
                        .child(title.into()),
                )
                .child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child(caption.into()),
                ),
        )
        .children(trailing)
}

/// A rounded card, the frame every extension and form sits in.
pub fn card(t: &ThemeColors) -> Div {
    v_flex()
        .w_full()
        .rounded(px(10.0))
        .border_1()
        .border_color(rgb(t.border))
        .bg(rgb(t.bg_secondary))
        .overflow_hidden()
}

/// An extension's icon on a tinted square.
pub fn icon_tile(icon: impl Into<SharedString>, color: u32) -> Div {
    div()
        .flex_shrink_0()
        .size(px(32.0))
        .rounded(px(8.0))
        .flex()
        .items_center()
        .justify_center()
        .bg(with_alpha(color, 0.14))
        .child(svg().path(icon.into()).size(px(16.0)).text_color(rgb(color)))
}

fn label(text: impl Into<SharedString>, t: &ThemeColors, cx: &App) -> Div {
    div()
        .text_size(ui_text_ms(cx))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(t.text_secondary))
        .child(text.into())
}

fn note(text: impl Into<SharedString>, t: &ThemeColors, cx: &App) -> Div {
    div()
        .text_size(ui_text_ms(cx))
        .text_color(rgb(t.text_muted))
        .child(text.into())
}

fn permissions_block(title: &str, p: &ExtPermissions, t: &ThemeColors, cx: &App) -> Div {
    let mut block = v_flex().gap(px(3.0)).child(label(title.to_string(), t, cx));
    if p.is_empty() {
        return block.child(note("Nothing beyond drawing its view.", t, cx));
    }
    for command in &p.commands {
        block = block.child(item(format!("Run `{command}`"), t, cx));
    }
    for path in &p.paths {
        block = block.child(item(format!("Read files under {path}"), t, cx));
    }
    if p.start_agents {
        block = block.child(
            div()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(tone_color(okena_core::extension::Tone::Warning, t)))
                .child("• Start agent sessions without asking first"),
        );
    }
    block
}

fn item(text: String, t: &ThemeColors, cx: &App) -> Div {
    div()
        .text_size(ui_text_ms(cx))
        .text_color(rgb(t.text_primary))
        .child(format!("• {text}"))
}

fn source_line(source: &ExtSource) -> String {
    match source {
        ExtSource::Git { url, git_ref, path, commit } => {
            let mut s = url.clone();
            if let Some(path) = path {
                s.push_str(&format!(" · {path}"));
            }
            s.push_str(&format!(" · {}", git_ref.as_deref().unwrap_or("default branch")));
            if !commit.is_empty() {
                s.push_str(&format!(" @ {}", &commit[..commit.len().min(10)]));
            }
            s
        }
        ExtSource::Local { path } => format!("Local folder {path}"),
    }
}

impl Render for ExtensionsManager {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let review = self.render_review(cx);
        let installed = self.render_installed(cx);
        let install = self.render_install(cx);
        v_flex()
            .px(self.inset)
            .gap(px(24.0))
            .when_some(self.busy.clone(), |d, busy| {
                d.child(
                    div()
                        .p(px(10.0))
                        .rounded(px(4.0))
                        .bg(with_alpha(t.border_active, 0.12))
                        .text_size(ui_text_md(cx))
                        .text_color(rgb(t.text_primary))
                        .child(busy),
                )
            })
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    h_flex()
                        .p(px(10.0))
                        .gap(px(8.0))
                        .rounded(px(4.0))
                        .border_1()
                        .border_color(rgb(t.error))
                        .child(
                            div()
                                .flex_1()
                                .text_size(ui_text_md(cx))
                                .text_color(rgb(t.error))
                                .child(error),
                        )
                        .child(button("ext-error-dismiss", "Dismiss", &t).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.error = None;
                                cx.notify();
                            },
                        ))),
                )
            })
            .children(review)
            .child(installed)
            .child(install)
            .child(note(
                "Extensions run sandboxed in okena's daemon. They reach your machine only through the commands and paths you approve.",
                &t,
                cx,
            ))
    }
}
