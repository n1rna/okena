//! Adding a Library origin: one form, wherever you add one.
//!
//! Origins are added in two places — Settings → Library, and the Origins page
//! the Library's sidebar `+` opens (QBL-429). So the questions, the wording,
//! the validation and the action each choice sends live here, and both places
//! render this rather than their own copy: adding an origin means the same
//! thing and asks the same things wherever you do it, and a change to one is a
//! change to both.
//!
//! The form asks two things: what **type** of origin it is
//! (`okena_core::library::OriginType` — knowledge, spec or freeform), and
//! which of the three ways to add it. The type is a choice on the form rather
//! than a form per type because the three ways are the same for every type
//! (QBL-440); it only changes a few words, which boxes are asked for, and
//! where the origin is recorded.
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
use okena_core::library::OriginType;
use std::rc::Rc;

/// The three ways to add an origin. Every type offers the same choices under
/// the same labels, so the words live here rather than in each page (QBL-415).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AddMode {
    Clone,
    Register,
    Create,
}

impl AddMode {
    /// In the order they are offered: cloning is what most teams do.
    pub const ALL: [AddMode; 3] = [AddMode::Clone, AddMode::Register, AddMode::Create];

    /// What the choice is called for an origin of `origin_type`. A freeform
    /// origin is a folder, not a store.
    pub fn label(self, origin_type: OriginType) -> &'static str {
        match (self, origin_type) {
            (AddMode::Clone, _) => "Clone a repository",
            (AddMode::Register, _) => "Add an existing folder",
            (AddMode::Create, OriginType::Freeform) => "Create a new folder",
            (AddMode::Create, _) => "Create a new store",
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

/// What each box suggests, for an origin of one type.
struct Placeholders {
    clone_url: &'static str,
    folder: &'static str,
    setup_id: &'static str,
    setup_path: &'static str,
}

fn placeholders(origin_type: OriginType) -> Placeholders {
    match origin_type {
        OriginType::Knowledge => Placeholders {
            clone_url: "e.g. git@github.com:acme/eng-knowledge.git",
            folder: "e.g. ~/knowledge/eng-knowledge",
            setup_id: "e.g. acme-eng",
            setup_path: "e.g. ~/knowledge/acme-eng",
        },
        OriginType::Spec => Placeholders {
            clone_url: "e.g. git@github.com:acme/team-plans.git",
            folder: "e.g. ~/openspec/team-plans",
            setup_id: "e.g. team-plans",
            setup_path: "e.g. ~/openspec/team-plans",
        },
        OriginType::Freeform => Placeholders {
            clone_url: "e.g. git@github.com:acme/runbooks.git",
            folder: "e.g. ~/notes/runbooks",
            setup_id: "e.g. Runbooks",
            setup_path: "e.g. ~/library/runbooks",
        },
    }
}

/// The form's state: which choice is showing, and every box it can ask for.
///
/// All the boxes exist whichever choice is showing, so switching choices and
/// switching back does not lose what was typed.
pub struct AddRootForm {
    origin_type: OriginType,
    pub mode: AddMode,
    pub init_git: bool,
    clone_url: Entity<SimpleInputState>,
    clone_path: Entity<SimpleInputState>,
    register_path: Entity<SimpleInputState>,
    /// Spec only: a plain OpenSpec root has no identity to take an id from.
    register_id: Entity<SimpleInputState>,
    /// The new store's id; for a freeform origin, the title its README gets.
    setup_id: Entity<SimpleInputState>,
    /// Knowledge only: spec stores carry no display name.
    setup_name: Entity<SimpleInputState>,
    setup_path: Entity<SimpleInputState>,
    setup_remote: Entity<SimpleInputState>,
}

fn input<V: 'static>(cx: &mut Context<V>, placeholder: &'static str) -> Entity<SimpleInputState> {
    cx.new(|cx| SimpleInputState::new(cx).placeholder(placeholder))
}

impl AddRootForm {
    pub fn new<V: 'static>(origin_type: OriginType, cx: &mut Context<V>) -> Self {
        let hints = placeholders(origin_type);
        Self {
            origin_type,
            mode: AddMode::Clone,
            init_git: true,
            clone_url: input(cx, hints.clone_url),
            clone_path: input(cx, "Leave blank to use the clone folder in Settings"),
            register_path: input(cx, hints.folder),
            register_id: input(cx, "Taken from the store's identity"),
            setup_id: input(cx, hints.setup_id),
            setup_name: input(cx, "e.g. Acme Engineering"),
            setup_path: input(cx, hints.setup_path),
            setup_remote: input(cx, hints.clone_url),
        }
    }

    /// The type of origin the form is adding.
    pub fn origin_type(&self) -> OriginType {
        self.origin_type
    }

    /// Add an origin of another type. What was typed stays: a URL or a folder
    /// is the same thing whichever type you turn out to want.
    pub fn set_origin_type(&mut self, origin_type: OriginType, cx: &mut App) {
        if self.origin_type == origin_type {
            return;
        }
        self.origin_type = origin_type;
        let hints = placeholders(origin_type);
        for (field, hint) in [
            (&self.clone_url, hints.clone_url),
            (&self.register_path, hints.folder),
            (&self.setup_id, hints.setup_id),
            (&self.setup_path, hints.setup_path),
            (&self.setup_remote, hints.clone_url),
        ] {
            field.update(cx, |input, cx| {
                input.set_placeholder(hint);
                cx.notify();
            });
        }
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
    pub fn request(&self, cx: &App) -> Result<ActionRequest, String> {
        build_request(
            self.origin_type,
            self.mode,
            self.init_git,
            &Typed {
                clone_url: self.value(&self.clone_url, cx),
                clone_path: self.value(&self.clone_path, cx),
                register_path: self.value(&self.register_path, cx),
                register_id: self.value(&self.register_id, cx),
                setup_id: self.value(&self.setup_id, cx),
                setup_name: self.value(&self.setup_name, cx),
                setup_path: self.value(&self.setup_path, cx),
                setup_remote: self.value(&self.setup_remote, cx),
            },
        )
    }
}

/// What the boxes hold, trimmed.
#[derive(Clone, Debug, Default)]
pub struct Typed {
    pub clone_url: String,
    pub clone_path: String,
    pub register_path: String,
    pub register_id: String,
    pub setup_id: String,
    pub setup_name: String,
    pub setup_path: String,
    pub setup_remote: String,
}

/// The action that adds an origin of `origin_type` the way `mode` says, from
/// what was typed — or the one line saying what is still missing.
///
/// Apart from the boxes so the rules — "a new store needs an id and a folder",
/// "a freeform origin needs only a folder" — are things that can be tested
/// rather than branches buried in a click handler. The same three actions go
/// out for every type; the type rides along on them.
pub fn build_request(
    origin_type: OriginType,
    mode: AddMode,
    init_git: bool,
    typed: &Typed,
) -> Result<ActionRequest, String> {
    let some = |v: &str| (!v.is_empty()).then(|| v.to_string());
    match mode {
        AddMode::Clone => {
            if typed.clone_url.is_empty() {
                return Err("Enter the repository URL to clone.".into());
            }
            Ok(ActionRequest::LibraryStoreClone {
                origin_type,
                url: typed.clone_url.clone(),
                path: some(&typed.clone_path),
            })
        }
        AddMode::Register => {
            if typed.register_path.is_empty() {
                return Err(match origin_type {
                    OriginType::Freeform => "Choose the folder to add.",
                    _ => "Choose the store checkout's folder.",
                }
                .into());
            }
            Ok(ActionRequest::LibraryStoreRegister {
                origin_type,
                path: typed.register_path.clone(),
                // Only a plain OpenSpec root is given an id here.
                id: some(&typed.register_id).filter(|_| origin_type == OriginType::Spec),
            })
        }
        AddMode::Create => {
            // A freeform origin has no id: it is a folder, and what is typed
            // as its name only titles its README.
            let needs_id = origin_type != OriginType::Freeform;
            if typed.setup_path.is_empty() || (needs_id && typed.setup_id.is_empty()) {
                return Err(if needs_id {
                    "A new store needs an id and a folder."
                } else {
                    "A new origin needs a folder."
                }
                .into());
            }
            Ok(ActionRequest::LibraryStoreSetup {
                origin_type,
                id: typed.setup_id.clone(),
                path: typed.setup_path.clone(),
                name: some(&typed.setup_name).filter(|_| origin_type == OriginType::Knowledge),
                description: None,
                remote: some(&typed.setup_remote).filter(|_| needs_id),
                init_git,
            })
        }
    }
}

/// What to say once the daemon has accepted an add.
///
/// Here rather than at each call site because the interesting cases are about
/// what the daemon reported — an identity it had to write, a checkout already
/// registered — and both places have to say the same thing about them.
pub fn describe(origin_type: OriginType, mode: AddMode, v: &serde_json::Value) -> String {
    let id = v["id"].as_str().unwrap_or("store");
    let root = v["root"].as_str().unwrap_or("");
    let flag = |key: &str| v[key].as_bool() == Some(true);
    match (origin_type, mode) {
        (OriginType::Knowledge, AddMode::Clone) => {
            if flag("identity_missing") {
                format!(
                    "Cloned '{id}' into {root}. It has no .okena-knowledge/store.yaml yet — commit one so every clone agrees on its id."
                )
            } else {
                format!("Cloned '{id}' into {root}.")
            }
        }
        (OriginType::Spec, AddMode::Clone) => {
            let root = v["root"].as_str().unwrap_or("the destination");
            if flag("metadata_created") {
                format!(
                    "Cloned '{id}' into {root}. okena wrote .openspec-store/store.yaml — commit it so every clone carries the same id."
                )
            } else {
                format!("Cloned store '{id}' into {root}.")
            }
        }
        (OriginType::Knowledge, AddMode::Register) => {
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
        (OriginType::Spec, AddMode::Register) => {
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
        // A freeform origin is its folder: there is no id to report, and
        // nothing was written into it.
        (OriginType::Freeform, AddMode::Clone) => {
            format!("Cloned into {root} and added it as a freeform origin.")
        }
        (OriginType::Freeform, AddMode::Register) => {
            if flag("already_registered") {
                format!("{root} was already a freeform origin.")
            } else {
                format!("Added {root} as a freeform origin.")
            }
        }
        (OriginType::Freeform, AddMode::Create) => {
            let committed = if flag("committed") {
                " with an initial commit"
            } else {
                ""
            };
            format!("Created {root}{committed}, with a README to start from.")
        }
        (origin_type, AddMode::Create) => {
            let committed = if flag("committed") {
                " with an initial commit"
            } else {
                ""
            };
            let then = match origin_type {
                OriginType::Spec => {
                    "Push it where teammates can clone it; each registers their own checkout."
                }
                _ => "Push it where your team can clone it.",
            };
            format!("Created store '{id}' at {root}{committed}. {then}")
        }
    }
}

// ─── Rendering ──────────────────────────────────────────────────────────────

/// One pill in a row of choices — the origin types, the ways to add one, and
/// the git toggle that looks like them.
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

/// The whole form: the origin's type, the three ways to add it, the boxes the
/// chosen way needs, and its submit button.
///
/// The callbacks are `&mut App` closures rather than anything view-shaped, so
/// this renders inside the settings panel and inside a harness pane without
/// knowing about either. Callers build them with `cx.listener`.
pub fn render_add_root(
    form: &AddRootForm,
    chrome: AddRootChrome,
    cx: &App,
    on_type: impl Fn(&OriginType, &mut Window, &mut App) + 'static,
    on_mode: impl Fn(&AddMode, &mut Window, &mut App) + 'static,
    on_init_git: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_submit: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let t = theme(cx);
    let prefix = chrome.id_prefix;
    let busy = chrome.busy;
    let origin_type = form.origin_type;
    let knowledge = origin_type == OriginType::Knowledge;
    let freeform = origin_type == OriginType::Freeform;
    let on_type = Rc::new(on_type);
    let on_mode = Rc::new(on_mode);
    let on_submit = Rc::new(on_submit);

    // What is being added comes first: it decides what the rest asks.
    let types = v_flex()
        .gap(px(5.0))
        .child(
            h_flex().gap(px(6.0)).flex_wrap().children(OriginType::all().map(|ty| {
                let on_type = on_type.clone();
                pill(
                    format!("{prefix}-type-{}", ty.slug()),
                    ty.label(),
                    origin_type == ty,
                    false,
                    &t,
                    cx,
                    move |_, window, app| on_type(&ty, window, app),
                )
            })),
        )
        .child(
            div()
                .min_w_0()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(origin_type.blurb()),
        );

    let tabs = h_flex().gap(px(6.0)).flex_wrap().children(
        AddMode::ALL.map(|mode| {
            let on_mode = on_mode.clone();
            pill(
                format!("{prefix}-mode-{}", mode.slug()),
                mode.label(origin_type),
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
                match origin_type {
                    OriginType::Knowledge => {
                        "The top of the checkout, with docs/, skills/, agents/ or templates/ in it."
                    }
                    OriginType::Spec => "The top of the checkout, with an openspec/ tree in it.",
                    OriginType::Freeform => {
                        "Any folder of markdown. Every .md file under it is listed."
                    }
                },
                &form.register_path,
                &t,
                cx,
            ))
            .when(origin_type == OriginType::Spec, |d| {
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
                if freeform { "Name (optional)" } else { "Store id" },
                match origin_type {
                    OriginType::Knowledge => {
                        "Kebab-case. Projects follow it with `stores: [acme-eng]` in .okena/knowledge.yaml."
                    }
                    OriginType::Spec => {
                        "Kebab-case. Repos point at it with `store: team-plans` in openspec/config.yaml."
                    }
                    OriginType::Freeform => "The heading of the README the folder starts with.",
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
            // A store records its clone source in its identity; a folder
            // has no identity to record one in.
            .when(!freeform, |d| {
                d.child(field_row(
                    "Remote (optional)",
                    "",
                    &form.setup_remote,
                    &t,
                    cx,
                ))
            })
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
            .child(submit(if busy {
                "Creating…"
            } else if freeform {
                "Create folder"
            } else {
                "Create store"
            })),
    };

    v_flex()
        .px(px(12.0))
        .py(px(10.0))
        .gap(px(12.0))
        .child(types)
        .child(tabs)
        .child(body)
}

/// One line naming what a failed add was trying to do, for an error banner.
pub fn failed_to(origin_type: OriginType, mode: AddMode) -> String {
    let verb = match mode {
        AddMode::Clone => "clone",
        AddMode::Register => "add",
        AddMode::Create => "create",
    };
    format!("Could not {verb} the {} origin", origin_type.slug())
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{AddMode, Typed, build_request, describe};
    use okena_core::api::ActionRequest;
    use okena_core::library::OriginType;
    use serde_json::json;

    #[test]
    fn a_finished_add_says_what_the_daemon_reported() {
        // The cases worth saying anything about are the ones where the daemon
        // had to do something the user did not ask for, or nothing at all.
        let cloned = describe(
            OriginType::Knowledge,
            AddMode::Clone,
            &json!({"id": "acme-eng", "root": "/k/acme", "identity_missing": true}),
        );
        assert!(cloned.starts_with("Cloned 'acme-eng' into /k/acme."), "{cloned}");
        assert!(cloned.contains("store.yaml"), "{cloned}");
        assert_eq!(
            describe(
                OriginType::Knowledge,
                AddMode::Clone,
                &json!({"id": "acme-eng", "root": "/k/acme"})
            ),
            "Cloned 'acme-eng' into /k/acme."
        );

        assert_eq!(
            describe(
                OriginType::Knowledge,
                AddMode::Register,
                &json!({"id": "acme-eng", "already_registered": true})
            ),
            "'acme-eng' was already added from that folder."
        );
        // A spec store okena had to write an identity into says so, and says
        // it before "already registered": the commit is the actionable part.
        let specs = describe(
            OriginType::Spec,
            AddMode::Register,
            &json!({"id": "team-plans", "metadata_created": true, "already_registered": true}),
        );
        assert!(specs.contains("okena wrote .openspec-store/store.yaml"), "{specs}");

        let created = describe(
            OriginType::Spec,
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
                OriginType::Knowledge,
                AddMode::Create,
                &json!({"id": "acme", "root": "/k/a"})
            )
            .contains("initial commit"),
            "a store created without git must not claim a commit"
        );
    }

    #[test]
    fn a_freeform_origin_is_reported_by_its_folder_since_it_has_no_id() {
        assert_eq!(
            describe(OriginType::Freeform, AddMode::Register, &json!({"root": "/notes"})),
            "Added /notes as a freeform origin."
        );
        assert_eq!(
            describe(
                OriginType::Freeform,
                AddMode::Register,
                &json!({"root": "/notes", "already_registered": true})
            ),
            "/notes was already a freeform origin."
        );
        assert_eq!(
            describe(
                OriginType::Freeform,
                AddMode::Create,
                &json!({"root": "/notes", "committed": true})
            ),
            "Created /notes with an initial commit, with a README to start from."
        );
        assert!(
            describe(OriginType::Freeform, AddMode::Clone, &json!({"root": "/c/runbooks"}))
                .contains("/c/runbooks")
        );
    }

    #[test]
    fn every_type_is_added_by_the_same_three_actions_carrying_its_type() {
        let typed = Typed {
            clone_url: "git@example.com:acme/x.git".into(),
            register_path: "~/x".into(),
            register_id: "plans".into(),
            setup_id: "acme".into(),
            setup_name: "Acme".into(),
            setup_path: "~/new".into(),
            setup_remote: "git@example.com:acme/new.git".into(),
            ..Default::default()
        };
        for origin_type in OriginType::all() {
            assert!(matches!(
                build_request(origin_type, AddMode::Clone, true, &typed),
                Ok(ActionRequest::LibraryStoreClone { origin_type: t, path: None, .. })
                    if t == origin_type
            ));
            let Ok(ActionRequest::LibraryStoreRegister { origin_type: t, id, .. }) =
                build_request(origin_type, AddMode::Register, true, &typed)
            else {
                panic!("expected a register");
            };
            assert_eq!(t, origin_type);
            // Only a plain OpenSpec root is handed an id.
            assert_eq!(id.is_some(), origin_type == OriginType::Spec, "{origin_type:?}");

            let Ok(ActionRequest::LibraryStoreSetup {
                origin_type: t,
                name,
                remote,
                init_git,
                ..
            }) = build_request(origin_type, AddMode::Create, false, &typed)
            else {
                panic!("expected a setup");
            };
            assert_eq!(t, origin_type);
            assert!(!init_git);
            assert_eq!(name.is_some(), origin_type == OriginType::Knowledge);
            assert_eq!(remote.is_some(), origin_type != OriginType::Freeform);
        }
    }

    #[test]
    fn what_is_missing_is_said_before_anything_is_sent() {
        let empty = Typed::default();
        for origin_type in OriginType::all() {
            for mode in AddMode::ALL {
                assert!(
                    build_request(origin_type, mode, true, &empty).is_err(),
                    "{origin_type:?} {mode:?}"
                );
            }
        }
        // A store needs an id and a folder; a freeform origin only a folder.
        let folder_only = Typed {
            setup_path: "~/new".into(),
            ..Default::default()
        };
        assert_eq!(
            build_request(OriginType::Knowledge, AddMode::Create, true, &folder_only)
                .unwrap_err(),
            "A new store needs an id and a folder."
        );
        assert!(build_request(OriginType::Freeform, AddMode::Create, true, &folder_only).is_ok());
    }

    #[test]
    fn creating_is_called_a_folder_for_freeform_and_a_store_otherwise() {
        assert_eq!(AddMode::Create.label(OriginType::Freeform), "Create a new folder");
        assert_eq!(AddMode::Create.label(OriginType::Spec), "Create a new store");
        assert_eq!(AddMode::Clone.label(OriginType::Freeform), "Clone a repository");
    }

    #[test]
    fn a_failed_add_names_what_it_was_doing() {
        use super::failed_to;
        assert_eq!(
            failed_to(OriginType::Knowledge, AddMode::Clone),
            "Could not clone the knowledge origin"
        );
        assert_eq!(
            failed_to(OriginType::Spec, AddMode::Create),
            "Could not create the spec origin"
        );
        assert_eq!(
            failed_to(OriginType::Freeform, AddMode::Register),
            "Could not add the freeform origin"
        );
    }
}
