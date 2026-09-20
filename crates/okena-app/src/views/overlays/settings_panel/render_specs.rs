//! Specs settings — OpenSpec stores, and where okena looks for roots.
//!
//! Follows OpenSpec's own model (<https://openspec.dev/docs/stores>). The
//! discovery switches, folders and directory overrides are ordinary settings;
//! everything that changes OpenSpec's machine state — registering, creating or
//! forgetting a store, the machine `defaultStore` — goes through the daemon,
//! which owns those files and takes the CLI's registry lock.

use super::SettingsPanel;
use super::components::{hook_input_row, section_container, section_header};
use crate::views::components::add_root_form::{
    AddMode, AddRootChrome, AddRootForm, RootKind, describe, render_add_root,
};
use crate::settings::settings_entity;
use crate::theme::{ThemeColors, theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_ms};
use crate::views::components::SimpleInput;
use crate::views::components::simple_input::{InputChangedEvent, SimpleInputState};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::specs::{
    SpecDiagnostic, SpecPointer, SpecReference, SpecRoot, SpecRootKind, SpecSeverity, SpecStores,
};

/// State of the Specs page.
pub(super) struct SpecsPage {
    /// Show the "add a store" form at the top of the page.
    ///
    /// Set when something sent you here to add a root, so the form is the
    /// first thing on screen instead of something to scroll for.
    pub(super) add_first: bool,
    stores: Option<SpecStores>,
    loading: bool,
    /// The discovery settings the current snapshot was read with. Any change —
    /// a toggle, a folder, a directory override — triggers a re-read.
    fetched_for: Option<String>,
    refresh_pending: bool,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
    /// The add-a-root form, shared with the Roots page (QBL-429).
    add: AddRootForm,
    folder_input: Entity<SimpleInputState>,
    clone_dir_input: Entity<SimpleInputState>,
    data_dir_input: Entity<SimpleInputState>,
    config_dir_input: Entity<SimpleInputState>,
}

pub(super) fn text_input(
    cx: &mut Context<SettingsPanel>,
    placeholder: &'static str,
    value: Option<String>,
) -> Entity<SimpleInputState> {
    cx.new(|cx| {
        let state = SimpleInputState::new(cx).placeholder(placeholder);
        match value.filter(|v| !v.is_empty()) {
            Some(v) => state.default_value(v),
            None => state,
        }
    })
}

impl SpecsPage {
    pub(super) fn new(
        data_dir: Option<String>,
        config_dir: Option<String>,
        clone_dir: Option<String>,
        cx: &mut Context<SettingsPanel>,
    ) -> Self {
        let clone_dir_input = text_input(cx, "~/openspec", clone_dir);
        cx.subscribe(
            &clone_dir_input,
            |_this, entity, _: &InputChangedEvent, cx| {
                let val = entity.read(cx).value().to_string();
                settings_entity(cx).update(cx, |state, cx| state.set_spec_clone_dir(val, cx));
            },
        )
        .detach();
        let data_dir_input = text_input(cx, "Same as the openspec CLI", data_dir);
        cx.subscribe(
            &data_dir_input,
            |_this, entity, _: &InputChangedEvent, cx| {
                let val = entity.read(cx).value().to_string();
                settings_entity(cx).update(cx, |state, cx| state.set_spec_data_dir(val, cx));
            },
        )
        .detach();
        let config_dir_input = text_input(cx, "Same as the openspec CLI", config_dir);
        cx.subscribe(
            &config_dir_input,
            |_this, entity, _: &InputChangedEvent, cx| {
                let val = entity.read(cx).value().to_string();
                settings_entity(cx).update(cx, |state, cx| state.set_spec_config_dir(val, cx));
            },
        )
        .detach();

        Self {
            stores: None,
            add_first: false,
            loading: false,
            fetched_for: None,
            refresh_pending: false,
            busy: false,
            error: None,
            notice: None,
            add: AddRootForm::new(RootKind::Specs, cx),
            folder_input: text_input(cx, "e.g. ~/p/specs", None),
            clone_dir_input,
            data_dir_input,
            config_dir_input,
        }
    }
}

/// Whether the folder as typed in settings is the root the daemon reported.
///
/// The daemon canonicalizes (`~` expanded, symlinks resolved) and may be on
/// another machine, so this compares by suffix rather than re-resolving here.
fn folder_matches(folder: &str, root_path: &str) -> bool {
    let f = folder.trim().trim_end_matches(['/', '\\']);
    if f.is_empty() {
        return false;
    }
    match f.strip_prefix('~') {
        Some(rest) => !rest.is_empty() && root_path.ends_with(rest),
        None => root_path == f || root_path.ends_with(f),
    }
}

fn health(root: &SpecRoot, t: &ThemeColors) -> (&'static str, u32) {
    let warned = root
        .status
        .iter()
        .any(|d| d.severity == SpecSeverity::Warning)
        || root.references.iter().any(|r| !r.status.is_empty());
    if !root.healthy {
        ("problem", t.error)
    } else if warned {
        ("check", t.warning)
    } else {
        ("ok", t.success)
    }
}

pub(super) fn badge(label: &str, color: u32, cx: &App) -> Div {
    div()
        .flex_shrink_0()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(px(3.0))
        .bg(with_alpha(color, 0.15))
        .text_size(ui_text_ms(cx))
        .text_color(rgb(color))
        .child(label.to_string())
}

pub(super) fn muted(text: impl Into<SharedString>, t: &ThemeColors, cx: &App) -> Div {
    div()
        .min_w_0()
        .text_size(ui_text_ms(cx))
        .text_color(rgb(t.text_muted))
        .child(text.into())
}

pub(super) fn muted_row(text: impl Into<SharedString>, t: &ThemeColors, cx: &App) -> Div {
    muted(text, t, cx).px(px(12.0)).py(px(10.0))
}

/// A path on one line, cut with an ellipsis. Paths have no spaces to wrap at,
/// so letting them wrap breaks them mid-segment, and in a row that gives them
/// no width they collapse to a character per line.
pub(super) fn path_line(text: impl Into<SharedString>, t: &ThemeColors, cx: &App) -> Div {
    muted(text, t, cx).w_full().truncate()
}

pub(super) fn banner(text: String, color: u32, cx: &App) -> Div {
    div()
        .mx(px(12.0))
        .mt(px(8.0))
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(4.0))
        .bg(with_alpha(color, 0.1))
        .text_size(ui_text_ms(cx))
        .text_color(rgb(color))
        .child(text)
}

pub(super) fn diagnostic(d: &SpecDiagnostic, t: &ThemeColors, cx: &App) -> AnyElement {
    let color = match d.severity {
        SpecSeverity::Error => t.error,
        SpecSeverity::Warning => t.warning,
    };
    v_flex()
        .gap(px(1.0))
        .child(
            div()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(color))
                .child(d.message.clone()),
        )
        .when_some(d.fix.clone(), |el, fix| {
            el.child(muted(format!("Fix: {fix}"), t, cx))
        })
        .into_any_element()
}

fn reference(r: &SpecReference, t: &ThemeColors, cx: &App) -> AnyElement {
    if r.status.is_empty() {
        h_flex()
            .gap(px(6.0))
            .min_w_0()
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.success))
                    .child(format!("✓ {}", r.id)),
            )
            .child(path_line(r.root.clone().unwrap_or_default(), t, cx).flex_1())
            .into_any_element()
    } else {
        v_flex()
            .children(r.status.iter().map(|d| diagnostic(d, t, cx)))
            .into_any_element()
    }
}

impl SettingsPanel {
    fn discovery_fingerprint(cx: &App) -> String {
        let s = &settings_entity(cx).read(cx).settings;
        format!("{:?}|{:?}", s.harness.specs, s.harness.spec_repo)
    }

    /// Read what the daemon discovers. Cheap and local — no network.
    fn refresh_spec_stores(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.action_client.clone() else {
            return;
        };
        self.specs.fetched_for = Some(Self::discovery_fingerprint(cx));
        self.specs.loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::SpecStores)
                    .and_then(|v| v.ok_or_else(|| "Missing spec stores".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<SpecStores>(v)
                            .map_err(|e| format!("Unexpected spec stores: {e}"))
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.specs.loading = false;
                    match result {
                        Ok(stores) => this.specs.stores = Some(stores),
                        Err(e) => this.specs.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Re-read after a settings change, once the change has reached the
    /// daemon — settings travel as a separate message, so reading at once
    /// would show the old discovery.
    fn refresh_spec_stores_soon(&mut self, cx: &mut Context<Self>) {
        self.specs.refresh_pending = true;
        cx.spawn(async move |this, cx| {
            smol::Timer::after(std::time::Duration::from_millis(500)).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.specs.refresh_pending = false;
                    this.refresh_spec_stores(cx);
                });
            });
        })
        .detach();
    }

    /// Run a store change on the daemon, then say what happened and re-read.
    fn run_spec_action(
        &mut self,
        action: ActionRequest,
        describe: impl Fn(&serde_json::Value) -> String + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.specs.busy {
            return;
        }
        let Some(client) = self.action_client.clone() else {
            self.specs.error = Some("The local daemon is unavailable.".into());
            cx.notify();
            return;
        };
        self.specs.busy = true;
        self.specs.error = None;
        self.specs.notice = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || client.post_action(action)).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.specs.busy = false;
                    match result {
                        Ok(v) => {
                            this.specs.notice =
                                Some(describe(&v.unwrap_or(serde_json::Value::Null)));
                            this.specs.add.clear(cx);
                            this.refresh_spec_stores(cx);
                        }
                        Err(e) => this.specs.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Send whatever the shared form is asking for, or say what is missing.
    fn submit_add_spec_root(&mut self, cx: &mut Context<Self>) {
        let mode = self.specs.add.mode;
        match self.specs.add.request(cx) {
            Ok(action) => {
                self.run_spec_action(action, move |v| describe(RootKind::Specs, mode, v), cx)
            }
            Err(missing) => {
                self.specs.error = Some(missing);
                cx.notify();
            }
        }
    }

    /// Register the checkout at `path`, from the shared form or from the
    /// "Register" button on a root okena found but nobody has registered.
    fn register_spec_store(&mut self, path: String, id: Option<String>, cx: &mut Context<Self>) {
        if path.trim().is_empty() {
            self.specs.error = Some("Choose the store checkout's folder.".into());
            cx.notify();
            return;
        }
        self.run_spec_action(
            ActionRequest::SpecStoreRegister { path, id },
            |v| describe(RootKind::Specs, AddMode::Register, v),
            cx,
        );
    }

    fn add_spec_folder(&mut self, cx: &mut Context<Self>) {
        let value = self.specs.folder_input.read(cx).value().trim().to_string();
        if value.is_empty() {
            return;
        }
        let mut folders = settings_entity(cx).read(cx).settings.harness.spec_folders();
        folders.push(value);
        settings_entity(cx).update(cx, |state, cx| state.set_spec_folders(folders, cx));
        self.specs
            .folder_input
            .update(cx, |i, cx| i.set_value("", cx));
    }

    fn remove_spec_folder(&mut self, index: usize, cx: &mut Context<Self>) {
        let mut folders = settings_entity(cx).read(cx).settings.harness.spec_folders();
        if index < folders.len() {
            folders.remove(index);
        }
        settings_entity(cx).update(cx, |state, cx| state.set_spec_folders(folders, cx));
    }

    fn spec_button(
        &self,
        id: String,
        label: &str,
        primary: bool,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        div()
            .id(SharedString::from(id))
            .cursor_pointer()
            .flex_shrink_0()
            .px(px(10.0))
            .py(px(3.0))
            .rounded(px(4.0))
            .when(primary, |d| {
                d.bg(rgb(t.button_primary_bg))
                    .hover(|s| s.bg(rgb(t.button_primary_hover)))
                    .text_color(rgb(t.button_primary_fg))
            })
            .when(!primary, |d| {
                d.border_1()
                    .border_color(rgb(t.border))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .text_color(rgb(t.text_secondary))
            })
            .when(self.specs.busy, |d| d.opacity(0.6))
            .text_size(ui_text_ms(cx))
            .child(label.to_string())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| on_click(this, cx)),
            )
            .into_any_element()
    }

    fn render_spec_root_row(
        &self,
        root: &SpecRoot,
        border_top: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let (label, color) = health(root, &t);

        let mut actions = h_flex().gap(px(6.0)).flex_shrink_0();
        if root.kind == SpecRootKind::Store
            && let Some(id) = root.store_id.clone()
        {
            if root.is_default {
                actions = actions.child(self.spec_button(
                    format!("specs-undefault-{id}"),
                    "Clear default",
                    false,
                    |this, cx| {
                        this.run_spec_action(
                            ActionRequest::SpecSetDefaultStore { id: None },
                            |_| "Cleared the machine default store.".into(),
                            cx,
                        )
                    },
                    cx,
                ));
            } else if root.healthy {
                let target = id.clone();
                actions = actions.child(self.spec_button(
                    format!("specs-default-{id}"),
                    "Make default",
                    false,
                    move |this, cx| {
                        this.run_spec_action(
                            ActionRequest::SpecSetDefaultStore {
                                id: Some(target.clone()),
                            },
                            |v| {
                                format!(
                                    "'{}' is now the machine default store — openspec commands run outside a planning repo use it.",
                                    v["default_store"].as_str().unwrap_or("")
                                )
                            },
                            cx,
                        )
                    },
                    cx,
                ));
            }
            let target = id.clone();
            actions = actions.child(self.spec_button(
                format!("specs-unregister-{id}"),
                "Unregister",
                false,
                move |this, cx| {
                    this.run_spec_action(
                        ActionRequest::SpecStoreUnregister { id: target.clone() },
                        |v| {
                            format!(
                                "Unregistered '{}'. Its files are still at {}.",
                                v["id"].as_str().unwrap_or(""),
                                v["left_on_disk"].as_str().unwrap_or("")
                            )
                        },
                        cx,
                    )
                },
                cx,
            ));
        } else if root.status.iter().any(|d| d.code == "store_unregistered") {
            let path = root.path.clone();
            actions = actions.child(self.spec_button(
                format!("specs-register-{}", root.key),
                "Register",
                true,
                move |this, cx| this.register_spec_store(path.clone(), None, cx),
                cx,
            ));
        }

        let mut body = v_flex()
            .gap(px(3.0))
            .flex_1()
            .min_w_0()
            .child(
                h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .flex_wrap()
                    .child(
                        div()
                            .text_size(ui_text(13.0, cx))
                            .text_color(rgb(t.text_primary))
                            .child(root.name.clone()),
                    )
                    .when(root.is_default, |d| {
                        d.child(badge("default", t.border_active, cx))
                    })
                    .child(badge(label, color, cx)),
            )
            .child(path_line(root.path.clone(), &t, cx))
            .when_some(root.remote.clone(), |d, remote| {
                d.child(muted(format!("Remote: {remote}"), &t, cx))
            })
            .when(!root.used_by.is_empty(), |d| {
                d.child(muted(
                    format!("Used by: {}", root.used_by.join(", ")),
                    &t,
                    cx,
                ))
            });
        if !root.references.is_empty() {
            body = body.child(muted("References:", &t, cx));
            for r in &root.references {
                body = body.child(reference(r, &t, cx));
            }
        }
        for d in &root.status {
            body = body.child(diagnostic(d, &t, cx));
        }

        h_flex()
            .items_start()
            .gap(px(12.0))
            .px(px(12.0))
            .py(px(10.0))
            .when(border_top, |d| d.border_t_1().border_color(rgb(t.border)))
            .child(body)
            .child(actions)
            .into_any_element()
    }

    fn render_spec_pointer_row(
        &self,
        pointer: &SpecPointer,
        border_top: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let target = if pointer.store_id.is_empty() {
            "?".to_string()
        } else {
            pointer.store_id.clone()
        };
        let resolved = pointer.root_key.is_some();
        v_flex()
            .gap(px(3.0))
            .px(px(12.0))
            .py(px(10.0))
            .when(border_top, |d| d.border_t_1().border_color(rgb(t.border)))
            .child(
                h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .child(
                        div()
                            .text_size(ui_text(13.0, cx))
                            .text_color(rgb(t.text_primary))
                            .child(format!("{} → {target}", pointer.project)),
                    )
                    .when(resolved, |d| d.child(badge("resolves", t.success, cx))),
            )
            .child(path_line(pointer.path.clone(), &t, cx))
            .children(pointer.status.iter().map(|d| diagnostic(d, &t, cx)))
            .into_any_element()
    }

    fn render_spec_folder_row(
        &self,
        index: usize,
        folder: &str,
        roots: &[SpecRoot],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let root = roots.iter().find(|r| folder_matches(folder, &r.path));
        let mut body = v_flex().gap(px(3.0)).flex_1().min_w_0().child(
            div()
                .text_size(ui_text(13.0, cx))
                .text_color(rgb(t.text_primary))
                .child(folder.to_string()),
        );
        let mut actions = h_flex().gap(px(6.0)).flex_shrink_0();
        match root {
            Some(r) if r.kind != SpecRootKind::Folder => {
                let kind = match r.kind {
                    SpecRootKind::Store => "a registered store",
                    _ => "a project root",
                };
                body = body.child(muted(
                    format!("Already listed as {kind} ({}).", r.name),
                    &t,
                    cx,
                ));
            }
            Some(r) => {
                let (label, color) = health(r, &t);
                body = body.child(h_flex().child(badge(label, color, cx)));
                for d in &r.status {
                    body = body.child(diagnostic(d, &t, cx));
                }
                if r.status.iter().any(|d| d.code == "store_unregistered") {
                    let path = r.path.clone();
                    actions = actions.child(self.spec_button(
                        format!("specs-folder-register-{index}"),
                        "Register",
                        true,
                        move |this, cx| this.register_spec_store(path.clone(), None, cx),
                        cx,
                    ));
                }
            }
            None => {}
        }
        actions = actions.child(self.spec_button(
            format!("specs-folder-remove-{index}"),
            "Remove",
            false,
            move |this, cx| this.remove_spec_folder(index, cx),
            cx,
        ));
        h_flex()
            .items_start()
            .gap(px(12.0))
            .px(px(12.0))
            .py(px(10.0))
            .when(index > 0, |d| d.border_t_1().border_color(rgb(t.border)))
            .child(body)
            .child(actions)
            .into_any_element()
    }

    fn render_add_spec_store(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        section_container(&t)
            .child(render_add_root(
                &self.specs.add,
                AddRootChrome {
                    id_prefix: "specs",
                    busy: self.specs.busy,
                },
                cx,
                cx.listener(|this, mode: &AddMode, _window, cx| {
                    this.specs.add.mode = *mode;
                    this.specs.error = None;
                    cx.notify();
                }),
                cx.listener(|this, _, _window, cx| {
                    this.specs.add.init_git = !this.specs.add.init_git;
                    cx.notify();
                }),
                cx.listener(|this, _, _window, cx| this.submit_add_spec_root(cx)),
            ))
            .into_any_element()
    }

    pub(super) fn render_specs(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        // Read on first paint rather than when the panel opens, so a user who
        // never visits this page makes no call; re-read when discovery changes.
        let fingerprint = Self::discovery_fingerprint(cx);
        if self.action_client.is_some()
            && !self.specs.loading
            && !self.specs.refresh_pending
            && self.specs.fetched_for.as_deref() != Some(fingerprint.as_str())
        {
            if self.specs.fetched_for.is_none() {
                self.refresh_spec_stores(cx);
            } else {
                self.refresh_spec_stores_soon(cx);
            }
        }

        let settings = settings_entity(cx).read(cx).settings.clone();
        let discovery = settings.harness.specs.clone();
        let folders = settings.harness.spec_folders();
        let stores = self.specs.stores.clone();
        let roots: Vec<SpecRoot> = stores.as_ref().map(|s| s.roots.clone()).unwrap_or_default();
        let daemon = self.action_client.is_some();

        let mut page = v_flex();

        // Sent here to add a root: the form leads, so it is on screen without
        // scrolling past the stores you already have.
        if self.specs.add_first {
            page = page
                .child(section_header("Add a store", &t, cx))
                .child(self.render_add_spec_store(cx));
        }

        // ── Overview ───────────────────────────────────────────────────────
        // Takes the row's free width; without `flex_1` the column shrinks to its
        // narrowest content and the paths wrap a character per line.
        let mut facts = v_flex()
            .flex_1()
            .min_w_0()
            .gap(px(2.0))
            .px(px(12.0))
            .py(px(8.0));
        if let Some(s) = &stores {
            facts = facts
                .child(path_line(
                    format!("Store registry: {}", s.registry_path),
                    &t,
                    cx,
                ))
                .child(path_line(
                    format!("Global config: {}", s.config_path),
                    &t,
                    cx,
                ))
                .child(muted(
                    format!(
                        "Machine default store: {}",
                        s.default_store.as_deref().unwrap_or("none")
                    ),
                    &t,
                    cx,
                ));
        }
        page = page.child(section_header("OpenSpec", &t, cx)).child(
            section_container(&t)
                .child(
                    h_flex()
                        .items_end()
                        .justify_between()
                        .border_t_1()
                        .border_color(rgb(t.border))
                        .child(facts)
                        .child(div().flex_shrink_0().px(px(12.0)).py(px(8.0)).child(self.spec_button(
                            "specs-refresh".into(),
                            if self.specs.loading { "Reading…" } else { "Refresh" },
                            false,
                            |this, cx| this.refresh_spec_stores(cx),
                            cx,
                        ))),
                ),
        );
        if let Some(notice) = self.specs.notice.clone() {
            page = page.child(banner(notice, t.success, cx));
        }
        if let Some(error) = self.specs.error.clone() {
            page = page.child(banner(error, t.error, cx));
        }
        if let Some(s) = &stores {
            for d in &s.status {
                page = page.child(div().mx(px(12.0)).mt(px(8.0)).child(diagnostic(d, &t, cx)));
            }
        }

        // ── Stores ─────────────────────────────────────────────────────────
        page = page.child(section_header("Stores", &t, cx));
        let mut container = section_container(&t);
        let store_roots: Vec<&SpecRoot> = roots
            .iter()
            .filter(|r| r.kind == SpecRootKind::Store)
            .collect();
        if !daemon {
            container = container.child(muted_row(
                "The local daemon is unavailable, so stores cannot be read.",
                &t,
                cx,
            ));
        } else if stores.is_none() {
            container = container.child(muted_row("Reading stores…", &t, cx));
        } else if store_roots.is_empty() {
            container = container.child(muted_row(
                if discovery.registry {
                    "No stores are registered on this machine yet. Register a checkout or create a store below."
                } else {
                    "Listing registered stores is off. Stores your projects point at still show here."
                },
                &t,
                cx,
            ));
        }
        for (i, root) in store_roots.iter().enumerate() {
            container = container.child(self.render_spec_root_row(root, i > 0, cx));
        }
        page = page.child(container);

        if !self.specs.add_first {
            page = page
                .child(section_header("Add a store", &t, cx))
                .child(self.render_add_spec_store(cx));
        }

        // ── Discovery ──────────────────────────────────────────────────────
        page = page.child(section_header("Discovery", &t, cx)).child(
            section_container(&t)
                .child(self.render_toggle(
                    "specs-discover-registry",
                    "List every registered store",
                    discovery.registry,
                    true,
                    |state, val, cx| state.set_spec_discovery_registry(val, cx),
                    cx,
                ))
                .child(self.render_toggle(
                    "specs-discover-projects",
                    "Find OpenSpec roots and store pointers in projects",
                    discovery.projects,
                    true,
                    |state, val, cx| state.set_spec_discovery_projects(val, cx),
                    cx,
                ))
                .child(hook_input_row(
                    "specs-clone-dir",
                    "Clone folder",
                    "Where a clone goes when no destination is given. Empty is ~/openspec.",
                    &self.specs.clone_dir_input,
                    &t,
                    false,
                    cx,
                )),
        );

        if discovery.projects && stores.is_some() {
            let project_roots: Vec<&SpecRoot> = roots
                .iter()
                .filter(|r| r.kind == SpecRootKind::Project)
                .collect();
            let pointers = stores
                .as_ref()
                .map(|s| s.pointers.clone())
                .unwrap_or_default();
            let mut container = section_container(&t);
            if project_roots.is_empty() && pointers.is_empty() {
                container = container.child(muted_row(
                    "No project holds an openspec/ tree or points at a store.",
                    &t,
                    cx,
                ));
            }
            let mut first = true;
            for root in project_roots {
                container = container.child(self.render_spec_root_row(root, !first, cx));
                first = false;
            }
            for pointer in &pointers {
                container = container.child(self.render_spec_pointer_row(pointer, !first, cx));
                first = false;
            }
            page = page
                .child(section_header("Found in projects", &t, cx))
                .child(container);
        }

        // ── Folders ────────────────────────────────────────────────────────
        let mut container = section_container(&t);
        if folders.is_empty() {
            container = container.child(muted_row(
                "No extra folders. Add one that holds — or will hold — an openspec/ tree to \
                 show it without registering it with OpenSpec.",
                &t,
                cx,
            ));
        }
        for (i, folder) in folders.iter().enumerate() {
            container = container.child(self.render_spec_folder_row(i, folder, &roots, cx));
        }
        container = container.child(
            h_flex()
                .gap(px(8.0))
                .items_center()
                .px(px(12.0))
                .py(px(8.0))
                .border_t_1()
                .border_color(rgb(t.border))
                .child(
                    okena_ui::input::input_container(&t, None)
                        .flex_1()
                        .min_w_0()
                        .px(px(8.0))
                        .py(px(5.0))
                        .child(
                            SimpleInput::new(&self.specs.folder_input).text_size(ui_text(13.0, cx)),
                        ),
                )
                .child(self.spec_button(
                    "specs-add-folder".into(),
                    "Add folder",
                    true,
                    |this, cx| this.add_spec_folder(cx),
                    cx,
                )),
        );
        page = page
            .child(section_header("Folders", &t, cx))
            .child(container);

        // ── OpenSpec directories ───────────────────────────────────────────
        page = page
            .child(section_header("OpenSpec directories", &t, cx))
            .child(
            section_container(&t)
                .child(hook_input_row(
                    "specs-data-dir",
                    "Data directory",
                    "Where OpenSpec keeps stores/registry.yaml. Empty resolves it like the CLI: \
                     $XDG_DATA_HOME/openspec, else ~/.local/share/openspec. Set it if your shell \
                     exports XDG_DATA_HOME — okena started from the dock never sees it.",
                    &self.specs.data_dir_input,
                    &t,
                    true,
                    cx,
                ))
                .child(hook_input_row(
                    "specs-config-dir",
                    "Config directory",
                    "Where OpenSpec keeps config.json and its defaultStore. Empty resolves \
                     $XDG_CONFIG_HOME/openspec, else ~/.config/openspec.",
                    &self.specs.config_dir_input,
                    &t,
                    false,
                    cx,
                )),
        );

        page.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::folder_matches;

    #[test]
    fn a_typed_folder_matches_the_root_the_daemon_canonicalized() {
        assert!(folder_matches("~/p/specs", "/Users/me/p/specs"));
        assert!(folder_matches("~/p/specs/", "/Users/me/p/specs"));
        // macOS resolves /tmp to /private/tmp.
        assert!(folder_matches("/tmp/specs", "/private/tmp/specs"));
        assert!(folder_matches("/srv/specs", "/srv/specs"));
    }

    #[test]
    fn unrelated_or_blank_folders_do_not_match() {
        assert!(!folder_matches("~/p/specs", "/Users/me/p/other"));
        assert!(!folder_matches("", "/anything"));
        assert!(!folder_matches("~", "/Users/me"));
    }
}
