//! The Library — knowledge, specs and any other markdown, as one page of
//! typed origins (QBL-440).
//!
//! Specs and Knowledge used to be two pages doing one job twice: list some
//! git-backed folders of markdown, show the files of the one you pick, edit
//! them, search them, commit and push them, and hand them to agents. They are
//! one page now. An origin has a type (`okena_core::library::OriginType`),
//! and the type decides only what its files are:
//!
//! - what the tree under the origin list looks like — `library_knowledge.rs`,
//!   `library_spec.rs`, `library_freeform.rs`;
//! - what "New" starts — a knowledge draft, an OpenSpec change, a document;
//! - whether it layers. Only knowledge origins do, so only they have an
//!   order, overrides and `okena-defaults` beneath them.
//!
//! Everything else on the page is the same code for all three: the origin
//! list, loading, opening and saving a document (`editor.rs`), file
//! operations (`file_ops.rs`), the git panel (`store_git.rs`), the search
//! island (`doc_search.rs`) and the Origins page (`roots_page.rs`).
//!
//! Origins are whatever the daemon discovered. Reading, saving, fetching and
//! pulling all go through it, and it refuses origins it did not discover and
//! paths outside one; the client never touches the filesystem.

use crate::theme::{ThemeColors, theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms};
use crate::views::components::SimpleInputState;
use crate::views::components::source_editor::BriefInput;
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::diagnostic::{Diagnostic, Severity};
use okena_core::library::{
    LibraryDocument, LibraryOrigin, LibraryOrigins, LibraryTree, OriginType,
};
use std::collections::HashSet;

use super::HarnessPane;
use super::editor::{DocumentBuffer, Documents};
use super::store_git::{StoreGitPanel, sync_badge};

/// Library-view state.
pub(crate) struct LibraryState {
    /// Every origin the daemon discovered. `None` until the first load lands.
    pub(crate) origins: Option<LibraryOrigins>,
    /// Key of the origin being shown. `None` until the first load picks the
    /// default one.
    pub(crate) root_key: Option<String>,
    /// What the open origin holds, in its type's shape.
    pub(crate) tree: Option<LibraryTree>,
    pub(crate) loading: bool,
    /// Bumped on every load, so a slow response for an origin the user has
    /// since left is dropped instead of replacing the newer one.
    pub(crate) load_generation: u64,
    pub(crate) error: Option<String>,
    /// The open origin's fetch, pull, commit and push.
    pub(crate) git: StoreGitPanel,
    /// Path of the file being read, relative to the origin.
    pub(crate) selected: Option<String>,
    /// The open document's buffer, and any other with unsaved edits.
    pub(crate) documents: Documents,
    pub(crate) content_error: Option<String>,
    /// Folded rows: a knowledge kind (`docs`) or folder (`docs/ci`), a
    /// change's name, a freeform folder. Collapsed rather than expanded
    /// state, so a fresh view shows everything.
    pub(crate) collapsed: HashSet<String>,

    // ── A spec origin's New change form ──
    /// Projects and context for the new change's agent, once its dialog has
    /// been opened.
    pub(crate) pickers: Option<Entity<crate::views::components::launch_pickers::LaunchPickers>>,
    /// The idea a new change is drafted from.
    pub(crate) idea_input: BriefInput,
    /// Whether the document panel is showing the new-change form.
    ///
    /// It stands where a document's text stands rather than taking the whole
    /// view, which hid the tree you were adding to and the specs you are meant
    /// to read before proposing.
    pub(crate) composing: bool,
    /// Directory name for the change being configured. Blank derives one from
    /// the prompt.
    pub(crate) name_input: Entity<SimpleInputState>,
    /// Origin a new change is drafted into. Follows the open one until the
    /// user picks another in the form.
    pub(crate) draft_root: Option<String>,
    pub(crate) drafting: bool,
}

impl LibraryState {
    pub(crate) fn new(cx: &mut Context<HarnessPane>) -> Self {
        Self {
            origins: None,
            root_key: None,
            tree: None,
            loading: false,
            load_generation: 0,
            error: None,
            git: StoreGitPanel::new(cx),
            selected: None,
            documents: Documents::default(),
            content_error: None,
            collapsed: HashSet::new(),
            pickers: None,
            idea_input: BriefInput::new(
                "e.g. let users sign in with Google, alongside the existing email flow",
            ),
            composing: false,
            name_input: cx.new(|cx| SimpleInputState::new(cx).placeholder("add-login")),
            draft_root: None,
            drafting: false,
        }
    }

    /// Stop showing the selected document. Its buffer stays only if it holds
    /// unsaved edits. Call before `root_key` changes: buffers are keyed by it.
    pub(crate) fn leave_selection(&mut self) {
        if let Some(path) = self.selected.take() {
            self.documents
                .leave(self.root_key.as_deref().unwrap_or_default(), &path);
        }
        self.content_error = None;
    }

    /// Whether the currently-loaded tree still lists `path`.
    fn tree_contains(&self, path: &str) -> bool {
        self.tree.as_ref().is_some_and(|t| t.contains(path))
    }

    /// The origin being shown, when it is still discovered.
    pub(crate) fn open_origin(&self) -> Option<&LibraryOrigin> {
        self.origins.as_ref()?.origin(self.root_key.as_deref()?)
    }

    /// The type of the origin being shown: from the origin when it is listed,
    /// else from its key, which says the same thing.
    pub(crate) fn open_type(&self) -> Option<OriginType> {
        self.open_origin().map(|o| o.origin_type).or_else(|| {
            okena_core::library::split_key(self.root_key.as_deref()?).map(|(t, _)| t)
        })
    }
}

// ─── Pure helpers ───────────────────────────────────────────────────────────

/// What the origin list calls each type's group.
pub(crate) fn group_label(origin_type: OriginType) -> &'static str {
    match origin_type {
        OriginType::Knowledge => "Knowledge",
        OriginType::Spec => "Specs",
        OriginType::Freeform => "Freeform",
    }
}

/// A problem, something worth a look, or fine.
pub(crate) fn health_color(origin: &LibraryOrigin, t: &ThemeColors) -> u32 {
    if !origin.healthy {
        t.error
    } else if origin.warned() {
        t.warning
    } else {
        t.success
    }
}

/// How to reach a spec origin from the CLI.
pub(crate) fn cli_hint(origin: &LibraryOrigin) -> String {
    use okena_core::library::OriginKind;
    match (origin.kind, origin.store_id.as_deref()) {
        (OriginKind::Store, Some(id)) => format!("openspec list --store {id}"),
        _ => format!("cd {} && openspec list", origin.path),
    }
}

/// How many entries of each kind a knowledge origin holds, in words.
pub(crate) fn counts_line(c: &okena_core::knowledge::KnowledgeCounts) -> String {
    let plural = |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    [
        plural(c.docs, "doc", "docs"),
        plural(c.skills, "skill", "skills"),
        plural(c.agents, "agent", "agents"),
        plural(c.templates, "template", "templates"),
    ]
    .join(" · ")
}

/// The line under an origin's name saying what it is: its type, where it was
/// found, and whether it is OpenSpec's machine default.
pub(crate) fn origin_caption(origin: &LibraryOrigin) -> String {
    let mut caption = format!(
        "{} · {}",
        origin.origin_type.label().to_lowercase(),
        origin.kind.label()
    );
    if origin.builtin {
        caption.push_str(" · okena's own");
    }
    if origin.is_default {
        caption.push_str(" · machine default");
    }
    caption
}

/// What "New" starts in the open origin, by its type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NewKind {
    /// An agent briefed to write knowledge.
    KnowledgeDraft,
    /// A scaffolded OpenSpec change, and an agent to fill it in.
    SpecChange,
    /// An agent briefed to write documents in the folder. A file of your own
    /// is the tree's `+`.
    FreeformDraft,
}

/// What "New" would start, or `None` where there is nowhere to write.
///
/// The open origin decides. okena's own store cannot be written in, so with
/// it open "New" falls back to another knowledge origin when there is one —
/// the draft form asks which — and is not offered otherwise.
pub(crate) fn new_kind(origins: &LibraryOrigins, open: Option<&LibraryOrigin>) -> Option<NewKind> {
    let drafts_knowledge = || {
        origins
            .of_type(OriginType::Knowledge)
            .any(|o| o.writable())
            .then_some(NewKind::KnowledgeDraft)
    };
    match open {
        Some(origin) if origin.healthy => match origin.origin_type {
            OriginType::Knowledge => drafts_knowledge(),
            OriginType::Spec => Some(NewKind::SpecChange),
            OriginType::Freeform => Some(NewKind::FreeformDraft),
        },
        _ => drafts_knowledge(),
    }
}

type Loaded = (
    LibraryOrigins,
    Option<String>,
    Option<Result<LibraryTree, String>>,
);

// ─── Loading and actions ────────────────────────────────────────────────────

impl HarnessPane {
    /// Load the discovered origins, then what the open (or default) one
    /// holds.
    pub(super) fn refresh_library(&mut self, cx: &mut Context<Self>) {
        self.library.load_generation += 1;
        let generation = self.library.load_generation;
        self.library.loading = true;
        self.library.error = None;
        // Which knowledge origins hold a copy of each layered file, for the
        // marks in the list and the list under an open one. Refreshed with the
        // tree, so a copy deleted or an origin reordered shows without
        // reopening the view.
        self.load_knowledge_layering(cx);
        cx.notify();

        let client = self.client.clone();
        let wanted = self.library.root_key.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || -> Result<Loaded, String> {
                let origins = client
                    .post_action(ActionRequest::LibraryOrigins)
                    .and_then(|v| v.ok_or_else(|| "Missing library origins".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<LibraryOrigins>(v)
                            .map_err(|e| format!("Unexpected library origins: {e}"))
                    })?;
                // Stay on the open origin while it still exists; otherwise
                // open the default one.
                let key = wanted
                    .filter(|k| origins.origin(k).is_some())
                    .or_else(|| origins.default_origin().map(|o| o.key.clone()));
                // An origin that is not usable has no tree to read: its
                // problems are on its overview.
                let tree = key
                    .clone()
                    .filter(|k| origins.origin(k).is_some_and(|o| o.healthy))
                    .map(|k| {
                        client
                            .post_action(ActionRequest::LibraryTree { root: Some(k) })
                            .and_then(|v| v.ok_or_else(|| "Missing library tree".to_string()))
                            .and_then(|v| {
                                serde_json::from_value::<LibraryTree>(v)
                                    .map_err(|e| format!("Unexpected library tree: {e}"))
                            })
                    });
                Ok((origins, key, tree))
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.library.load_generation != generation {
                        return;
                    }
                    this.library.loading = false;
                    match result {
                        Ok((origins, key, tree)) => {
                            if key != this.library.root_key {
                                this.library.leave_selection();
                            }
                            this.library.root_key = key;
                            this.library.origins = Some(origins);
                            this.prune_knowledge_order(cx);
                            this.doc_search_refreshed(cx);
                            match tree {
                                Some(Ok(tree)) => {
                                    this.library.tree = Some(tree);
                                    // A file deleted or renamed since it was
                                    // opened must not stay on screen looking
                                    // current — unless it holds edits, which
                                    // would go with it.
                                    let root = this.library.root_key.clone().unwrap_or_default();
                                    if let Some(path) = this.library.selected.clone()
                                        && !this.library.tree_contains(&path)
                                        && !this.library.documents.is_dirty(&root, &path)
                                    {
                                        this.library.leave_selection();
                                    }
                                }
                                Some(Err(e)) => {
                                    this.library.tree = None;
                                    this.library.error = Some(e);
                                }
                                None => this.library.tree = None,
                            }
                        }
                        Err(e) => this.library.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Drop keys for knowledge origins that are no longer discovered from the
    /// saved layering order.
    ///
    /// A listing is the only moment okena learns that an origin has gone — it
    /// was unregistered, or its project left the workspace — so it is the
    /// moment to forget where it used to sit. One whose checkout is merely
    /// missing is still discovered, so it keeps its place.
    ///
    /// Nothing is written unless something actually dropped, and a listing
    /// with no knowledge origins is left alone: "none came back" must not be
    /// read as "arrange nothing".
    fn prune_knowledge_order(&mut self, cx: &mut Context<Self>) {
        let Some(origins) = self.library.origins.as_ref() else {
            return;
        };
        let layers: Vec<LibraryOrigin> = origins.of_type(OriginType::Knowledge).cloned().collect();
        if layers.is_empty() {
            return;
        }
        let settings = crate::settings::settings_entity(cx);
        let saved = settings
            .read(cx)
            .settings
            .active_space()
            .library
            .knowledge
            .order
            .clone();
        if saved.is_empty() {
            return;
        }
        let kept = okena_core::knowledge_order::normalize(&layers, &saved);
        if kept != saved {
            settings.update(cx, |state, cx| state.set_knowledge_root_order(kept, cx));
        }
    }

    fn select_origin(&mut self, key: String, cx: &mut Context<Self>) {
        if self.library.root_key.as_deref() == Some(key.as_str()) {
            return;
        }
        self.library.leave_selection();
        self.library.root_key = Some(key);
        self.library.tree = None;
        self.library.collapsed.clear();
        // Each of these stands where the document does and is about the
        // origin that was open; another origin's row is how you leave them.
        self.knowledge_draft.open = false;
        self.library.composing = false;
        self.file_ops.creating = None;
        self.refresh_library(cx);
    }

    /// Show the origin keyed `root_key` with nothing selected — a project's
    /// store chip opening the store it follows.
    pub(crate) fn open_library_root(&mut self, root_key: String, cx: &mut Context<Self>) {
        self.roots.open = false;
        self.select_origin(root_key, cx);
    }

    /// Open `path` in the origin keyed `root_key`, switching origins when
    /// needed.
    ///
    /// For links from outside the tree: a search hit, a project map's docs in
    /// the project info panel, a launcher's brief. The refresh an origin
    /// switch starts keeps the origin it was asked for, so the file stays
    /// open.
    pub(crate) fn open_library_doc(
        &mut self,
        root_key: String,
        path: String,
        cx: &mut Context<Self>,
    ) {
        // The Origins page stands where the document does; a document you
        // asked for has to be what you see.
        self.roots.open = false;
        if self.library.root_key.as_deref() != Some(root_key.as_str()) {
            self.library.leave_selection();
            self.library.root_key = Some(root_key);
            self.library.tree = None;
            self.library.collapsed.clear();
            self.refresh_library(cx);
        }
        self.open_library_file(path, cx);
    }

    /// Open one file of the current origin: its held buffer when it has
    /// unsaved edits, else a fresh read.
    pub(super) fn open_library_file(&mut self, path: String, cx: &mut Context<Self>) {
        if self.library.selected.as_deref() != Some(path.as_str()) {
            self.library.leave_selection();
        }
        // A form and a document share the one panel, so picking a document is
        // how you leave the form. Without this, clicking one while drafting
        // looked like nothing had happened.
        self.knowledge_draft.open = false;
        self.library.composing = false;
        self.library.selected = Some(path.clone());
        self.library.content_error = None;
        self.knowledge_override.clear();
        cx.notify();

        // Opening one of okena's defaults asks who is already overriding it,
        // which the preview says and the picker marks.
        if self.library_open_origin().is_some_and(|o| o.builtin) {
            self.load_knowledge_overrides(path.clone(), cx);
        }

        let root_key = self.library.root_key.clone().unwrap_or_default();
        if self.library.documents.get(&root_key, &path).is_some() {
            return;
        }

        let client = self.client.clone();
        let root = self.library.root_key.clone();
        let wanted = path.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::LibraryRead { root, path })
                    .and_then(|v| v.ok_or_else(|| "Missing document".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<LibraryDocument>(v)
                            .map_err(|e| format!("Unexpected document: {e}"))
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    // A slow read must not replace a file opened after it.
                    if this.library.selected.as_deref() != Some(wanted.as_str())
                        || this.library.root_key.clone().unwrap_or_default() != root_key
                    {
                        return;
                    }
                    match result {
                        Ok(doc) => this.library.documents.insert(
                            &root_key,
                            DocumentBuffer::new(doc.path, doc.content, doc.revision),
                        ),
                        Err(e) => this.library.content_error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// The origin currently being shown, when it is still discovered.
    pub(super) fn library_open_origin(&self) -> Option<&LibraryOrigin> {
        self.library.open_origin()
    }

    pub(super) fn toggle_library_fold(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.library.collapsed.remove(&key) {
            self.library.collapsed.insert(key);
        }
        cx.notify();
    }

    /// Start whatever "New" means in the open origin.
    fn open_library_new(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(origins) = self.library.origins.as_ref() else {
            return;
        };
        match new_kind(origins, self.library.open_origin()) {
            Some(NewKind::KnowledgeDraft) => self.open_draft(OriginType::Knowledge, cx),
            Some(NewKind::SpecChange) => self.open_new_change(cx),
            Some(NewKind::FreeformDraft) => self.open_draft(OriginType::Freeform, cx),
            None => {}
        }
    }
}

// ─── Rendering: the pieces every origin type shares ─────────────────────────

impl HarnessPane {
    pub(super) fn section_label(&self, label: &str, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .px(px(6.0))
            .pt(px(10.0))
            .pb(px(3.0))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(label.to_uppercase())
            .into_any_element()
    }

    pub(super) fn muted_line(&self, text: impl Into<SharedString>, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .px(px(6.0))
            .py(px(3.0))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(text.into())
            .into_any_element()
    }

    pub(super) fn fact_row(&self, label: &str, value: String, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        h_flex()
            .gap(px(10.0))
            .items_start()
            .min_w_0()
            .child(
                div()
                    .w(px(72.0))
                    .flex_shrink_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(label.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(t.text_primary))
                    .child(value),
            )
            .into_any_element()
    }

    pub(super) fn diagnostic_row(&self, d: &Diagnostic, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        let color = match d.severity {
            Severity::Error => t.error,
            Severity::Warning => t.warning,
        };
        v_flex()
            .gap(px(1.0))
            .child(
                div()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(color))
                    .child(d.message.clone()),
            )
            .when_some(d.fix.clone(), |el, fix| {
                el.child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child(format!("Fix: {fix}")),
                )
            })
            .into_any_element()
    }

    /// A label over a form field.
    pub(super) fn field_label(&self, label: &str, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_secondary))
            .child(label.to_string())
            .into_any_element()
    }

    /// Explanatory line under a form field.
    pub(super) fn field_hint(&self, hint: &str, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(hint.to_string())
            .into_any_element()
    }

    pub(super) fn choice_chip(
        &self,
        id: String,
        label: String,
        selected: bool,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        div()
            .id(SharedString::from(id))
            .cursor_pointer()
            .px(px(12.0))
            .py(px(5.0))
            .rounded(px(4.0))
            .when(selected, |d| {
                d.bg(with_alpha(t.button_primary_bg, 0.2))
                    .text_color(rgb(t.text_primary))
            })
            .when(!selected, |d| {
                d.bg(rgb(t.bg_secondary))
                    .text_color(rgb(t.text_secondary))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
            })
            .text_size(ui_text_md(cx))
            .child(label)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    on_click(this, cx);
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// A disclosure row: a kind group, a folder.
    pub(super) fn render_fold_row(
        &self,
        key: String,
        label: String,
        count: Option<usize>,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let collapsed = self.library.collapsed.contains(&key);
        let toggle = key.clone();
        h_flex()
            .id(SharedString::from(format!("library-fold-{key}")))
            .cursor_pointer()
            .w_full()
            .min_w_0()
            .items_center()
            .gap(px(4.0))
            .pl(px(6.0 + 12.0 * depth as f32))
            .pr(px(6.0))
            .py(px(3.0))
            .rounded(px(3.0))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .child(
                div()
                    .w(px(10.0))
                    .flex_shrink_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(if collapsed { "›" } else { "⌄" }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(label),
            )
            .when_some(count, |d, n| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child(n.to_string()),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.toggle_library_fold(toggle.clone(), cx);
                }),
            )
            .into_any_element()
    }

    /// One selectable document row of a tree.
    pub(super) fn render_document_row(
        &self,
        path: &str,
        label: &str,
        left: f32,
        trailing: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let selected = self.library.selected.as_deref() == Some(path);
        let open = path.to_string();
        h_flex()
            .id(SharedString::from(format!("library-doc-{path}")))
            .cursor_pointer()
            .w_full()
            .min_w_0()
            .items_center()
            .gap(px(6.0))
            .pl(px(left))
            .pr(px(8.0))
            .py(px(3.0))
            .rounded(px(3.0))
            .when(selected, |d| d.bg(with_alpha(t.button_primary_bg, 0.18)))
            .when(!selected, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(if selected {
                        t.text_primary
                    } else {
                        t.text_secondary
                    }))
                    .child(format!(
                        "{}{label}",
                        self.unsaved_marker(path).unwrap_or_default()
                    )),
            )
            .children(trailing)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.open_library_file(open.clone(), cx);
                }),
            )
            .into_any_element()
    }

    /// One selectable origin in the origin list.
    fn render_origin_row(&self, origin: &LibraryOrigin, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let selected = self.library.root_key.as_deref() == Some(origin.key.as_str());
        let key = origin.key.clone();
        let tag = |text: &'static str, color: u32| {
            div()
                .flex_shrink_0()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(color))
                .child(text)
        };
        h_flex()
            .id(SharedString::from(format!("library-origin-{}", origin.key)))
            .cursor_pointer()
            .w_full()
            .min_w_0()
            .items_center()
            .gap(px(6.0))
            .px(px(6.0))
            .py(px(3.0))
            .rounded(px(3.0))
            .when(selected, |d| d.bg(with_alpha(t.button_primary_bg, 0.18)))
            .when(!selected, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
            .child(
                div()
                    .size(px(6.0))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(rgb(health_color(origin, &t))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(if selected {
                        t.text_primary
                    } else {
                        t.text_secondary
                    }))
                    .child(origin.name.clone()),
            )
            .when(origin.builtin, |d| d.child(tag("okena's", t.text_muted)))
            .when_some(origin.git.as_ref().and_then(sync_badge), |d, badge| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.warning))
                        .child(badge),
                )
            })
            .when(origin.is_default, |d| d.child(tag("default", t.text_muted)))
            .child(tag(origin.kind.label(), t.text_muted))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.select_origin(key.clone(), cx);
                }),
            )
            .into_any_element()
    }

    /// Left column: the origins by type, then what the open one holds.
    ///
    /// While the island narrows the page, the matches across every origin
    /// stand here instead (QBL-436).
    fn render_library_tree(&self, origins: &LibraryOrigins, cx: &mut Context<Self>) -> AnyElement {
        if self.doc_search_active() {
            return self.render_doc_results(cx);
        }
        let t = theme(cx);
        let mut col = self.file_sidebar_column("library-tree");

        // The origins are listed here, so this is where you add and arrange
        // them (QBL-429).
        col = col.child(self.tree_heading_with_add(
            "Origins",
            "library-origins-page",
            "Add or arrange origins",
            |this, _window, cx| this.open_roots_page(cx),
            cx,
        ));
        for origin_type in OriginType::all() {
            let of_type: Vec<&LibraryOrigin> = origins.of_type(origin_type).collect();
            if of_type.is_empty() {
                continue;
            }
            // The type is what tells three kinds of folder apart, so it
            // heads its origins rather than tagging each row.
            col = col.child(
                div()
                    .px(px(6.0))
                    .pt(px(6.0))
                    .pb(px(1.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(group_label(origin_type)),
            );
            for origin in of_type {
                col = col.child(self.render_origin_row(origin, cx));
            }
        }
        let missing: Vec<_> = origins
            .pointers
            .iter()
            .filter(|p| p.root_key.is_none())
            .collect();
        if !missing.is_empty() {
            col = col.child(self.section_label("Followed, not here", cx));
            for p in missing {
                col = col.child(self.muted_line(format!("{} → {}", p.project, p.store_id), cx));
            }
        }

        let Some(tree) = self.library.tree.clone() else {
            if self.library.loading {
                col = col.child(self.muted_line("Loading…", cx));
            }
            return col.into_any_element();
        };
        // What an origin holds is its type's own shape.
        let rows = match &tree {
            LibraryTree::Knowledge(tree) => self.render_knowledge_entries(tree, cx),
            LibraryTree::Spec(tree) => self.render_spec_entries(tree, cx),
            LibraryTree::Freeform(tree) => self.render_freeform_entries(tree, cx),
        };
        col.children(rows).into_any_element()
    }

    /// Right column with nothing open: what this origin is, its sync state,
    /// its uncommitted changes and its problems.
    fn render_origin_overview(&self, origin: &LibraryOrigin, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let mut col = v_flex()
            .id("library-origin-overview")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .px(px(20.0))
            .py(px(16.0))
            .gap(px(8.0))
            .child(
                h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .child(
                        div()
                            .text_size(ui_text(15.0, cx))
                            .text_color(rgb(t.text_primary))
                            .child(origin.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(origin_caption(origin)),
                    ),
            );
        if let Some(description) = origin.description.clone() {
            col = col.child(
                div()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(description),
            );
        }
        if let Some(id) = origin.store_id.clone() {
            col = col.child(self.fact_row("Id", id, cx));
        }
        col = col.child(self.fact_row("Path", origin.path.clone(), cx));
        if let Some(remote) = origin.remote.clone() {
            col = col.child(self.fact_row("Remote", remote, cx));
        }
        // What only one type has.
        if let Some(counts) = origin.counts.as_ref() {
            col = col.child(self.fact_row("Entries", counts_line(counts), cx));
        }
        if let Some(documents) = origin.documents {
            let noun = if documents == 1 { "document" } else { "documents" };
            col = col.child(self.fact_row("Holds", format!("{documents} {noun}"), cx));
        }
        if let Some(schema) = origin.schema.clone() {
            col = col.child(self.fact_row("Schema", schema, cx));
        }
        if !origin.used_by.is_empty() {
            col = col.child(self.fact_row("Used by", origin.used_by.join(", "), cx));
        }
        if origin.origin_type == OriginType::Spec {
            col = col.child(self.fact_row("CLI", cli_hint(origin), cx));
        }
        if origin.origin_type == OriginType::Freeform {
            col = col.child(self.field_hint(
                "A freeform origin is any folder of markdown: listed, edited, searched and \
                 handed to agents as context. It has no layout to follow and nothing in it \
                 overrides anything.",
                cx,
            ));
        }

        // An origin at the top of its own git checkout; a project's, or a
        // folder without git, has no sync state and shows none.
        if let Some(git) = origin.git.clone() {
            col = col.child(self.render_store_git(&git, cx));
        }

        if !origin.references.is_empty() {
            col = col.child(div().pt(px(8.0)).child(self.section_label("References", cx)));
            for r in &origin.references {
                col = col.child(match &r.root {
                    Some(path) if r.status.is_empty() => {
                        self.fact_row(&r.id, format!("✓ {path}"), cx)
                    }
                    _ => v_flex()
                        .children(r.status.iter().map(|d| self.diagnostic_row(d, cx)))
                        .into_any_element(),
                });
            }
        }
        if !origin.status.is_empty() {
            col = col.child(div().pt(px(8.0)).child(self.section_label("Problems", cx)));
            for d in &origin.status {
                col = col.child(self.diagnostic_row(d, cx));
            }
        }
        col.child(
            div()
                .pt(px(12.0))
                .text_size(ui_text_md(cx))
                .text_color(rgb(t.text_muted))
                .child("Select a document to read it."),
        )
        .into_any_element()
    }

    /// Right column: the opened file, or the origin's overview.
    fn render_library_document(
        &self,
        origin: Option<&LibraryOrigin>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let Some(path) = self.library.selected.clone() else {
            return match origin {
                Some(origin) => self.render_origin_overview(origin, cx),
                None => v_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(t.text_muted))
                            .child("Select an origin."),
                    )
                    .into_any_element(),
            };
        };

        // okena's own store is rewritten to match the build on every start, so
        // it opens as a preview with one action: override it from a knowledge
        // origin of your own (QBL-415). The daemon refuses the writes too —
        // this only stops the view offering what would be refused.
        let read_only = origin.is_some_and(|o| o.builtin);
        let buffer = self.open_buffer();
        let body: AnyElement = match buffer {
            // The source carries the frontmatter the meta block is drawn from,
            // so editing shows the editor alone.
            Some(buffer) if buffer.editing() && !read_only => self
                .render_document_editor(buffer, cx)
                .unwrap_or_else(|| self.info_banner("Loading…".into(), cx)),
            _ => {
                let mut page = v_flex()
                    .w_full()
                    .max_w(okena_markdown::DOC_MAX_WIDTH)
                    .min_w_0();
                // A knowledge entry says what it is and what an agent picks
                // it by; the other types' documents are their text.
                if let Some(LibraryTree::Knowledge(tree)) = self.library.tree.as_ref()
                    && let Some(entry) = tree.entry(&path)
                {
                    page = page.child(self.render_entry_meta(entry, cx));
                }
                page = if let Some(err) = &self.library.content_error {
                    page.child(self.error_banner(err.clone(), cx))
                } else {
                    match buffer {
                        Some(buffer) => page.children(self.render_document_preview(buffer, cx)),
                        None => page.child(self.info_banner("Loading…".into(), cx)),
                    }
                };
                v_flex()
                    .id("library-document-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(20.0))
                    .py(px(14.0))
                    .child(page)
                    .into_any_element()
            }
        };
        let save_error = buffer
            .and_then(|b| b.save_error.clone())
            .map(|e| self.error_banner(e, cx));

        let header = match origin {
            Some(origin) => format!("{} · {path}", origin.name),
            None => path.clone(),
        };
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                h_flex()
                    .w_full()
                    .flex_shrink_0()
                    .items_center()
                    .px(px(16.0))
                    .py(px(6.0))
                    .gap(px(8.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(header),
                    )
                    .children(if read_only {
                        // No Edit/Preview toggle, Save, Revert, Rename or
                        // Delete: none of them could survive the next start.
                        self.render_default_controls(cx)
                    } else {
                        let mut controls = self.render_document_controls(cx);
                        controls.extend(self.render_file_controls(cx));
                        controls
                    })
                    .child(self.small_button(
                        "library-close-document",
                        "Overview",
                        cx.listener(|this, _, _window, cx| {
                            this.library.leave_selection();
                            cx.notify();
                        }),
                        cx,
                    )),
            )
            .children(if read_only {
                self.render_override_picker(&path, cx)
            } else {
                self.render_file_op_bar(cx)
            })
            .children(save_error)
            .child(body)
            // "Refine with agent" would write into the file, so a default is
            // not offered it either.
            .children(
                (!read_only)
                    .then(|| self.render_document_agent(cx))
                    .flatten()
                    .map(|card| {
                        div()
                            .flex_shrink_0()
                            .px(px(16.0))
                            .py(px(10.0))
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .child(card)
                    }),
            )
            .into_any_element()
    }

    /// Nothing discovered: say what an origin is and where to add one.
    fn render_no_origins(&self, origins: &LibraryOrigins, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let mut col = v_flex()
            .p(px(20.0))
            .gap(px(8.0))
            .max_w(px(720.0))
            .child(
                div()
                    .text_size(ui_text(15.0, cx))
                    .text_color(rgb(t.text_primary))
                    .child("Nothing in the Library yet"),
            )
            .child(self.field_hint(
                "The Library holds the markdown your agents work from, in origins: folders, \
                 usually git repositories, of one of three types.",
                cx,
            ));
        for origin_type in OriginType::all() {
            col = col.child(self.fact_row(origin_type.label(), origin_type.blurb().to_string(), cx));
        }
        col.child(self.field_hint(
            "Clone a repository, add a folder you already have, or create a new origin. A \
             project also brings its own: its .okena/knowledge/ folder and its openspec/ tree.",
            cx,
        ))
        .children(origins.status.iter().map(|d| self.diagnostic_row(d, cx)))
        .children(
            origins
                .pointers
                .iter()
                .flat_map(|p| p.status.iter())
                .map(|d| self.diagnostic_row(d, cx)),
        )
        // The sidebar's `+` opens the Origins page, but with no origins there
        // is no sidebar — and this is exactly when you came to add one.
        .child(
            h_flex()
                .pt(px(6.0))
                .gap(px(6.0))
                .child(self.primary_button(
                    "library-add-origin",
                    "Add an origin",
                    cx.listener(|this, _, _window, cx| this.open_roots_page(cx)),
                    cx,
                ))
                .child(self.small_button(
                    "library-open-settings",
                    "Open library settings",
                    cx.listener(|this, _, _window, cx| this.open_settings("library", cx)),
                    cx,
                )),
        )
        .into_any_element()
    }

    pub(super) fn render_library_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_document_input(window, cx);
        let t = theme(cx);
        let view = v_flex().size_full();

        let Some(origins) = self.library.origins.clone() else {
            // An unreachable daemon is the whole story — an empty list next to
            // it would imply there is nothing to show.
            if let Some(err) = self.library.error.clone() {
                return view
                    .child(self.error_banner(err, cx))
                    .child(div().px(px(12.0)).child(self.small_button(
                        "library-retry",
                        "Retry",
                        cx.listener(|this, _, _window, cx| this.refresh_library(cx)),
                        cx,
                    )))
                    .into_any_element();
            }
            return view
                .child(self.info_banner("Loading the library…".into(), cx))
                .into_any_element();
        };

        let open_origin = self
            .library
            .root_key
            .as_deref()
            .and_then(|k| origins.origin(k))
            .cloned();
        let mut actions = vec![
            self.toolbar_icon(
                "library-settings",
                "icons/settings.svg",
                "Library settings",
                cx.listener(|this, _, _window, cx| this.open_settings("library", cx)),
                cx,
            ),
            self.small_button(
                "library-refresh",
                if self.library.loading {
                    "Refreshing…"
                } else {
                    "Refresh"
                },
                cx.listener(|this, _, _window, cx| this.refresh_library(cx)),
                cx,
            ),
        ];
        // Only offered where something could be written.
        if new_kind(&origins, open_origin.as_ref()).is_some() {
            actions.push(self.primary_button(
                "library-new",
                "New",
                cx.listener(|this, _, window, cx| this.open_library_new(window, cx)),
                cx,
            ));
        }
        let toggle = self.file_sidebar_toggle(!origins.origins.is_empty(), cx);
        let mut view = view.child(self.render_toolbar_with_leading(Some(toggle), actions, cx));
        if let Some(notice) = self.knowledge_draft.notice.clone() {
            view = view.child(self.info_banner(notice, cx));
        }
        if let Some(err) = self.library.error.clone() {
            view = view.child(self.error_banner(err, cx));
        }
        if origins.origins.is_empty() {
            // The page is the way out of this state, so it takes the column
            // when it is open; there is no sidebar to put it beside.
            return view
                .child(if self.roots.open {
                    self.render_roots_page(cx)
                } else {
                    self.render_no_origins(&origins, cx)
                })
                .into_any_element();
        }

        let sidebar = self.files.open.then(|| {
            let tree = self.render_library_tree(&origins, cx);
            self.render_file_sidebar(tree, cx)
        });
        view.child(
            h_flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .bg(rgb(t.bg_primary))
                // Closed, the document takes the whole width: there is nothing
                // left of it to leave a gap for.
                .children(sidebar)
                // A form stands where a document's text stands. It used to
                // take the whole view, which hid the tree you were adding to
                // and the files you are meant to read before adding to them.
                .child(if self.roots.open {
                    self.render_roots_page(cx)
                } else if self.knowledge_draft.open {
                    self.render_knowledge_draft_form(&origins, cx)
                } else if self.library.composing {
                    self.render_new_change_form(&origins, cx)
                } else {
                    self.render_library_document(open_origin.as_ref(), cx)
                }),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{NewKind, cli_hint, counts_line, group_label, new_kind, origin_caption};
    use okena_core::knowledge::KnowledgeCounts;
    use okena_core::library::{LibraryOrigin, LibraryOrigins, OriginKind, OriginType};

    fn origin(key: &str) -> LibraryOrigin {
        let (origin_type, _) = okena_core::library::split_key(key).expect("a library key");
        serde_json::from_value(serde_json::json!({
            "key": key, "type": origin_type.slug(), "kind": "store", "name": key,
            "path": "/work/app", "healthy": true,
        }))
        .expect("origin")
    }

    fn origins(list: Vec<LibraryOrigin>) -> LibraryOrigins {
        LibraryOrigins {
            origins: list,
            ..Default::default()
        }
    }

    #[test]
    fn new_starts_what_the_open_origins_type_writes() {
        let knowledge = origin("knowledge:store:eng");
        let spec = origin("spec:store:plans");
        let freeform = origin("freeform:path:/notes");
        let all = origins(vec![knowledge.clone(), spec.clone(), freeform.clone()]);
        assert_eq!(new_kind(&all, Some(&knowledge)), Some(NewKind::KnowledgeDraft));
        assert_eq!(new_kind(&all, Some(&spec)), Some(NewKind::SpecChange));
        assert_eq!(new_kind(&all, Some(&freeform)), Some(NewKind::FreeformDraft));
    }

    #[test]
    fn new_is_not_offered_where_nothing_can_be_written() {
        let defaults = LibraryOrigin {
            builtin: true,
            ..origin("knowledge:store:okena-defaults")
        };
        // okena's own store alone: there is nowhere to draft knowledge.
        let only_defaults = origins(vec![defaults.clone()]);
        assert_eq!(new_kind(&only_defaults, Some(&defaults)), None);
        // With a store of your own beside it, the draft form picks that one.
        let with_own = origins(vec![origin("knowledge:store:eng"), defaults.clone()]);
        assert_eq!(new_kind(&with_own, Some(&defaults)), Some(NewKind::KnowledgeDraft));
        // A broken spec origin cannot take a change.
        let broken = LibraryOrigin {
            healthy: false,
            ..origin("spec:store:plans")
        };
        assert_eq!(new_kind(&origins(vec![broken.clone()]), Some(&broken)), None);
    }

    #[test]
    fn the_cli_hint_selects_a_store_by_id_and_anything_else_by_directory() {
        let store = LibraryOrigin {
            store_id: Some("team-plans".into()),
            ..origin("spec:store:team-plans")
        };
        assert_eq!(cli_hint(&store), "openspec list --store team-plans");
        // A folder holding store metadata is not registered, so --store would
        // fail.
        let folder = LibraryOrigin {
            kind: OriginKind::Folder,
            store_id: Some("team-plans".into()),
            ..origin("spec:path:/work/app")
        };
        assert_eq!(cli_hint(&folder), "cd /work/app && openspec list");
    }

    #[test]
    fn an_origin_says_its_type_where_it_was_found_and_what_is_special_about_it() {
        assert_eq!(origin_caption(&origin("freeform:path:/notes")), "freeform · store");
        let defaults = LibraryOrigin {
            builtin: true,
            ..origin("knowledge:store:okena-defaults")
        };
        assert_eq!(origin_caption(&defaults), "knowledge · store · okena's own");
        let default_store = LibraryOrigin {
            is_default: true,
            ..origin("spec:store:plans")
        };
        assert_eq!(origin_caption(&default_store), "spec · store · machine default");
    }

    #[test]
    fn counts_read_as_a_line_and_types_group_under_plain_names() {
        let counts = KnowledgeCounts {
            docs: 1,
            skills: 2,
            agents: 0,
            templates: 1,
        };
        assert_eq!(counts_line(&counts), "1 doc · 2 skills · 0 agents · 1 template");
        let labels: Vec<_> = OriginType::all().into_iter().map(group_label).collect();
        assert_eq!(labels, ["Knowledge", "Specs", "Freeform"]);
    }
}
