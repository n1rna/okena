//! The document editor the Specs and Knowledge views share.
//!
//! gpui-component's code editor over the file's source: rope-backed, with
//! tree-sitter highlighting, undo, soft wrap, IME and find — none of which
//! `SimpleInput` has. Markdown toggles between the source and today's preview;
//! any other text file is only ever the source.
//!
//! Saving goes through the daemon (`SpecWrite` / `KnowledgeWrite`), which
//! checks the path exactly as a read does and refuses a file that changed on
//! disk since it was read. A document with unsaved edits keeps its buffer while
//! another one is open, so clicking around never throws work away.

use crate::keybindings::SaveDocument;
use crate::theme::theme;
use crate::ui::tokens::{ui_text, ui_text_ms};
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use gpui_component::input::{Input, InputEvent, InputState};
use okena_core::api::ActionRequest;
use std::collections::HashMap;
use std::sync::Arc;

use super::markdown::MarkdownCache;
use super::{HarnessPane, HarnessSection};

/// Key context around the editor. `SaveDocument` is bound in it, so `cmd-s`
/// means nothing anywhere a document is not being edited.
pub const EDITOR_CONTEXT: &str = "HarnessEditor";

/// The editor's highlighting language for `path`. `text` where no grammar is
/// enabled.
pub(crate) fn language_for(path: &str) -> &'static str {
    let ext = std::path::Path::new(path)
        .extension()
        .map(|x| x.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "md" | "markdown" => "markdown",
        "yaml" | "yml" => "yaml",
        "sh" | "bash" | "zsh" => "bash",
        "json" => "json",
        "toml" => "toml",
        _ => "text",
    }
}

/// Whether `path` has a preview to toggle to.
pub(crate) fn is_markdown(path: &str) -> bool {
    language_for(path) == "markdown"
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditorMode {
    Edit,
    Preview,
}

/// One opened file: what is on disk, and the buffer being edited over it.
pub(crate) struct DocumentBuffer {
    pub(crate) path: String,
    /// The file as last read or saved.
    saved: String,
    /// `saved`'s revision, handed back with a write.
    revision: String,
    /// Made on the first frame that shows the document: an input needs a
    /// window, and the read that fills it lands without one.
    input: Option<Entity<InputState>>,
    pub(crate) mode: EditorMode,
    pub(crate) dirty: bool,
    pub(crate) saving: bool,
    pub(crate) save_error: Option<String>,
    preview: MarkdownCache,
}

impl DocumentBuffer {
    pub(crate) fn new(path: String, content: String, revision: String) -> Self {
        // Markdown opens as it reads; there is nothing else to show a script as.
        let mode = if is_markdown(&path) {
            EditorMode::Preview
        } else {
            EditorMode::Edit
        };
        Self {
            path,
            saved: content,
            revision,
            input: None,
            mode,
            dirty: false,
            saving: false,
            save_error: None,
            preview: MarkdownCache::default(),
        }
    }

    /// Whether the editor, rather than the preview, is showing.
    pub(crate) fn editing(&self) -> bool {
        self.mode == EditorMode::Edit || !is_markdown(&self.path)
    }

    /// The buffer's text: the input's once it exists, else the file's.
    fn text(&self, cx: &App) -> SharedString {
        match &self.input {
            Some(input) => input.read(cx).value(),
            None => self.saved.clone().into(),
        }
    }
}

/// Every buffer a view holds, keyed by root key and path: the open document,
/// plus any other with unsaved edits.
#[derive(Default)]
pub(crate) struct Documents {
    buffers: HashMap<(String, String), DocumentBuffer>,
}

impl Documents {
    fn key(root: &str, path: &str) -> (String, String) {
        (root.to_string(), path.to_string())
    }

    pub(crate) fn get(&self, root: &str, path: &str) -> Option<&DocumentBuffer> {
        self.buffers.get(&Self::key(root, path))
    }

    pub(crate) fn get_mut(&mut self, root: &str, path: &str) -> Option<&mut DocumentBuffer> {
        self.buffers.get_mut(&Self::key(root, path))
    }

    pub(crate) fn insert(&mut self, root: &str, buffer: DocumentBuffer) {
        self.buffers.insert(Self::key(root, &buffer.path), buffer);
    }

    pub(crate) fn remove(&mut self, root: &str, path: &str) {
        self.buffers.remove(&Self::key(root, path));
    }

    /// Stop showing `path`: its buffer goes, unless it holds edits that would
    /// be lost with it.
    pub(crate) fn leave(&mut self, root: &str, path: &str) {
        if !self.is_dirty(root, path) {
            self.remove(root, path);
        }
    }

    pub(crate) fn is_dirty(&self, root: &str, path: &str) -> bool {
        self.get(root, path).is_some_and(|b| b.dirty)
    }
}

impl HarnessPane {
    fn documents(&self, section: HarnessSection) -> Option<&Documents> {
        match section {
            HarnessSection::Specs => Some(&self.specs.documents),
            HarnessSection::Knowledge => Some(&self.knowledge.documents),
            HarnessSection::Tasks => None,
        }
    }

    fn documents_mut(&mut self, section: HarnessSection) -> Option<&mut Documents> {
        match section {
            HarnessSection::Specs => Some(&mut self.specs.documents),
            HarnessSection::Knowledge => Some(&mut self.knowledge.documents),
            HarnessSection::Tasks => None,
        }
    }

    /// Root key and path of the document `section` is showing.
    fn open_document_key(&self, section: HarnessSection) -> Option<(String, String)> {
        let (root, selected) = match section {
            HarnessSection::Specs => (&self.specs.root_key, &self.specs.selected),
            HarnessSection::Knowledge => (&self.knowledge.root_key, &self.knowledge.selected),
            HarnessSection::Tasks => return None,
        };
        Some((root.clone().unwrap_or_default(), selected.clone()?))
    }

    /// The document `section` is showing, once it has loaded.
    pub(super) fn open_buffer(&self, section: HarnessSection) -> Option<&DocumentBuffer> {
        let (root, path) = self.open_document_key(section)?;
        self.documents(section)?.get(&root, &path)
    }

    /// Give the open document its input, the first frame it is shown.
    pub(super) fn ensure_document_input(
        &mut self,
        section: HarnessSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((root, path)) = self.open_document_key(section) else {
            return;
        };
        let Some(buffer) = self
            .documents_mut(section)
            .and_then(|d| d.get_mut(&root, &path))
        else {
            return;
        };
        sync_editor_colors(cx);
        if buffer.input.is_some() {
            return;
        }
        let saved = buffer.saved.clone();
        let language = language_for(&path);
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor(language)
                // Prose, not source: no gutter to fold or number lines in.
                .line_number(false)
                .folding(false)
                .soft_wrap(true)
                .default_value(saved)
        });
        buffer.input = Some(input.clone());
        cx.subscribe(
            &input,
            move |this: &mut Self, input, event: &InputEvent, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                let value = input.read(cx).value();
                if let Some(buffer) = this
                    .documents_mut(section)
                    .and_then(|d| d.get_mut(&root, &path))
                {
                    let dirty = buffer.saved.as_str() != value.as_ref();
                    if dirty != buffer.dirty {
                        buffer.dirty = dirty;
                        cx.notify();
                    }
                }
            },
        )
        .detach();
    }

    /// Write the open document's buffer back to its file.
    pub(super) fn save_document(&mut self, section: HarnessSection, cx: &mut Context<Self>) {
        let Some((root, path)) = self.open_document_key(section) else {
            return;
        };
        let Some(buffer) = self
            .documents_mut(section)
            .and_then(|d| d.get_mut(&root, &path))
        else {
            return;
        };
        if buffer.saving {
            return;
        }
        let content = buffer.text(cx).to_string();
        if content == buffer.saved {
            return;
        }
        buffer.saving = true;
        buffer.save_error = None;
        let revision = buffer.revision.clone();
        cx.notify();

        let root_arg = (!root.is_empty()).then(|| root.clone());
        let action = match section {
            HarnessSection::Specs => ActionRequest::SpecWrite {
                root: root_arg,
                path: path.clone(),
                content: content.clone(),
                revision,
            },
            HarnessSection::Knowledge => ActionRequest::KnowledgeWrite {
                root: root_arg,
                path: path.clone(),
                content: content.clone(),
                revision,
            },
            HarnessSection::Tasks => return,
        };
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(action)
                    .and_then(|v| v.ok_or_else(|| "Missing save result".to_string()))
                    .and_then(|v| {
                        v.get("revision")
                            .and_then(|r| r.as_str())
                            .map(str::to_string)
                            .ok_or_else(|| "Unexpected save result".to_string())
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    let saved = result.is_ok();
                    // Reverted while the write was in flight: nothing to update.
                    let Some(buffer) = this
                        .documents_mut(section)
                        .and_then(|d| d.get_mut(&root, &path))
                    else {
                        return;
                    };
                    buffer.saving = false;
                    match result {
                        Ok(revision) => {
                            buffer.dirty = buffer.text(cx).as_ref() != content.as_str();
                            buffer.saved = content;
                            buffer.revision = revision;
                        }
                        Err(e) => buffer.save_error = Some(e),
                    }
                    // A knowledge entry's title, description and tags come
                    // from its frontmatter, which the save may have changed.
                    if saved && section == HarnessSection::Knowledge {
                        this.refresh_knowledge(cx);
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Drop the open document's edits and read the file again.
    fn revert_document(&mut self, section: HarnessSection, cx: &mut Context<Self>) {
        let Some((root, path)) = self.open_document_key(section) else {
            return;
        };
        if let Some(documents) = self.documents_mut(section) {
            documents.remove(&root, &path);
        }
        match section {
            HarnessSection::Specs => self.open_spec_doc(path, cx),
            HarnessSection::Knowledge => self.open_knowledge_file(path, cx),
            HarnessSection::Tasks => {}
        }
    }

    fn set_document_mode(
        &mut self,
        section: HarnessSection,
        mode: EditorMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((root, path)) = self.open_document_key(section) else {
            return;
        };
        let Some(buffer) = self
            .documents_mut(section)
            .and_then(|d| d.get_mut(&root, &path))
        else {
            return;
        };
        buffer.mode = mode;
        // Switching to Edit means typing next.
        if mode == EditorMode::Edit
            && let Some(input) = buffer.input.clone()
        {
            input.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    /// Save state, Revert, Save and the Edit/Preview toggle, for the right of
    /// the document header.
    pub(super) fn render_document_controls(
        &self,
        section: HarnessSection,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(buffer) = self.open_buffer(section) else {
            return Vec::new();
        };
        let t = theme(cx);
        let ids = match section {
            HarnessSection::Knowledge => ["knowledge-doc-revert", "knowledge-doc-save"],
            _ => ["spec-doc-revert", "spec-doc-save"],
        };
        let mut controls = Vec::new();
        let state = if buffer.saving {
            Some(("Saving…", t.text_muted))
        } else if buffer.dirty {
            Some(("Unsaved", t.warning))
        } else {
            None
        };
        if let Some((label, color)) = state {
            controls.push(
                div()
                    .flex_shrink_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(color))
                    .child(label)
                    .into_any_element(),
            );
        }
        if (buffer.dirty || buffer.save_error.is_some()) && !buffer.saving {
            controls.push(self.small_button(
                ids[0],
                "Revert",
                cx.listener(move |this, _, _window, cx| this.revert_document(section, cx)),
                cx,
            ));
        }
        if buffer.dirty && !buffer.saving {
            controls.push(self.small_button(
                ids[1],
                "Save",
                cx.listener(move |this, _, _window, cx| this.save_document(section, cx)),
                cx,
            ));
        }
        if is_markdown(&buffer.path) {
            controls.push(self.render_mode_toggle(section, buffer.mode, cx));
        }
        controls
    }

    fn render_mode_toggle(
        &self,
        section: HarnessSection,
        current: EditorMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let segment = |mode: EditorMode, label: &'static str, cx: &mut Context<Self>| {
            let selected = mode == current;
            div()
                .id(SharedString::from(format!("{section:?}-doc-{label}")))
                .cursor_pointer()
                .px(px(10.0))
                .py(px(3.0))
                .rounded(px(3.0))
                .text_size(ui_text_ms(cx))
                .when(selected, |d| {
                    d.bg(rgb(t.bg_hover)).text_color(rgb(t.text_primary))
                })
                .when(!selected, |d| {
                    d.text_color(rgb(t.text_muted))
                        .hover(|s| s.text_color(rgb(t.text_primary)))
                })
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        this.set_document_mode(section, mode, window, cx);
                    }),
                )
        };
        h_flex()
            .flex_shrink_0()
            .p(px(2.0))
            .gap(px(2.0))
            .rounded(px(4.0))
            .bg(rgb(t.bg_secondary))
            .child(segment(EditorMode::Edit, "Edit", cx))
            .child(segment(EditorMode::Preview, "Preview", cx))
            .into_any_element()
    }

    /// The editor filling the document column. `None` until the input exists,
    /// which is the frame after the read lands.
    pub(super) fn render_document_editor(
        &self,
        section: HarnessSection,
        buffer: &DocumentBuffer,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let input = buffer.input.as_ref()?;
        let t = theme(cx);
        Some(
            div()
                .id(SharedString::from(format!("{section:?}-document-editor")))
                .key_context(EDITOR_CONTEXT)
                .on_action(cx.listener(move |this, _: &SaveDocument, _window, cx| {
                    this.save_document(section, cx);
                }))
                .flex_1()
                .min_h_0()
                .w_full()
                .child(
                    Input::new(input)
                        .appearance(false)
                        .h_full()
                        .px(px(20.0))
                        .py(px(14.0))
                        .text_size(ui_text(13.0, cx))
                        .text_color(rgb(t.text_primary)),
                )
                .into_any_element(),
        )
    }

    /// The buffer, edits included, as formatted Markdown.
    pub(super) fn render_document_preview(
        &self,
        buffer: &DocumentBuffer,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let text = buffer.text(cx);
        let doc = buffer.preview.get(&text, theme(cx).is_dark());
        self.render_markdown_blocks(&doc, cx)
    }

    /// `●` before a tree row whose document holds unsaved edits.
    pub(super) fn unsaved_marker(
        &self,
        section: HarnessSection,
        path: &str,
    ) -> Option<&'static str> {
        let root = match section {
            HarnessSection::Specs => self.specs.root_key.as_deref(),
            HarnessSection::Knowledge => self.knowledge.root_key.as_deref(),
            HarnessSection::Tasks => return None,
        };
        self.documents(section)?
            .is_dirty(root.unwrap_or_default(), path)
            .then_some("● ")
    }
}

/// Paint the editor in okena's theme rather than gpui-component's.
///
/// Its caret, selection and background come from gpui-component's global
/// theme, which okena only ever switches between light and dark. The editor is
/// the only gpui-component input okena draws, so taking those colours over is
/// safe; only what differs is written, so a frame that changes nothing does not
/// mark the global changed.
fn sync_editor_colors(cx: &mut App) {
    let t = theme(cx);
    let caret: Hsla = rgb(t.cursor).into();
    let selection: Hsla = rgb(t.bg_selection).into();
    let background: Hsla = rgb(t.bg_primary).into();
    let foreground: Hsla = rgb(t.text_primary).into();

    let current = gpui_component::Theme::global(cx);
    let style = &current.highlight_theme.style;
    let up_to_date = current.caret == caret
        && current.selection == selection
        && style.editor_background == Some(background)
        && style.editor_foreground == Some(foreground)
        && style.editor_active_line.is_none();
    if up_to_date {
        return;
    }
    let global = gpui_component::Theme::global_mut(cx);
    global.caret = caret;
    global.selection = selection;
    let mut highlight = (*global.highlight_theme).clone();
    highlight.style.editor_background = Some(background);
    highlight.style.editor_foreground = Some(foreground);
    highlight.style.editor_active_line = None;
    global.highlight_theme = Arc::new(highlight);
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{DocumentBuffer, Documents, EditorMode, is_markdown, language_for};

    #[test]
    fn files_get_the_grammar_their_extension_names() {
        assert_eq!(language_for("openspec/changes/x/proposal.md"), "markdown");
        assert_eq!(language_for("docs/README.MD"), "markdown");
        assert_eq!(language_for("openspec/changes/x/.openspec.yaml"), "yaml");
        assert_eq!(language_for("skills/release/run.sh"), "bash");
        assert_eq!(language_for("templates/x.json"), "json");
        // No grammar: still editable, just unhighlighted.
        assert_eq!(language_for("Makefile"), "text");
        assert!(is_markdown("a.md") && !is_markdown("a.yaml"));
    }

    #[test]
    fn markdown_opens_in_preview_and_everything_else_in_the_editor() {
        let md = DocumentBuffer::new("a.md".into(), "# A".into(), "r".into());
        assert_eq!(md.mode, EditorMode::Preview);
        assert!(!md.editing());
        let sh = DocumentBuffer::new("run.sh".into(), "echo".into(), "r".into());
        assert!(sh.editing());
    }

    #[test]
    fn leaving_a_document_keeps_it_only_while_it_has_unsaved_edits() {
        let mut docs = Documents::default();
        docs.insert(
            "store:eng",
            DocumentBuffer::new("a.md".into(), "a".into(), "r".into()),
        );
        docs.insert(
            "store:eng",
            DocumentBuffer::new("b.md".into(), "b".into(), "r".into()),
        );
        docs.get_mut("store:eng", "b.md").unwrap().dirty = true;

        docs.leave("store:eng", "a.md");
        docs.leave("store:eng", "b.md");
        assert!(docs.get("store:eng", "a.md").is_none());
        assert!(docs.is_dirty("store:eng", "b.md"));
        // The same path in another root is another document.
        assert!(!docs.is_dirty("store:other", "b.md"));
    }
}
