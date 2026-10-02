//! Creating, renaming and deleting files in a Library origin's tree.
//!
//! Plain file management beside the editor: a "New" form in the tree, and
//! Rename and Delete for the open document. Every operation goes through the
//! daemon (`LibraryFileCreate` … `LibraryFileDelete`), whatever the origin's
//! type: it checks each path the way a read does and never replaces an
//! existing file. What differs by type is only where a new item goes and what
//! it starts with ([`plan_new`]). The view re-lists
//! its tree after each one, so the result shows without a refresh. The
//! agent-driven flows — drafting a change, "New with agent" — sit beside this
//! and are unchanged.
//!
//! Empty folders are not shown as such: every tree lists files. The one
//! exception is a spec change, which is a folder by definition — deleting its
//! last document leaves it listed with "no artifacts yet".

use crate::theme::theme;
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms};
use crate::views::components::{SimpleInput, SimpleInputState};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::knowledge::KnowledgeKind;
use okena_core::specs::change_slug;
use okena_ui::simple_input::InputChangedEvent;

use super::editor::{DocumentBuffer, EditorMode};
use super::HarnessPane;

/// What the "New" form makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NewItem {
    /// A knowledge entry of this kind.
    Knowledge(KnowledgeKind),
    /// A change directory under `openspec/changes/`.
    SpecChange,
    /// A capability: `openspec/specs/<name>/spec.md`.
    SpecCapability,
    /// A document inside the change at this root-relative path.
    SpecDocument { change: String },
    /// A markdown file anywhere in a freeform origin.
    Freeform,
}

impl NewItem {
    fn title(&self) -> String {
        match self {
            NewItem::Knowledge(_) => "New entry".to_string(),
            NewItem::SpecChange => "New change".to_string(),
            NewItem::SpecCapability => "New spec".to_string(),
            NewItem::SpecDocument { change } => format!(
                "New document in {}",
                change.rsplit('/').next().unwrap_or(change)
            ),
            NewItem::Freeform => "New document".to_string(),
        }
    }

    fn placeholder(&self) -> &'static str {
        match self {
            NewItem::Knowledge(KnowledgeKind::Doc) => "ci/pipeline",
            NewItem::Knowledge(KnowledgeKind::Skill) => "release",
            NewItem::Knowledge(KnowledgeKind::Agent) => "reviewer",
            NewItem::Knowledge(KnowledgeKind::Template) => "task-start",
            NewItem::SpecChange => "add-login",
            NewItem::SpecCapability => "auth",
            NewItem::SpecDocument { .. } => "design.md",
            NewItem::Freeform => "workflows/release",
        }
    }
}

/// What a "New" form asks the daemon for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PlannedItem {
    /// Relative to the root.
    pub(crate) path: String,
    /// A folder rather than a file.
    pub(crate) folder: bool,
    /// What a new file starts with.
    pub(crate) content: String,
}

fn kind_name(kind: KnowledgeKind) -> &'static str {
    match kind {
        KnowledgeKind::Doc => "Doc",
        KnowledgeKind::Skill => "Skill",
        KnowledgeKind::Agent => "Agent",
        KnowledgeKind::Template => "Template",
    }
}

/// Where a new item named `name` goes, and what it starts with.
///
/// Knowledge entries start from their kind's frontmatter, as
/// `docs/reference/knowledge.md` defines it, with the `description` left for
/// the author: it is what people and agents pick an entry by. The daemon checks
/// the path again; this only turns a name into one.
pub(crate) fn plan_new(item: &NewItem, name: &str) -> Result<PlannedItem, String> {
    let name = name.trim().trim_matches('/');
    if name.is_empty() {
        return Err("Name it first.".into());
    }
    let stem = name.strip_suffix(".md").unwrap_or(name);
    let leaf = stem.rsplit('/').next().unwrap_or(stem);
    let file = |path: String, content: String| PlannedItem {
        path,
        folder: false,
        content,
    };
    let planned = match item {
        NewItem::Knowledge(KnowledgeKind::Doc) => file(
            format!("docs/{stem}.md"),
            format!("---\ntitle: {leaf}\ndescription:\ntags: []\n---\n\n# {leaf}\n"),
        ),
        NewItem::Knowledge(KnowledgeKind::Skill) => file(
            format!("skills/{stem}/SKILL.md"),
            format!("---\nname: {leaf}\ndescription:\n---\n\n# {leaf}\n"),
        ),
        NewItem::Knowledge(KnowledgeKind::Agent) => file(
            format!("agents/{stem}.md"),
            format!("---\nname: {leaf}\ndescription:\n---\n\n"),
        ),
        NewItem::Knowledge(KnowledgeKind::Template) => file(
            format!("templates/{stem}.md"),
            "---\nfor: []\ndescription:\n---\n\n".to_string(),
        ),
        NewItem::SpecChange => {
            let slug = change_slug(name);
            if slug.is_empty() {
                return Err("A change name needs letters or numbers.".into());
            }
            PlannedItem {
                path: format!("openspec/changes/{slug}"),
                folder: true,
                content: String::new(),
            }
        }
        NewItem::SpecCapability => file(
            format!("openspec/specs/{name}/spec.md"),
            format!("# {name} Specification\n\n## Purpose\n\n## Requirements\n"),
        ),
        NewItem::SpecDocument { change } => {
            let named = if std::path::Path::new(name).extension().is_some() {
                name.to_string()
            } else {
                format!("{name}.md")
            };
            file(format!("{change}/{named}"), String::new())
        }
        // No layout to follow: the name is the path, and the file starts with
        // the heading its title is read from.
        NewItem::Freeform => {
            let named = if std::path::Path::new(name).extension().is_some() {
                name.to_string()
            } else {
                format!("{name}.md")
            };
            file(named, format!("# {leaf}\n"))
        }
    };
    okena_core::fs::normalize_relative(&planned.path)?;
    Ok(planned)
}

/// The Library's file operation state.
pub(crate) struct FileOps {
    /// What the "New" form is making, while it is open.
    pub(crate) creating: Option<NewItem>,
    /// Path of the document being renamed, while the rename box is open.
    pub(crate) renaming: Option<String>,
    /// Path whose delete is armed; confirming deletes it.
    pub(crate) pending_delete: Option<String>,
    /// An operation is in flight.
    pub(crate) busy: bool,
    pub(crate) error: Option<String>,
    /// The new item's name, or the renamed file's new path.
    pub(crate) name_input: Entity<SimpleInputState>,
}

impl FileOps {
    pub(crate) fn new(cx: &mut Context<HarnessPane>) -> Self {
        let name_input = cx.new(SimpleInputState::new);
        // The form names the path it will create, so it follows the typing.
        cx.subscribe(
            &name_input,
            |_this: &mut HarnessPane, _, _: &InputChangedEvent, cx| cx.notify(),
        )
        .detach();
        Self {
            creating: None,
            renaming: None,
            pending_delete: None,
            busy: false,
            error: None,
            name_input,
        }
    }

    fn close(&mut self) {
        self.creating = None;
        self.renaming = None;
        self.pending_delete = None;
        self.error = None;
    }
}

// ─── Actions ────────────────────────────────────────────────────────────────

impl HarnessPane {
    /// Open the "New" form for `item`, focused on the name.
    pub(super) fn open_new_form(
        &mut self,
        item: NewItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let files = &mut self.file_ops;
        files.close();
        let placeholder = item.placeholder();
        files.creating = Some(item);
        let input = files.name_input.clone();
        input.update(cx, |input, cx| {
            input.set_placeholder(placeholder);
            input.set_value("", cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn cancel_file_op(&mut self, cx: &mut Context<Self>) {
        self.file_ops.close();
        cx.notify();
    }

    /// Enter in the name box: create, or rename, whichever is open.
    fn submit_file_op(&mut self, cx: &mut Context<Self>) {
        if self.file_ops.creating.is_some() {
            self.submit_new(cx);
        } else if self.file_ops.renaming.is_some() {
            self.submit_rename(cx);
        }
    }

    /// Post `action`; on success close the form and run `on_done` with the
    /// reply. The tree is re-listed either way: a refusal is often a sign that
    /// what is shown is stale.
    fn run_file_action(
        &mut self,
        action: ActionRequest,
        on_done: impl FnOnce(&mut Self, serde_json::Value) + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.file_ops.busy {
            return;
        }
        self.file_ops.busy = true;
        self.file_ops.error = None;
        cx.notify();

        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(action)
                    .and_then(|v| v.ok_or_else(|| "Missing result".to_string()))
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(reply) => {
                            this.file_ops.busy = false;
                            this.file_ops.close();
                            on_done(this, reply);
                        }
                        Err(e) => {
                            this.file_ops.busy = false;
                            this.file_ops.error = Some(e);
                        }
                    }
                    this.refresh_library(cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn submit_new(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.file_ops.creating.clone() else {
            return;
        };
        let name = self.file_ops.name_input.read(cx).value().to_string();
        let planned = match plan_new(&item, &name) {
            Ok(p) => p,
            Err(e) => {
                self.file_ops.error = Some(e);
                cx.notify();
                return;
            }
        };
        let root = self.library.root_key.clone();
        let path = planned.path.clone();
        let action = if planned.folder {
            ActionRequest::LibraryFolderCreate { root, path }
        } else {
            ActionRequest::LibraryFileCreate {
                root,
                path,
                content: planned.content.clone(),
            }
        };
        self.run_file_action(
            action,
            move |this, reply| {
                let path = reply
                    .get("path")
                    .and_then(|p| p.as_str())
                    .unwrap_or(&planned.path)
                    .to_string();
                if planned.folder {
                    // A new change opens expanded: its documents go there next.
                    if let Some(name) = path.rsplit('/').next() {
                        this.library.collapsed.remove(name);
                    }
                    return;
                }
                let revision = reply
                    .get("revision")
                    .and_then(|r| r.as_str())
                    .unwrap_or_default()
                    .to_string();
                // Straight into the editor: a new file is for writing in.
                let mut buffer = DocumentBuffer::new(path, planned.content, revision);
                buffer.mode = EditorMode::Edit;
                this.show_new_document(buffer);
            },
            cx,
        );
    }

    /// Select a freshly created file, with its buffer already in place.
    fn show_new_document(&mut self, buffer: DocumentBuffer) {
        let path = buffer.path.clone();
        self.library.leave_selection();
        let root = self.library.root_key.clone().unwrap_or_default();
        self.library.documents.insert(&root, buffer);
        self.library.selected = Some(path);
    }

    fn start_rename(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.library.selected.clone() else {
            return;
        };
        let files = &mut self.file_ops;
        files.close();
        files.renaming = Some(path.clone());
        let input = files.name_input.clone();
        input.update(cx, |input, cx| {
            input.set_placeholder("New path");
            input.set_value(path, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn submit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(from) = self.file_ops.renaming.clone() else {
            return;
        };
        let to = self.file_ops.name_input.read(cx).value().trim().to_string();
        if to.is_empty() || to == from {
            self.cancel_file_op(cx);
            return;
        }
        let action = ActionRequest::LibraryFileRename {
            root: self.library.root_key.clone(),
            from: from.clone(),
            to,
        };
        self.run_file_action(
            action,
            move |this, reply| {
                let Some(to) = reply.get("path").and_then(|p| p.as_str()) else {
                    return;
                };
                let to = to.to_string();
                // The open document follows the file, unsaved edits and all.
                let root = this.library.root_key.clone().unwrap_or_default();
                this.library.documents.rename(&root, &from, &to);
                if this.library.selected.as_deref() == Some(from.as_str()) {
                    this.library.selected = Some(to);
                }
            },
            cx,
        );
    }

    fn arm_delete(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.library.selected.clone() else {
            return;
        };
        self.file_ops.close();
        self.file_ops.pending_delete = Some(path);
        cx.notify();
    }

    fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.file_ops.pending_delete.clone() else {
            return;
        };
        let action = ActionRequest::LibraryFileDelete {
            root: self.library.root_key.clone(),
            path: path.clone(),
        };
        self.run_file_action(
            action,
            move |this, _reply| {
                let root = this.library.root_key.clone().unwrap_or_default();
                this.library.documents.remove(&root, &path);
                if this.library.selected.as_deref() == Some(path.as_str()) {
                    this.library.selected = None;
                    this.library.content_error = None;
                }
            },
            cx,
        );
    }
}

// ─── Rendering ──────────────────────────────────────────────────────────────

impl HarnessPane {
    /// A `+` that opens a "New" form, for a tree heading or a change row.
    pub(super) fn add_button(
        &self,
        id: impl Into<ElementId>,
        tooltip: &'static str,
        on_add: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        div()
            .id(id)
            .cursor_pointer()
            .flex_shrink_0()
            .px(px(5.0))
            .rounded(px(3.0))
            .text_size(ui_text_md(cx))
            .text_color(rgb(t.text_muted))
            .hover(|s| s.bg(rgb(t.bg_hover)).text_color(rgb(t.text_primary)))
            .child("+")
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(tooltip).build(window, cx)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    // Inside a disclosure row, the click must not also fold it.
                    cx.stop_propagation();
                    on_add(this, window, cx);
                }),
            )
            .into_any_element()
    }

    /// A tree section heading with a `+` beside it.
    pub(super) fn tree_heading_with_add(
        &self,
        label: &str,
        id: &'static str,
        tooltip: &'static str,
        on_add: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        h_flex()
            .w_full()
            .items_center()
            .px(px(6.0))
            .pt(px(10.0))
            .pb(px(3.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(label.to_uppercase()),
            )
            .child(self.add_button(id, tooltip, on_add, cx))
            .into_any_element()
    }

    /// The name box, with Enter to submit and Escape to cancel.
    fn render_name_field(&self, cx: &mut Context<Self>) -> AnyElement {
        let files = &self.file_ops;
        let t = theme(cx);
        div()
            .id("library-file-name")
            .w_full()
            .child(
                okena_ui::input::input_container(&t, None)
                    .w_full()
                    .px(px(8.0))
                    .py(px(4.0))
                    .child(SimpleInput::new(&files.name_input).text_size(ui_text(13.0, cx))),
            )
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _window, cx| {
                // Typing belongs to the box, not to the pane's shortcuts.
                cx.stop_propagation();
                match event.keystroke.key.as_str() {
                    "enter" => this.submit_file_op(cx),
                    "escape" => this.cancel_file_op(cx),
                    _ => {}
                }
            }))
            .into_any_element()
    }

    fn file_note(
        &self,
        text: impl Into<SharedString>,
        color: u32,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(color))
            .child(text.into())
            .into_any_element()
    }

    fn danger_button(
        &self,
        id: &'static str,
        label: &str,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        div()
            .id(id)
            .cursor_pointer()
            .flex_shrink_0()
            .px(px(10.0))
            .py(px(3.0))
            .rounded(px(4.0))
            .bg(rgb(t.error))
            .text_size(ui_text_md(cx))
            .text_color(rgb(t.button_primary_fg))
            .child(label.to_string())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| on_click(this, cx)),
            )
            .into_any_element()
    }

    /// The "New" form, for the top of a tree while it is open.
    pub(super) fn render_new_form(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let files = &self.file_ops;
        let item = files.creating.clone()?;
        let t = theme(cx);
        let name = files.name_input.read(cx).value().to_string();
        let hint = match plan_new(&item, &name) {
            Ok(p) if p.folder => format!("Creates {}/", p.path),
            Ok(p) => format!("Creates {}", p.path),
            Err(_) => "Type a name; a path like ci/pipeline makes folders.".to_string(),
        };
        let busy = files.busy;
        let error = files.error.clone();
        let (create_id, cancel_id) = ("library-new-create", "library-new-cancel");

        let mut form = v_flex()
            .gap(px(6.0))
            .mx(px(2.0))
            .my(px(6.0))
            .p(px(8.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_secondary))
            .child(self.file_note(item.title(), t.text_secondary, cx));
        if let NewItem::Knowledge(current) = &item {
            let mut kinds = h_flex().gap(px(4.0)).flex_wrap();
            for kind in KnowledgeKind::all() {
                kinds = kinds.child(self.choice_chip(
                    format!("knowledge-new-kind-{}", kind.folder()),
                    kind_name(kind).to_string(),
                    kind == *current,
                    move |this, _cx| {
                        this.file_ops.creating = Some(NewItem::Knowledge(kind));
                    },
                    cx,
                ));
            }
            form = form.child(kinds);
        }
        form = form
            .child(self.render_name_field(cx))
            .child(self.file_note(hint, t.text_muted, cx));
        if let Some(error) = error {
            form = form.child(self.file_note(error, t.error, cx));
        }
        Some(
            form.child(
                h_flex()
                    .gap(px(6.0))
                    .child(if busy {
                        self.file_note("Creating…", t.text_muted, cx)
                    } else {
                        self.small_button(
                            create_id,
                            "Create",
                            cx.listener(move |this, _, _window, cx| this.submit_new(cx)),
                            cx,
                        )
                    })
                    .child(self.small_button(
                        cancel_id,
                        "Cancel",
                        cx.listener(move |this, _, _window, cx| this.cancel_file_op(cx)),
                        cx,
                    )),
            )
            .into_any_element(),
        )
    }

    /// Rename and Delete for the open document, for the right of its header.
    pub(super) fn render_file_controls(
        &self,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let files = &self.file_ops;
        let open = self.open_buffer();
        // Not while a save is in flight: its reply lands by path. And not
        // before the file has loaded, when there may be nothing to act on.
        if files.busy
            || files.renaming.is_some()
            || files.pending_delete.is_some()
            || open.is_none_or(|b| b.saving)
        {
            return Vec::new();
        }
        let ids = ["library-doc-rename", "library-doc-delete"];
        vec![
            self.small_button(
                ids[0],
                "Rename",
                cx.listener(move |this, _, window, cx| this.start_rename(window, cx)),
                cx,
            ),
            self.small_button(
                ids[1],
                "Delete…",
                cx.listener(move |this, _, _window, cx| this.arm_delete(cx)),
                cx,
            ),
        ]
    }

    /// The rename box or the delete confirmation, under the document header.
    pub(super) fn render_file_op_bar(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let files = &self.file_ops;
        let t = theme(cx);
        let busy = files.busy;
        let error = files.error.clone();
        let (go_id, cancel_id) = ("library-file-op-go", "library-file-op-cancel");
        let bar = v_flex()
            .flex_shrink_0()
            .gap(px(6.0))
            .px(px(16.0))
            .py(px(8.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_secondary));

        let (bar, go) = if let Some(from) = files.renaming.clone() {
            (
                bar.child(self.render_name_field(cx))
                    .child(self.file_note(
                        format!(
                            "Move {from} to a new path in this origin. Folders it needs are created."
                        ),
                        t.text_muted,
                        cx,
                    )),
                self.small_button(
                    go_id,
                    "Rename",
                    cx.listener(move |this, _, _window, cx| this.submit_rename(cx)),
                    cx,
                ),
            )
        } else if let Some(path) = files.pending_delete.clone() {
            let unsaved = self.open_buffer().is_some_and(|b| b.dirty);
            (
                bar.child(self.file_note(
                    format!(
                        "Delete {path} from disk?{}",
                        if unsaved {
                            " Its unsaved edits go with it."
                        } else {
                            ""
                        }
                    ),
                    t.text_primary,
                    cx,
                )),
                self.danger_button(
                    go_id,
                    "Delete",
                    move |this, cx| this.confirm_delete(cx),
                    cx,
                ),
            )
        } else {
            return None;
        };

        let mut bar = bar;
        if let Some(error) = error {
            bar = bar.child(self.file_note(error, t.error, cx));
        }
        Some(
            bar.child(
                h_flex()
                    .gap(px(6.0))
                    .child(if busy {
                        self.file_note("Working…", t.text_muted, cx)
                    } else {
                        go
                    })
                    .child(self.small_button(
                        cancel_id,
                        "Cancel",
                        cx.listener(move |this, _, _window, cx| this.cancel_file_op(cx)),
                        cx,
                    )),
            )
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::super::editor::{DocumentBuffer, Documents};
    use super::{NewItem, PlannedItem, plan_new};
    use okena_core::knowledge::KnowledgeKind;

    fn path_of(item: NewItem, name: &str) -> (String, bool) {
        let PlannedItem { path, folder, .. } = plan_new(&item, name).expect("planned");
        (path, folder)
    }

    #[test]
    fn each_knowledge_kind_lands_in_its_folder_with_its_frontmatter() {
        assert_eq!(
            path_of(NewItem::Knowledge(KnowledgeKind::Doc), "ci/pipeline.md").0,
            "docs/ci/pipeline.md"
        );
        assert_eq!(
            path_of(NewItem::Knowledge(KnowledgeKind::Skill), "release").0,
            "skills/release/SKILL.md"
        );
        assert_eq!(
            path_of(NewItem::Knowledge(KnowledgeKind::Agent), "reviewer").0,
            "agents/reviewer.md"
        );
        assert_eq!(
            path_of(NewItem::Knowledge(KnowledgeKind::Template), "task-start").0,
            "templates/task-start.md"
        );
        let skill = plan_new(&NewItem::Knowledge(KnowledgeKind::Skill), "ops/release").unwrap();
        assert!(
            skill.content.starts_with("---\nname: release\n"),
            "{}",
            skill.content
        );
        let doc = plan_new(&NewItem::Knowledge(KnowledgeKind::Doc), "ci/pipeline").unwrap();
        assert!(doc.content.contains("title: pipeline"));
    }

    #[test]
    fn spec_items_follow_the_openspec_layout() {
        assert_eq!(
            path_of(NewItem::SpecChange, "Add Login"),
            ("openspec/changes/add-login".to_string(), true)
        );
        assert_eq!(
            path_of(NewItem::SpecCapability, "auth"),
            ("openspec/specs/auth/spec.md".to_string(), false)
        );
        let change = "openspec/changes/add-login".to_string();
        assert_eq!(
            path_of(
                NewItem::SpecDocument {
                    change: change.clone()
                },
                "design"
            )
            .0,
            "openspec/changes/add-login/design.md"
        );
        assert_eq!(
            path_of(NewItem::SpecDocument { change }, "notes.txt").0,
            "openspec/changes/add-login/notes.txt",
            "an extension given is kept"
        );
    }

    #[test]
    fn a_freeform_document_goes_where_its_name_says_and_starts_with_its_heading() {
        // No layout to follow, so nothing is prefixed.
        let doc = plan_new(&NewItem::Freeform, "workflows/release").unwrap();
        assert_eq!(doc.path, "workflows/release.md");
        assert!(!doc.folder);
        assert_eq!(doc.content, "# release\n", "the title the tree will show");
        assert_eq!(
            path_of(NewItem::Freeform, "notes.markdown").0,
            "notes.markdown",
            "an extension given is kept"
        );
        assert!(plan_new(&NewItem::Freeform, "../escape").is_err());
    }

    #[test]
    fn names_the_tree_could_not_list_are_refused_before_posting() {
        assert!(plan_new(&NewItem::SpecChange, "   ").is_err());
        assert!(plan_new(&NewItem::SpecChange, "!!!").is_err());
        assert!(plan_new(&NewItem::Knowledge(KnowledgeKind::Doc), "../escape").is_err());
        assert!(
            plan_new(
                &NewItem::SpecDocument {
                    change: "openspec/changes/x".into()
                },
                ".hidden.md"
            )
            .is_err()
        );
    }

    #[test]
    fn a_renamed_file_keeps_its_buffer_and_edits() {
        let mut docs = Documents::default();
        docs.insert(
            "store:eng",
            DocumentBuffer::new("docs/a.md".into(), "a".into(), "r".into()),
        );
        docs.get_mut("store:eng", "docs/a.md").unwrap().dirty = true;
        docs.rename("store:eng", "docs/a.md", "docs/b.md");
        assert!(docs.get("store:eng", "docs/a.md").is_none());
        let moved = docs.get("store:eng", "docs/b.md").expect("followed");
        assert_eq!(moved.path, "docs/b.md");
        assert!(moved.dirty);
    }
}
