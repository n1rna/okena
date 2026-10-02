//! Library settings — the origins this space reads, adding one, and where
//! okena looks for them.
//!
//! One page for every origin type (QBL-440): knowledge stores, OpenSpec roots
//! and freeform folders are listed, added and removed the same way, and the
//! page says for each what only its type has.
//!
//! What is a setting and what is not follows where each type records itself.
//! The discovery switches, the clone folders, the OpenSpec folders and
//! directory overrides, and the freeform folders are ordinary settings
//! (`spaces[].library`). Knowledge stores live in okena's own registry
//! (`knowledge/stores.yaml` in the profile, ADR-0003) and OpenSpec stores in
//! OpenSpec's machine registry, with its `defaultStore`
//! (<https://openspec.dev/docs/stores>) — so adding, creating and forgetting
//! an origin all go through the daemon, which owns those files and takes the
//! CLI's registry lock.

use super::SettingsPanel;
use super::components::{hook_input_row, section_container, section_header};
use crate::settings::settings_entity;
use crate::theme::{ThemeColors, theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_ms};
use crate::views::components::SimpleInput;
use crate::views::components::add_root_form::{
    AddMode, AddRootChrome, AddRootForm, describe, render_add_root,
};
use crate::views::components::simple_input::{InputChangedEvent, SimpleInputState};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::diagnostic::{Diagnostic, Severity};
use okena_core::library::{
    LibraryOrigin, LibraryOrigins, LibraryPointer, OriginKind, OriginType,
};
use okena_core::specs::SpecReference;
use okena_core::store_git::StoreGitStatus;

/// State of the Library page.
pub(super) struct LibraryPage {
    /// Show the "add an origin" form at the top of the page.
    ///
    /// Set when something sent you here to add one, so the form is the first
    /// thing on screen instead of something to scroll for.
    pub(super) add_first: bool,
    origins: Option<LibraryOrigins>,
    loading: bool,
    /// The settings the current snapshot was read with. Any change — a toggle,
    /// a folder, a directory override — triggers a re-read.
    fetched_for: Option<String>,
    refresh_pending: bool,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
    /// The add-an-origin form, shared with the Origins page (QBL-429).
    add: AddRootForm,
    /// A folder to show as a spec origin without registering it with OpenSpec.
    spec_folder_input: Entity<SimpleInputState>,
    knowledge_clone_dir_input: Entity<SimpleInputState>,
    spec_clone_dir_input: Entity<SimpleInputState>,
    freeform_clone_dir_input: Entity<SimpleInputState>,
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

/// A text box whose every change is written to a setting.
fn setting_input(
    cx: &mut Context<SettingsPanel>,
    placeholder: &'static str,
    value: Option<String>,
    write: fn(&mut crate::settings::SettingsState, String, &mut Context<crate::settings::SettingsState>),
) -> Entity<SimpleInputState> {
    let input = text_input(cx, placeholder, value);
    cx.subscribe(&input, move |_this, entity, _: &InputChangedEvent, cx| {
        let val = entity.read(cx).value().to_string();
        settings_entity(cx).update(cx, |state, cx| write(state, val, cx));
    })
    .detach();
    input
}

impl LibraryPage {
    pub(super) fn new(
        library: &okena_core::spaces::LibraryConfig,
        cx: &mut Context<SettingsPanel>,
    ) -> Self {
        Self {
            origins: None,
            add_first: false,
            loading: false,
            fetched_for: None,
            refresh_pending: false,
            busy: false,
            error: None,
            notice: None,
            add: AddRootForm::new(OriginType::Knowledge, cx),
            spec_folder_input: text_input(cx, "e.g. ~/p/specs", None),
            knowledge_clone_dir_input: setting_input(
                cx,
                "~/knowledge",
                library.knowledge.clone_dir.clone(),
                |state, val, cx| state.set_knowledge_clone_dir(val, cx),
            ),
            spec_clone_dir_input: setting_input(
                cx,
                "~/openspec",
                library.spec.clone_dir.clone(),
                |state, val, cx| state.set_spec_clone_dir(val, cx),
            ),
            freeform_clone_dir_input: setting_input(
                cx,
                "~/library",
                library.freeform.clone_dir.clone(),
                |state, val, cx| state.set_freeform_clone_dir(val, cx),
            ),
            data_dir_input: setting_input(
                cx,
                "Same as the openspec CLI",
                library.spec.data_dir.clone(),
                |state, val, cx| state.set_spec_data_dir(val, cx),
            ),
            config_dir_input: setting_input(
                cx,
                "Same as the openspec CLI",
                library.spec.config_dir.clone(),
                |state, val, cx| state.set_spec_config_dir(val, cx),
            ),
        }
    }
}

fn health(origin: &LibraryOrigin, t: &ThemeColors) -> (&'static str, u32) {
    if !origin.healthy {
        ("problem", t.error)
    } else if origin.warned() {
        ("check", t.warning)
    } else {
        ("ok", t.success)
    }
}

/// `main → origin/main · 3 to pull · uncommitted changes`
fn git_line(git: &StoreGitStatus) -> String {
    let mut parts = vec![match (&git.branch, &git.upstream) {
        (Some(branch), Some(upstream)) => format!("{branch} → {upstream}"),
        (Some(branch), None) => format!("{branch}, no upstream"),
        (None, _) => "detached HEAD".to_string(),
    }];
    if git.behind > 0 {
        parts.push(format!("{} to pull", git.behind));
    }
    if git.ahead > 0 {
        parts.push(format!("{} to push", git.ahead));
    }
    if git.dirty {
        parts.push("uncommitted changes".to_string());
    }
    parts.join(" · ")
}

/// What an origin holds, in the words of its type. `None` where there is
/// nothing to say: an origin nobody can read, a spec root with no schema.
fn holds_line(origin: &LibraryOrigin) -> Option<String> {
    match origin.origin_type {
        OriginType::Knowledge => origin
            .counts
            .as_ref()
            .filter(|_| origin.healthy)
            .map(crate::views::harness::counts_line),
        OriginType::Spec => origin.schema.clone().map(|s| format!("Schema: {s}")),
        OriginType::Freeform => origin
            .documents
            .map(|n| format!("{n} {}", if n == 1 { "document" } else { "documents" })),
    }
}

/// Whether the page offers to take `origin` off the list. A project's origin
/// belongs to its repository, and okena's own store would only come back.
fn removable(origin: &LibraryOrigin) -> bool {
    !origin.builtin && origin.kind != OriginKind::Project
}

/// What the list says when a type has no origins yet.
fn empty_text(origin_type: OriginType, spec_registry: bool) -> &'static str {
    match origin_type {
        OriginType::Knowledge => {
            "No knowledge origins on this machine yet. Clone your team's store below."
        }
        OriginType::Spec if spec_registry => {
            "No OpenSpec roots yet. Register a checkout, create a store, or add a folder below."
        }
        OriginType::Spec => {
            "Listing registered OpenSpec stores is off. Roots your projects hold or point at still show here."
        }
        OriginType::Freeform => {
            "No freeform origins yet. Add any folder of markdown below."
        }
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

pub(super) fn diagnostic(d: &Diagnostic, t: &ThemeColors, cx: &App) -> AnyElement {
    let color = match d.severity {
        Severity::Error => t.error,
        Severity::Warning => t.warning,
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
    fn library_fingerprint(cx: &App) -> String {
        let s = &settings_entity(cx).read(cx).settings;
        format!("{:?}|{}", s.active_space().library, s.active_space)
    }

    /// Read what the daemon discovers, with each origin's local sync state.
    /// Cheap and local — no network.
    fn refresh_library_origins(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.action_client.clone() else {
            return;
        };
        self.library.fetched_for = Some(Self::library_fingerprint(cx));
        self.library.loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::LibraryOrigins)
                    .and_then(|v| v.ok_or_else(|| "Missing library origins".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<LibraryOrigins>(v)
                            .map_err(|e| format!("Unexpected library origins: {e}"))
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.library.loading = false;
                    match result {
                        Ok(origins) => this.library.origins = Some(origins),
                        Err(e) => this.library.error = Some(e),
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
    fn refresh_library_origins_soon(&mut self, cx: &mut Context<Self>) {
        self.library.refresh_pending = true;
        cx.spawn(async move |this, cx| {
            smol::Timer::after(std::time::Duration::from_millis(500)).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.library.refresh_pending = false;
                    this.refresh_library_origins(cx);
                });
            });
        })
        .detach();
    }

    /// Run an origin change on the daemon, then say what happened and re-read.
    fn run_library_action(
        &mut self,
        action: ActionRequest,
        describe: impl Fn(&serde_json::Value) -> String + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.library.busy {
            return;
        }
        let Some(client) = self.action_client.clone() else {
            self.library.error = Some("The local daemon is unavailable.".into());
            cx.notify();
            return;
        };
        self.library.busy = true;
        self.library.error = None;
        self.library.notice = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || client.post_action(action)).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.library.busy = false;
                    match result {
                        Ok(v) => {
                            this.library.notice =
                                Some(describe(&v.unwrap_or(serde_json::Value::Null)));
                            this.library.add.clear(cx);
                            this.refresh_library_origins(cx);
                        }
                        Err(e) => this.library.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Send whatever the shared form is asking for, or say what is missing.
    fn submit_add_origin(&mut self, cx: &mut Context<Self>) {
        let origin_type = self.library.add.origin_type();
        let mode = self.library.add.mode;
        match self.library.add.request(cx) {
            Ok(action) => {
                self.run_library_action(action, move |v| describe(origin_type, mode, v), cx)
            }
            Err(missing) => {
                self.library.error = Some(missing);
                cx.notify();
            }
        }
    }

    /// Register the OpenSpec checkout at `path`, from the "Register" button on
    /// a root okena found but nobody has registered with OpenSpec.
    fn register_spec_store(&mut self, path: String, cx: &mut Context<Self>) {
        self.run_library_action(
            ActionRequest::LibraryStoreRegister {
                origin_type: OriginType::Spec,
                path,
                id: None,
            },
            |v| describe(OriginType::Spec, AddMode::Register, v),
            cx,
        );
    }

    /// Show a folder as a spec origin without registering it with OpenSpec.
    fn add_spec_folder(&mut self, cx: &mut Context<Self>) {
        let value = self
            .library
            .spec_folder_input
            .read(cx)
            .value()
            .trim()
            .to_string();
        if value.is_empty() {
            return;
        }
        let mut folders = settings_entity(cx)
            .read(cx)
            .settings
            .active_space()
            .spec_folders();
        folders.push(value);
        settings_entity(cx).update(cx, |state, cx| state.set_spec_folders(folders, cx));
        self.library
            .spec_folder_input
            .update(cx, |i, cx| i.set_value("", cx));
    }

    fn library_button(
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
            .when(self.library.busy, |d| d.opacity(0.6))
            .text_size(ui_text_ms(cx))
            .child(label.to_string())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| on_click(this, cx)),
            )
            .into_any_element()
    }

    /// The buttons one origin's row offers.
    fn origin_actions(&self, origin: &LibraryOrigin, cx: &mut Context<Self>) -> Div {
        let mut actions = h_flex().gap(px(6.0)).flex_shrink_0();
        // `defaultStore` is OpenSpec's, and names a registered store.
        if origin.origin_type == OriginType::Spec
            && origin.kind == OriginKind::Store
            && let Some(id) = origin.store_id.clone()
        {
            if origin.is_default {
                actions = actions.child(self.library_button(
                    format!("library-undefault-{id}"),
                    "Clear default",
                    false,
                    |this, cx| {
                        this.run_library_action(
                            ActionRequest::LibrarySetDefaultStore { id: None },
                            |_| "Cleared the machine default store.".into(),
                            cx,
                        )
                    },
                    cx,
                ));
            } else if origin.healthy {
                let target = id.clone();
                actions = actions.child(self.library_button(
                    format!("library-default-{id}"),
                    "Make default",
                    false,
                    move |this, cx| {
                        this.run_library_action(
                            ActionRequest::LibrarySetDefaultStore {
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
        }
        // A spec root okena shows that OpenSpec itself does not know yet.
        if origin.origin_type == OriginType::Spec
            && origin.status.iter().any(|d| d.code == "store_unregistered")
        {
            let path = origin.path.clone();
            actions = actions.child(self.library_button(
                format!("library-register-{}", origin.key),
                "Register",
                true,
                move |this, cx| this.register_spec_store(path.clone(), cx),
                cx,
            ));
        }
        if removable(origin) {
            let key = origin.key.clone();
            actions = actions.child(self.library_button(
                format!("library-remove-{}", origin.key),
                "Remove",
                false,
                move |this, cx| {
                    this.run_library_action(
                        ActionRequest::LibraryStoreUnregister { root: key.clone() },
                        |v| {
                            format!(
                                "Removed '{}'. Its folder is still at {}.",
                                v["id"].as_str().unwrap_or(""),
                                v["left_on_disk"].as_str().unwrap_or("")
                            )
                        },
                        cx,
                    )
                },
                cx,
            ));
        }
        actions
    }

    fn render_origin_row(
        &self,
        origin: &LibraryOrigin,
        border_top: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let (label, color) = health(origin, &t);
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
                            .child(origin.name.clone()),
                    )
                    .when_some(
                        origin.store_id.clone().filter(|id| *id != origin.name),
                        |d, id| d.child(badge(&id, t.border_active, cx)),
                    )
                    .child(badge(origin.kind.label(), t.text_muted, cx))
                    .when(origin.builtin, |d| {
                        d.child(badge("okena's own", t.text_muted, cx))
                    })
                    .when(origin.is_default, |d| {
                        d.child(badge("default", t.border_active, cx))
                    })
                    .child(badge(label, color, cx)),
            )
            .child(path_line(origin.path.clone(), &t, cx))
            .when_some(origin.description.clone(), |d, description| {
                d.child(muted(description, &t, cx))
            })
            .when_some(origin.remote.clone(), |d, remote| {
                d.child(muted(format!("Remote: {remote}"), &t, cx))
            })
            .when_some(origin.git.as_ref().map(git_line), |d, line| {
                d.child(muted(line, &t, cx))
            })
            .when_some(holds_line(origin), |d, line| d.child(muted(line, &t, cx)))
            .when(!origin.used_by.is_empty(), |d| {
                d.child(muted(
                    format!("Used by: {}", origin.used_by.join(", ")),
                    &t,
                    cx,
                ))
            });
        if !origin.references.is_empty() {
            body = body.child(muted("References:", &t, cx));
            for r in &origin.references {
                body = body.child(reference(r, &t, cx));
            }
        }
        for d in &origin.status {
            body = body.child(diagnostic(d, &t, cx));
        }

        h_flex()
            .items_start()
            .gap(px(12.0))
            .px(px(12.0))
            .py(px(10.0))
            .when(border_top, |d| d.border_t_1().border_color(rgb(t.border)))
            .child(body)
            .child(self.origin_actions(origin, cx))
            .into_any_element()
    }

    fn render_pointer_row(
        &self,
        pointer: &LibraryPointer,
        border_top: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let resolved = pointer.root_key.is_some();
        let target = if pointer.store_id.is_empty() {
            "?".to_string()
        } else {
            pointer.store_id.clone()
        };
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
                            .child(match pointer.origin_type {
                                OriginType::Spec => {
                                    format!("{} points at {target}", pointer.project)
                                }
                                _ => format!("{} follows {target}", pointer.project),
                            }),
                    )
                    .when(resolved, |d| {
                        d.child(badge("on this machine", t.success, cx))
                    }),
            )
            .child(path_line(pointer.path.clone(), &t, cx))
            .children(pointer.status.iter().map(|d| diagnostic(d, &t, cx)))
            .into_any_element()
    }

    fn render_add_origin(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        section_container(&t)
            .child(render_add_root(
                &self.library.add,
                AddRootChrome {
                    id_prefix: "library",
                    busy: self.library.busy,
                },
                cx,
                cx.listener(|this, origin_type: &OriginType, _window, cx| {
                    this.library.add.set_origin_type(*origin_type, cx);
                    this.library.error = None;
                    cx.notify();
                }),
                cx.listener(|this, mode: &AddMode, _window, cx| {
                    this.library.add.mode = *mode;
                    this.library.error = None;
                    cx.notify();
                }),
                cx.listener(|this, _, _window, cx| {
                    this.library.add.init_git = !this.library.add.init_git;
                    cx.notify();
                }),
                cx.listener(|this, _, _window, cx| this.submit_add_origin(cx)),
            ))
            .into_any_element()
    }

    pub(super) fn render_library(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        // Read on first paint rather than when the panel opens, so a user who
        // never visits this page makes no call; re-read when discovery changes.
        let fingerprint = Self::library_fingerprint(cx);
        if self.action_client.is_some()
            && !self.library.loading
            && !self.library.refresh_pending
            && self.library.fetched_for.as_deref() != Some(fingerprint.as_str())
        {
            if self.library.fetched_for.is_none() {
                self.refresh_library_origins(cx);
            } else {
                self.refresh_library_origins_soon(cx);
            }
        }

        let config = settings_entity(cx)
            .read(cx)
            .settings
            .active_space()
            .library
            .clone();
        let origins = self.library.origins.clone();
        let daemon = self.action_client.is_some();

        let mut page = v_flex();

        // Sent here to add an origin: the form leads, so it is on screen
        // without scrolling past the origins you already have.
        if self.library.add_first {
            page = page
                .child(section_header("Add an origin", &t, cx))
                .child(self.render_add_origin(cx));
        }

        // ── Overview ───────────────────────────────────────────────────────
        // Takes the row's free width; without `flex_1` the column shrinks to
        // its narrowest content and the paths wrap a character per line.
        let mut facts = v_flex()
            .flex_1()
            .min_w_0()
            .gap(px(2.0))
            .px(px(12.0))
            .py(px(8.0));
        if let Some(o) = &origins {
            facts = facts
                .child(path_line(
                    format!("Knowledge registry: {}", o.knowledge_registry_path),
                    &t,
                    cx,
                ))
                .child(path_line(
                    format!("OpenSpec registry: {}", o.spec_registry_path),
                    &t,
                    cx,
                ))
                .child(path_line(
                    format!("OpenSpec config: {}", o.spec_config_path),
                    &t,
                    cx,
                ))
                .child(muted(
                    format!(
                        "OpenSpec machine default store: {}",
                        o.spec_default_store.as_deref().unwrap_or("none")
                    ),
                    &t,
                    cx,
                ));
        }
        page = page.child(section_header("Library", &t, cx)).child(
            section_container(&t).child(
                h_flex()
                    .items_end()
                    .justify_between()
                    .border_t_1()
                    .border_color(rgb(t.border))
                    .child(facts)
                    .child(div().flex_shrink_0().px(px(12.0)).py(px(8.0)).child(
                        self.library_button(
                            "library-refresh".into(),
                            if self.library.loading {
                                "Reading…"
                            } else {
                                "Refresh"
                            },
                            false,
                            |this, cx| this.refresh_library_origins(cx),
                            cx,
                        ),
                    )),
            ),
        );
        if let Some(notice) = self.library.notice.clone() {
            page = page.child(banner(notice, t.success, cx));
        }
        if let Some(error) = self.library.error.clone() {
            page = page.child(banner(error, t.error, cx));
        }
        if let Some(o) = &origins {
            for d in &o.status {
                page = page.child(div().mx(px(12.0)).mt(px(8.0)).child(diagnostic(d, &t, cx)));
            }
        }

        // ── The origins, a list per type ───────────────────────────────────
        for origin_type in OriginType::all() {
            page = page.child(section_header(
                crate::views::harness::group_label(origin_type),
                &t,
                cx,
            ));
            let mut container = section_container(&t);
            let Some(all) = &origins else {
                page = page.child(container.child(muted_row(
                    if daemon {
                        "Reading origins…"
                    } else {
                        "The local daemon is unavailable, so origins cannot be read."
                    },
                    &t,
                    cx,
                )));
                continue;
            };
            let of_type: Vec<&LibraryOrigin> = all.of_type(origin_type).collect();
            let pointers: Vec<&LibraryPointer> = all
                .pointers
                .iter()
                .filter(|p| p.origin_type == origin_type)
                .collect();
            if of_type.is_empty() && pointers.is_empty() {
                container = container.child(muted_row(
                    empty_text(origin_type, config.spec.registry),
                    &t,
                    cx,
                ));
            }
            let mut first = true;
            for origin in of_type {
                container = container.child(self.render_origin_row(origin, !first, cx));
                first = false;
            }
            // What the space's projects follow or point at, found or not.
            for pointer in pointers {
                container = container.child(self.render_pointer_row(pointer, !first, cx));
                first = false;
            }
            page = page.child(container);
        }

        if !self.library.add_first {
            page = page
                .child(section_header("Add an origin", &t, cx))
                .child(self.render_add_origin(cx));
        }

        // ── Discovery ──────────────────────────────────────────────────────
        page = page.child(section_header("Discovery", &t, cx)).child(
            section_container(&t)
                .child(self.render_toggle(
                    "library-discover-knowledge-projects",
                    "Find knowledge in projects",
                    config.knowledge.projects,
                    true,
                    |state, val, cx| state.set_knowledge_discovery_projects(val, cx),
                    cx,
                ))
                .child(self.render_toggle(
                    "library-discover-spec-registry",
                    "List every registered OpenSpec store",
                    config.spec.registry,
                    true,
                    |state, val, cx| state.set_spec_discovery_registry(val, cx),
                    cx,
                ))
                .child(self.render_toggle(
                    "library-discover-spec-projects",
                    "Find OpenSpec roots and store pointers in projects",
                    config.spec.projects,
                    true,
                    |state, val, cx| state.set_spec_discovery_projects(val, cx),
                    cx,
                )),
        );

        // ── A spec folder that is not a store ──────────────────────────────
        page = page
            .child(section_header("OpenSpec folders", &t, cx))
            .child(
                section_container(&t)
                    .child(muted_row(
                        "Show a folder that holds — or will hold — an openspec/ tree without \
                         registering it with OpenSpec. It is listed under Specs above, and \
                         removed there.",
                        &t,
                        cx,
                    ))
                    .child(
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
                                        SimpleInput::new(&self.library.spec_folder_input)
                                            .text_size(ui_text(13.0, cx)),
                                    ),
                            )
                            .child(self.library_button(
                                "library-add-spec-folder".into(),
                                "Add folder",
                                true,
                                |this, cx| this.add_spec_folder(cx),
                                cx,
                            )),
                    ),
            );

        // ── Where clones go ────────────────────────────────────────────────
        page = page.child(section_header("Clone folders", &t, cx)).child(
            section_container(&t)
                .child(hook_input_row(
                    "library-knowledge-clone-dir",
                    "Knowledge",
                    "Where a knowledge store is cloned when no destination is given. Empty is ~/knowledge.",
                    &self.library.knowledge_clone_dir_input,
                    &t,
                    true,
                    cx,
                ))
                .child(hook_input_row(
                    "library-spec-clone-dir",
                    "Specs",
                    "Where an OpenSpec store is cloned when no destination is given. Empty is ~/openspec, the CLI's convention.",
                    &self.library.spec_clone_dir_input,
                    &t,
                    true,
                    cx,
                ))
                .child(hook_input_row(
                    "library-freeform-clone-dir",
                    "Freeform",
                    "Where a freeform origin is cloned when no destination is given. Empty is ~/library.",
                    &self.library.freeform_clone_dir_input,
                    &t,
                    false,
                    cx,
                )),
        );

        // ── OpenSpec directories ───────────────────────────────────────────
        page = page
            .child(section_header("OpenSpec directories", &t, cx))
            .child(
                section_container(&t)
                    .child(hook_input_row(
                        "library-spec-data-dir",
                        "Data directory",
                        "Where OpenSpec keeps stores/registry.yaml. Empty resolves it like the CLI: \
                         $XDG_DATA_HOME/openspec, else ~/.local/share/openspec. Set it if your shell \
                         exports XDG_DATA_HOME — okena started from the dock never sees it.",
                        &self.library.data_dir_input,
                        &t,
                        true,
                        cx,
                    ))
                    .child(hook_input_row(
                        "library-spec-config-dir",
                        "Config directory",
                        "Where OpenSpec keeps config.json and its defaultStore. Empty resolves \
                         $XDG_CONFIG_HOME/openspec, else ~/.config/openspec.",
                        &self.library.config_dir_input,
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
    use super::{empty_text, git_line, holds_line, removable};
    use okena_core::knowledge::KnowledgeCounts;
    use okena_core::library::{LibraryOrigin, OriginKind, OriginType};
    use okena_core::store_git::StoreGitStatus;

    fn origin(key: &str) -> LibraryOrigin {
        let (origin_type, _) = okena_core::library::split_key(key).expect("a library key");
        serde_json::from_value(serde_json::json!({
            "key": key, "type": origin_type.slug(), "kind": "store", "name": key,
            "path": "/o", "healthy": true,
        }))
        .expect("origin")
    }

    #[test]
    fn the_git_line_names_the_branch_and_only_what_needs_doing() {
        let tracked = StoreGitStatus {
            branch: Some("main".into()),
            upstream: Some("origin/main".into()),
            ..Default::default()
        };
        assert_eq!(git_line(&tracked), "main → origin/main");
        assert_eq!(
            git_line(&StoreGitStatus {
                behind: 3,
                ahead: 1,
                dirty: true,
                ..tracked.clone()
            }),
            "main → origin/main · 3 to pull · 1 to push · uncommitted changes"
        );
        assert_eq!(
            git_line(&StoreGitStatus {
                upstream: None,
                ..tracked
            }),
            "main, no upstream"
        );
        assert_eq!(git_line(&StoreGitStatus::default()), "detached HEAD");
    }

    #[test]
    fn each_type_says_what_it_holds_in_its_own_words() {
        let knowledge = LibraryOrigin {
            counts: Some(KnowledgeCounts {
                docs: 2,
                skills: 0,
                agents: 1,
                templates: 0,
            }),
            ..origin("knowledge:store:eng")
        };
        assert_eq!(
            holds_line(&knowledge).as_deref(),
            Some("2 docs · 0 skills · 1 agent · 0 templates")
        );
        // Counts of a store nobody can read would be zeros that look like a
        // fact.
        let broken = LibraryOrigin {
            healthy: false,
            ..knowledge
        };
        assert_eq!(holds_line(&broken), None);

        let spec = LibraryOrigin {
            schema: Some("spec-driven".into()),
            ..origin("spec:store:plans")
        };
        assert_eq!(holds_line(&spec).as_deref(), Some("Schema: spec-driven"));
        let freeform = LibraryOrigin {
            documents: Some(1),
            ..origin("freeform:path:/notes")
        };
        assert_eq!(holds_line(&freeform).as_deref(), Some("1 document"));
    }

    #[test]
    fn a_project_origin_and_okenas_own_store_have_no_remove() {
        assert!(removable(&origin("knowledge:store:eng")));
        assert!(removable(&LibraryOrigin {
            kind: OriginKind::Folder,
            ..origin("freeform:path:/notes")
        }));
        assert!(!removable(&LibraryOrigin {
            kind: OriginKind::Project,
            ..origin("spec:path:/repo")
        }));
        assert!(!removable(&LibraryOrigin {
            builtin: true,
            ..origin("knowledge:store:okena-defaults")
        }));
    }

    #[test]
    fn an_empty_type_says_how_to_fill_it() {
        for origin_type in OriginType::all() {
            assert!(!empty_text(origin_type, true).is_empty());
        }
        // With the registry switch off, an empty Specs list is expected, and
        // says why rather than suggesting nothing is registered.
        assert!(empty_text(OriginType::Spec, false).contains("is off"));
    }
}
