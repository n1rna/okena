//! Adding a knowledge or specs root: one form, wherever you add one.
//!
//! Roots have always been added in Settings → Knowledge and Settings → Specs.
//! QBL-429 adds a second place — the Roots page the sidebars' `+` opens — and
//! those forms stay. So the questions, the wording, the validation and the
//! action each choice sends live here, and both places render this rather than
//! their own copy: adding a root means the same thing and asks the same things
//! wherever you do it, and a change to one is a change to both.
//!
//! What stays with the caller is what genuinely differs: who holds the busy
//! flag, where the notice and the error are shown, and what else is on the
//! page around the form.

use crate::theme::{ThemeColors, theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_ms};
use crate::views::components::simple_input::{SimpleInput, SimpleInputState};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use std::rc::Rc;

/// Which section's roots are being added to.
///
/// Knowledge and specs roots are different things in different registries, but
/// they are added the same three ways, so the kind is a parameter rather than
/// two near-identical forms.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RootKind {
    Knowledge,
    Specs,
}

impl RootKind {
    fn thing(self) -> &'static str {
        match self {
            RootKind::Knowledge => "knowledge",
            RootKind::Specs => "specs",
        }
    }
}

/// The three ways to add a root. Knowledge and Specs offer the same choices
/// under the same labels, so the words live here rather than in each page
/// (QBL-415).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AddMode {
    Clone,
    Register,
    Create,
}

impl AddMode {
    /// In the order they are offered: cloning is what most teams do.
    pub const ALL: [AddMode; 3] = [AddMode::Clone, AddMode::Register, AddMode::Create];

    pub fn label(self) -> &'static str {
        match self {
            AddMode::Clone => "Clone a repository",
            AddMode::Register => "Add an existing folder",
            AddMode::Create => "Create a new store",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            AddMode::Clone => "clone",
            AddMode::Register => "register",
            AddMode::Create => "create",
        }
    }
}

/// The form's state: which choice is showing, and every box it can ask for.
///
/// All the boxes exist whichever choice is showing, so switching choices and
/// switching back does not lose what was typed.
pub struct AddRootForm {
    kind: RootKind,
    pub mode: AddMode,
    pub init_git: bool,
    clone_url: Entity<SimpleInputState>,
    clone_path: Entity<SimpleInputState>,
    register_path: Entity<SimpleInputState>,
    /// Specs only: a plain OpenSpec root has no identity to take an id from.
    register_id: Entity<SimpleInputState>,
    setup_id: Entity<SimpleInputState>,
    /// Knowledge only: specs stores carry no display name.
    setup_name: Entity<SimpleInputState>,
    setup_path: Entity<SimpleInputState>,
    setup_remote: Entity<SimpleInputState>,
}

fn input<V: 'static>(cx: &mut Context<V>, placeholder: &'static str) -> Entity<SimpleInputState> {
    cx.new(|cx| SimpleInputState::new(cx).placeholder(placeholder))
}

impl AddRootForm {
    pub fn new<V: 'static>(kind: RootKind, cx: &mut Context<V>) -> Self {
        let knowledge = kind == RootKind::Knowledge;
        Self {
            kind,
            mode: AddMode::Clone,
            init_git: true,
            clone_url: input(
                cx,
                if knowledge {
                    "e.g. git@github.com:acme/eng-knowledge.git"
                } else {
                    "e.g. git@github.com:acme/team-plans.git"
                },
            ),
            clone_path: input(cx, "Leave blank to use the clone folder in Settings"),
            register_path: input(
                cx,
                if knowledge {
                    "e.g. ~/knowledge/eng-knowledge"
                } else {
                    "e.g. ~/openspec/team-plans"
                },
            ),
            register_id: input(cx, "Taken from the store's identity"),
            setup_id: input(cx, if knowledge { "e.g. acme-eng" } else { "e.g. team-plans" }),
            setup_name: input(cx, "e.g. Acme Engineering"),
            setup_path: input(
                cx,
                if knowledge {
                    "e.g. ~/knowledge/acme-eng"
                } else {
                    "e.g. ~/openspec/team-plans"
                },
            ),
            setup_remote: input(
                cx,
                if knowledge {
                    "e.g. git@github.com:acme/eng-knowledge.git"
                } else {
                    "e.g. git@github.com:acme/team-plans.git"
                },
            ),
        }
    }

    pub fn kind(&self) -> RootKind {
        self.kind
    }

    fn value(&self, field: &Entity<SimpleInputState>, cx: &App) -> String {
        field.read(cx).value().trim().to_string()
    }

    /// Empty every box. Called once an add has been accepted, so the form does
    /// not still hold the URL of the store you just cloned.
    pub fn clear(&self, cx: &mut App) {
        for field in [
            &self.clone_url,
            &self.clone_path,
            &self.register_path,
            &self.register_id,
            &self.setup_id,
            &self.setup_name,
            &self.setup_path,
            &self.setup_remote,
        ] {
            field.update(cx, |i, cx| i.set_value("", cx));
        }
    }

    /// The action this form submits, or the one line saying what is still
    /// missing.
    ///
    /// Pure apart from reading the boxes, so the rule "a new store needs an id
    /// and a folder" is a thing that can be tested rather than a branch buried
    /// in a click handler.
    pub fn request(&self, cx: &App) -> Result<ActionRequest, String> {
        let some = |v: String| (!v.is_empty()).then_some(v);
        match (self.kind, self.mode) {
            (_, AddMode::Clone) => {
                let url = self.value(&self.clone_url, cx);
                if url.is_empty() {
                    return Err("Enter the repository URL to clone.".into());
                }
                let path = some(self.value(&self.clone_path, cx));
                Ok(match self.kind {
                    RootKind::Knowledge => ActionRequest::KnowledgeStoreClone { url, path },
                    RootKind::Specs => ActionRequest::SpecStoreClone { url, path },
                })
            }
            (_, AddMode::Register) => {
                let path = self.value(&self.register_path, cx);
                if path.is_empty() {
                    return Err("Choose the store checkout's folder.".into());
                }
                Ok(match self.kind {
                    RootKind::Knowledge => ActionRequest::KnowledgeStoreRegister { path },
                    RootKind::Specs => ActionRequest::SpecStoreRegister {
                        path,
                        id: some(self.value(&self.register_id, cx)),
                    },
                })
            }
            (_, AddMode::Create) => {
                let id = self.value(&self.setup_id, cx);
                let path = self.value(&self.setup_path, cx);
                if id.is_empty() || path.is_empty() {
                    return Err("A new store needs an id and a folder.".into());
                }
                let remote = some(self.value(&self.setup_remote, cx));
                Ok(match self.kind {
                    RootKind::Knowledge => ActionRequest::KnowledgeStoreSetup {
                        id,
                        path,
                        name: some(self.value(&self.setup_name, cx)),
                        description: None,
                        remote,
                        init_git: self.init_git,
                    },
                    RootKind::Specs => ActionRequest::SpecStoreSetup {
                        id,
                        path,
                        remote,
                        init_git: self.init_git,
                    },
                })
            }
        }
    }
}

/// What to say once the daemon has accepted an add.
///
/// Here rather than at each call site because the interesting cases are about
/// what the daemon reported — an identity it had to write, a checkout already
/// registered — and both places have to say the same thing about them.
pub fn describe(kind: RootKind, mode: AddMode, v: &serde_json::Value) -> String {
    let id = v["id"].as_str().unwrap_or("store");
    let root = v["root"].as_str().unwrap_or("");
    let flag = |key: &str| v[key].as_bool() == Some(true);
    match (kind, mode) {
        (RootKind::Knowledge, AddMode::Clone) => {
            if flag("identity_missing") {
                format!(
                    "Cloned '{id}' into {root}. It has no .okena-knowledge/store.yaml yet — commit one so every clone agrees on its id."
                )
            } else {
                format!("Cloned '{id}' into {root}.")
            }
        }
        (RootKind::Specs, AddMode::Clone) => {
            let root = v["root"].as_str().unwrap_or("the destination");
            if flag("metadata_created") {
                format!(
                    "Cloned '{id}' into {root}. okena wrote .openspec-store/store.yaml — commit it so every clone carries the same id."
                )
            } else {
                format!("Cloned store '{id}' into {root}.")
            }
        }
        (RootKind::Knowledge, AddMode::Register) => {
            if flag("already_registered") {
                format!("'{id}' was already added from that folder.")
            } else if flag("identity_missing") {
                format!(
                    "Added '{id}', named after its folder. Commit a .okena-knowledge/store.yaml so every clone agrees on the id."
                )
            } else {
                format!("Added store '{id}'.")
            }
        }
        (RootKind::Specs, AddMode::Register) => {
            if flag("metadata_created") {
                format!(
                    "Registered '{id}'. okena wrote .openspec-store/store.yaml — commit it so every clone carries the same id."
                )
            } else if flag("already_registered") {
                format!("'{id}' was already registered at that folder.")
            } else {
                format!("Registered store '{id}'.")
            }
        }
        (kind, AddMode::Create) => {
            let committed = if flag("committed") {
                " with an initial commit"
            } else {
                ""
            };
            let then = match kind {
                RootKind::Knowledge => "Push it where your team can clone it.",
                RootKind::Specs => {
                    "Push it where teammates can clone it; each registers their own checkout."
                }
            };
            format!("Created store '{id}' at {root}{committed}. {then}")
        }
    }
}

// ─── Rendering ──────────────────────────────────────────────────────────────

/// One pill in a row of choices — the add-root modes, and the git toggle that
/// looks like them.
fn pill(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    selected: bool,
    always_lit: bool,
    t: &ThemeColors,
    cx: &App,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id.into())
        .cursor_pointer()
        .px(px(10.0))
        .py(px(3.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(rgb(if selected { t.border_active } else { t.border }))
        .when(selected, |d| {
            d.bg(with_alpha(t.button_primary_bg, 0.15))
        })
        .text_size(ui_text_ms(cx))
        .text_color(rgb(if selected || always_lit {
            t.text_primary
        } else {
            t.text_secondary
        }))
        .child(label.into())
        .on_mouse_down(MouseButton::Left, on_click)
        .into_any_element()
}

/// A label, a box and a line of hint under it.
fn field_row(
    label: &str,
    hint: &str,
    field: &Entity<SimpleInputState>,
    t: &ThemeColors,
    cx: &App,
) -> AnyElement {
    v_flex()
        .gap(px(4.0))
        .child(
            div()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_secondary))
                .child(label.to_string()),
        )
        .child(
            okena_ui::input::input_container(t, None)
                .w_full()
                .px(px(8.0))
                .py(px(5.0))
                .child(SimpleInput::new(field).text_size(ui_text(13.0, cx))),
        )
        .when(!hint.is_empty(), |d| {
            d.child(
                div()
                    .min_w_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(hint.to_string()),
            )
        })
        .into_any_element()
}

fn submit_button(
    id: impl Into<SharedString>,
    label: &str,
    busy: bool,
    t: &ThemeColors,
    cx: &App,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id.into())
        .cursor_pointer()
        .flex_shrink_0()
        .px(px(10.0))
        .py(px(3.0))
        .rounded(px(4.0))
        .bg(rgb(t.button_primary_bg))
        .hover(|s| s.bg(rgb(t.button_primary_hover)))
        .text_color(rgb(t.button_primary_fg))
        .when(busy, |d| d.opacity(0.6))
        .text_size(ui_text_ms(cx))
        .child(label.to_string())
        .on_mouse_down(MouseButton::Left, on_click)
        .into_any_element()
}

/// What the caller has to hand over that the form cannot know.
pub struct AddRootChrome {
    /// Prefixes every element id, so two of these can be on screen at once
    /// without colliding.
    pub id_prefix: &'static str,
    /// An add is already running: the submit button dims and the caller
    /// ignores the click.
    pub busy: bool,
}

/// The whole form: the three choices, the boxes the chosen one needs, and its
/// submit button.
///
/// The callbacks are `&mut App` closures rather than anything view-shaped, so
/// this renders inside the settings panel and inside a harness pane without
/// knowing about either. Callers build them with `cx.listener`.
pub fn render_add_root(
    form: &AddRootForm,
    chrome: AddRootChrome,
    cx: &App,
    on_mode: impl Fn(&AddMode, &mut Window, &mut App) + 'static,
    on_init_git: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_submit: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let t = theme(cx);
    let prefix = chrome.id_prefix;
    let busy = chrome.busy;
    let knowledge = form.kind == RootKind::Knowledge;
    let on_mode = Rc::new(on_mode);
    let on_submit = Rc::new(on_submit);

    let tabs = h_flex().gap(px(6.0)).flex_wrap().children(
        AddMode::ALL.map(|mode| {
            let on_mode = on_mode.clone();
            pill(
                format!("{prefix}-mode-{}", mode.slug()),
                mode.label(),
                form.mode == mode,
                false,
                &t,
                cx,
                move |_, window, app| on_mode(&mode, window, app),
            )
        }),
    );

    let submit = |label: &str| {
        let on_submit = on_submit.clone();
        h_flex().child(submit_button(
            format!("{prefix}-{}-submit", form.mode.slug()),
            label,
            busy,
            &t,
            cx,
            move |event, window, app| on_submit(event, window, app),
        ))
    };

    let body = match form.mode {
        AddMode::Clone => v_flex()
            .gap(px(10.0))
            .child(field_row(
                "Repository URL",
                "Git runs without prompts, so credentials come from an SSH agent or credential helper.",
                &form.clone_url,
                &t,
                cx,
            ))
            .child(field_row(
                "Destination (optional)",
                "",
                &form.clone_path,
                &t,
                cx,
            ))
            .child(submit(if busy { "Cloning…" } else { "Clone" })),
        AddMode::Register => v_flex()
            .gap(px(10.0))
            .child(field_row(
                "Folder",
                if knowledge {
                    "The top of the checkout, with docs/, skills/, agents/ or templates/ in it."
                } else {
                    "The top of the checkout, with an openspec/ tree in it."
                },
                &form.register_path,
                &t,
                cx,
            ))
            .when(!knowledge, |d| {
                d.child(field_row(
                    "Store id (optional)",
                    "Only for a plain OpenSpec root; a store brings its own id.",
                    &form.register_id,
                    &t,
                    cx,
                ))
            })
            .child(submit(if busy { "Adding…" } else { "Add" })),
        AddMode::Create => v_flex()
            .gap(px(10.0))
            .child(field_row(
                "Store id",
                if knowledge {
                    "Kebab-case. Projects follow it with `stores: [acme-eng]` in .okena/knowledge.yaml."
                } else {
                    "Kebab-case. Repos point at it with `store: team-plans` in openspec/config.yaml."
                },
                &form.setup_id,
                &t,
                cx,
            ))
            .when(knowledge, |d| {
                d.child(field_row("Name (optional)", "", &form.setup_name, &t, cx))
            })
            .child(field_row(
                "Folder",
                "An empty folder outside any other git repository.",
                &form.setup_path,
                &t,
                cx,
            ))
            .child(field_row(
                "Remote (optional)",
                "",
                &form.setup_remote,
                &t,
                cx,
            ))
            .child(h_flex().child(pill(
                format!("{prefix}-init-git"),
                if form.init_git {
                    "✓ Initialize Git with an initial commit"
                } else {
                    "Initialize Git with an initial commit"
                },
                form.init_git,
                true,
                &t,
                cx,
                on_init_git,
            )))
            .child(submit(if busy { "Creating…" } else { "Create store" })),
    };

    v_flex()
        .px(px(12.0))
        .py(px(10.0))
        .gap(px(12.0))
        .child(tabs)
        .child(body)
}

/// One line naming what a failed add was trying to do, for an error banner.
pub fn failed_to(kind: RootKind, mode: AddMode) -> String {
    let verb = match mode {
        AddMode::Clone => "clone",
        AddMode::Register => "add",
        AddMode::Create => "create",
    };
    format!("Could not {verb} the {} root", kind.thing())
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{AddMode, RootKind, describe};
    use serde_json::json;

    #[test]
    fn a_finished_add_says_what_the_daemon_reported() {
        // The cases worth saying anything about are the ones where the daemon
        // had to do something the user did not ask for, or nothing at all.
        let cloned = describe(
            RootKind::Knowledge,
            AddMode::Clone,
            &json!({"id": "acme-eng", "root": "/k/acme", "identity_missing": true}),
        );
        assert!(cloned.starts_with("Cloned 'acme-eng' into /k/acme."), "{cloned}");
        assert!(cloned.contains("store.yaml"), "{cloned}");
        assert_eq!(
            describe(
                RootKind::Knowledge,
                AddMode::Clone,
                &json!({"id": "acme-eng", "root": "/k/acme"})
            ),
            "Cloned 'acme-eng' into /k/acme."
        );

        assert_eq!(
            describe(
                RootKind::Knowledge,
                AddMode::Register,
                &json!({"id": "acme-eng", "already_registered": true})
            ),
            "'acme-eng' was already added from that folder."
        );
        // A specs store okena had to write an identity into says so, and says
        // it before "already registered": the commit is the actionable part.
        let specs = describe(
            RootKind::Specs,
            AddMode::Register,
            &json!({"id": "team-plans", "metadata_created": true, "already_registered": true}),
        );
        assert!(specs.contains("okena wrote .openspec-store/store.yaml"), "{specs}");

        let created = describe(
            RootKind::Specs,
            AddMode::Create,
            &json!({"id": "team-plans", "root": "/o/tp", "committed": true}),
        );
        assert_eq!(
            created,
            "Created store 'team-plans' at /o/tp with an initial commit. \
             Push it where teammates can clone it; each registers their own checkout."
        );
        assert!(
            !describe(
                RootKind::Knowledge,
                AddMode::Create,
                &json!({"id": "acme", "root": "/k/a"})
            )
            .contains("initial commit"),
            "a store created without git must not claim a commit"
        );
    }

    #[test]
    fn a_failed_add_names_what_it_was_doing() {
        use super::failed_to;
        assert_eq!(
            failed_to(RootKind::Knowledge, AddMode::Clone),
            "Could not clone the knowledge root"
        );
        assert_eq!(
            failed_to(RootKind::Specs, AddMode::Create),
            "Could not create the specs root"
        );
    }
}
