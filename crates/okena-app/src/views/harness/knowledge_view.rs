//! Knowledge view — knowledge stores and project roots, and the entries in the
//! one you pick.
//!
//! Roots are whatever the daemon discovered (ADR-0003): stores in okena's
//! registry and the kind folders projects carry. Reading, saving, fetching and
//! pulling all go through the daemon, which refuses roots it did not discover
//! and paths outside a root; the client never touches the filesystem. Files
//! edit through the editor the Specs view shares (`editor.rs`).

use crate::theme::{ThemeColors, theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms};
use crate::views::components::{SimpleInput, SimpleInputState};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::knowledge::{
    Diagnostic, KnowledgeDocument, KnowledgeEntry, KnowledgeKind, KnowledgeRoot, KnowledgeRootKind,
    KnowledgeStores, KnowledgeTree, Severity,
};
use okena_ui::simple_input::InputChangedEvent;
use std::collections::HashSet;

use super::editor::{DocumentBuffer, Documents};
use super::store_git::{StoreGitPanel, StoreSection, sync_badge};
use super::{HarnessPane, HarnessSection};

/// Width of the root and entry list, matching the Specs view.
const TREE_WIDTH: f32 = 280.0;

/// Knowledge-view state.
pub(crate) struct KnowledgeState {
    /// Every root the daemon discovered. `None` until the first load lands.
    pub(crate) stores: Option<KnowledgeStores>,
    /// Key of the root being shown.
    pub(crate) root_key: Option<String>,
    pub(crate) tree: Option<KnowledgeTree>,
    pub(crate) loading: bool,
    /// Bumped on every load, so a slow response for a root the user has since
    /// left is dropped instead of replacing the newer one.
    pub(crate) load_generation: u64,
    pub(crate) error: Option<String>,
    /// The open store's fetch, pull, commit and push.
    pub(crate) git: StoreGitPanel,
    /// Path of the file being read, relative to the root: an entry, or one of
    /// a skill's supporting files.
    pub(crate) selected: Option<String>,
    /// The open file's buffer, and any other with unsaved edits.
    pub(crate) documents: Documents,
    pub(crate) content_error: Option<String>,
    pub(crate) filter: Entity<SimpleInputState>,
    /// Groups (`docs`) and doc folders (`docs/ci`) folded shut. Collapsed
    /// rather than expanded state, so a fresh view shows everything.
    pub(crate) collapsed: HashSet<String>,
}

impl KnowledgeState {
    pub(crate) fn new(cx: &mut Context<HarnessPane>) -> Self {
        let filter = cx.new(|cx| SimpleInputState::new(cx).placeholder("Filter entries…"));
        // The list filters as you type, so every keystroke re-renders.
        cx.subscribe(
            &filter,
            |_this: &mut HarnessPane, _, _: &InputChangedEvent, cx| cx.notify(),
        )
        .detach();
        Self {
            stores: None,
            root_key: None,
            tree: None,
            loading: false,
            load_generation: 0,
            error: None,
            git: StoreGitPanel::new(cx),
            selected: None,
            documents: Documents::default(),
            content_error: None,
            filter,
            collapsed: HashSet::new(),
        }
    }

    /// Stop showing the open file. Its buffer stays only if it holds unsaved
    /// edits. Call before `root_key` changes: buffers are keyed by it.
    fn clear_selection(&mut self) {
        if let Some(path) = self.selected.take() {
            self.documents
                .leave(self.root_key.as_deref().unwrap_or_default(), &path);
        }
        self.content_error = None;
    }

    /// Whether the loaded tree still lists `path`, as an entry or a skill file.
    fn tree_contains(&self, path: &str) -> bool {
        self.tree.as_ref().is_some_and(|t| {
            t.entries
                .iter()
                .any(|e| e.path == path || e.files.iter().any(|f| f == path))
        })
    }
}

// ─── Pure helpers ───────────────────────────────────────────────────────────

/// One line of the entry list.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Row<'a> {
    /// A folder of docs, keyed `docs/<folder path>` for collapsing.
    Folder {
        key: String,
        name: String,
        depth: usize,
    },
    Entry {
        entry: &'a KnowledgeEntry,
        depth: usize,
    },
}

/// One kind's section of the list.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Group<'a> {
    pub(crate) kind: KnowledgeKind,
    /// Matching entries, counted before folding.
    pub(crate) count: usize,
    pub(crate) rows: Vec<Row<'a>>,
}

/// Whether `entry` matches a filter typed by the user: case-insensitive, over
/// what a person would search by. An empty filter matches everything.
pub(crate) fn entry_matches(entry: &KnowledgeEntry, filter: &str) -> bool {
    let needle = filter.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    std::iter::once(entry.title.as_str())
        .chain(std::iter::once(entry.name.as_str()))
        .chain(std::iter::once(entry.path.as_str()))
        .chain(entry.description.as_deref())
        .chain(entry.tags.iter().map(String::as_str))
        .any(|field| field.to_lowercase().contains(&needle))
}

/// The list: kinds in their fixed order, empty kinds left out, docs nested
/// under folder rows. Folding is ignored while filtering, so a match is never
/// hidden inside a closed folder.
pub(crate) fn group_entries<'a>(
    entries: &'a [KnowledgeEntry],
    filter: &str,
    collapsed: &HashSet<String>,
) -> Vec<Group<'a>> {
    let filtering = !filter.trim().is_empty();
    let mut groups = Vec::new();
    for kind in KnowledgeKind::all() {
        let mut matching: Vec<&KnowledgeEntry> = entries
            .iter()
            .filter(|e| e.kind == kind && entry_matches(e, filter))
            .collect();
        if matching.is_empty() {
            continue;
        }
        matching.sort_by(|a, b| a.path.cmp(&b.path));
        let count = matching.len();
        let mut rows = Vec::new();
        if filtering || !collapsed.contains(kind.folder()) {
            match kind {
                KnowledgeKind::Doc => nest_docs(&matching, filtering, collapsed, &mut rows),
                _ => rows.extend(
                    matching
                        .into_iter()
                        .map(|entry| Row::Entry { entry, depth: 0 }),
                ),
            }
        }
        groups.push(Group { kind, count, rows });
    }
    groups
}

fn nest_docs<'a>(
    docs: &[&'a KnowledgeEntry],
    filtering: bool,
    collapsed: &HashSet<String>,
    rows: &mut Vec<Row<'a>>,
) {
    let mut open: Vec<&str> = Vec::new();
    for entry in docs {
        let folders: Vec<&str> = entry
            .name
            .rsplit_once('/')
            .map_or_else(Vec::new, |(dir, _)| dir.split('/').collect());
        let shared = open
            .iter()
            .zip(&folders)
            .take_while(|(a, b)| a == b)
            .count();
        open.truncate(shared);
        let mut hidden = false;
        for (depth, folder) in folders.iter().enumerate() {
            let key = format!("docs/{}", folders[..=depth].join("/"));
            if depth >= shared && !hidden {
                rows.push(Row::Folder {
                    key: key.clone(),
                    name: (*folder).to_string(),
                    depth,
                });
            }
            if depth >= open.len() {
                open.push(folder);
            }
            hidden = hidden || (!filtering && collapsed.contains(&key));
        }
        if !hidden {
            rows.push(Row::Entry {
                entry,
                depth: folders.len(),
            });
        }
    }
}

fn kind_label(kind: KnowledgeRootKind) -> &'static str {
    match kind {
        KnowledgeRootKind::Store => "store",
        KnowledgeRootKind::Project => "project",
    }
}

fn entry_kind_label(kind: KnowledgeKind) -> &'static str {
    match kind {
        KnowledgeKind::Doc => "doc",
        KnowledgeKind::Skill => "skill",
        KnowledgeKind::Agent => "agent",
        KnowledgeKind::Template => "template",
    }
}

fn health_color(root: &KnowledgeRoot, t: &ThemeColors) -> u32 {
    if !root.healthy {
        t.error
    } else if root.status.iter().any(|d| d.severity == Severity::Warning) {
        t.warning
    } else {
        t.success
    }
}

fn counts_line(root: &KnowledgeRoot) -> String {
    let c = &root.counts;
    let plural = |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    [
        plural(c.docs, "doc", "docs"),
        plural(c.skills, "skill", "skills"),
        plural(c.agents, "agent", "agents"),
        plural(c.templates, "template", "templates"),
    ]
    .join(" · ")
}

type Loaded = (
    KnowledgeStores,
    Option<String>,
    Option<Result<KnowledgeTree, String>>,
);

// ─── Loading and actions ────────────────────────────────────────────────────

impl HarnessPane {
    /// Load the discovered roots, then the entries of the open (or default)
    /// root.
    pub(super) fn refresh_knowledge(&mut self, cx: &mut Context<Self>) {
        self.knowledge.load_generation += 1;
        let generation = self.knowledge.load_generation;
        self.knowledge.loading = true;
        self.knowledge.error = None;
        cx.notify();

        let client = self.client.clone();
        let wanted = self.knowledge.root_key.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || -> Result<Loaded, String> {
                let stores = client
                    .post_action(ActionRequest::KnowledgeStores)
                    .and_then(|v| v.ok_or_else(|| "Missing knowledge stores".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<KnowledgeStores>(v)
                            .map_err(|e| format!("Unexpected knowledge stores: {e}"))
                    })?;
                // Stay on the open root while it still exists and works;
                // otherwise open the default one.
                let key = wanted
                    .filter(|k| stores.root(k).is_some_and(|r| r.healthy))
                    .or_else(|| {
                        stores
                            .default_root()
                            .filter(|r| r.healthy)
                            .map(|r| r.key.clone())
                    });
                let tree = key.clone().map(|k| {
                    client
                        .post_action(ActionRequest::KnowledgeTree { root: Some(k) })
                        .and_then(|v| v.ok_or_else(|| "Missing knowledge tree".to_string()))
                        .and_then(|v| {
                            serde_json::from_value::<KnowledgeTree>(v)
                                .map_err(|e| format!("Unexpected knowledge tree: {e}"))
                        })
                });
                Ok((stores, key, tree))
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.knowledge.load_generation != generation {
                        return;
                    }
                    this.knowledge.loading = false;
                    match result {
                        Ok((stores, key, tree)) => {
                            if key != this.knowledge.root_key {
                                this.knowledge.clear_selection();
                            }
                            this.knowledge.root_key = key;
                            this.knowledge.stores = Some(stores);
                            match tree {
                                Some(Ok(tree)) => {
                                    this.knowledge.tree = Some(tree);
                                    // A file deleted or renamed since it was
                                    // opened must not stay on screen looking
                                    // current — unless it holds edits, which
                                    // would go with it.
                                    let root = this.knowledge.root_key.clone().unwrap_or_default();
                                    if let Some(path) = this.knowledge.selected.clone()
                                        && !this.knowledge.tree_contains(&path)
                                        && !this.knowledge.documents.is_dirty(&root, &path)
                                    {
                                        this.knowledge.clear_selection();
                                    }
                                }
                                Some(Err(e)) => {
                                    this.knowledge.tree = None;
                                    this.knowledge.error = Some(e);
                                }
                                None => this.knowledge.tree = None,
                            }
                        }
                        Err(e) => this.knowledge.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn select_knowledge_root(&mut self, key: String, cx: &mut Context<Self>) {
        if self.knowledge.root_key.as_deref() == Some(key.as_str()) {
            return;
        }
        self.knowledge.clear_selection();
        self.knowledge.root_key = Some(key);
        self.knowledge.tree = None;
        self.knowledge.collapsed.clear();
        self.refresh_knowledge(cx);
    }

    /// Open one file of the current root: its held buffer when it has unsaved
    /// edits, else a fresh read.
    pub(super) fn open_knowledge_file(&mut self, path: String, cx: &mut Context<Self>) {
        if self.knowledge.selected.as_deref() != Some(path.as_str()) {
            self.knowledge.clear_selection();
        }
        self.knowledge.selected = Some(path.clone());
        self.knowledge.content_error = None;
        cx.notify();

        let root_key = self.knowledge.root_key.clone().unwrap_or_default();
        if self.knowledge.documents.get(&root_key, &path).is_some() {
            return;
        }

        let client = self.client.clone();
        let root = self.knowledge.root_key.clone();
        let wanted = path.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::KnowledgeRead { root, path })
                    .and_then(|v| v.ok_or_else(|| "Missing document".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<KnowledgeDocument>(v)
                            .map_err(|e| format!("Unexpected document: {e}"))
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    // A slow read must not replace a file opened after it.
                    if this.knowledge.selected.as_deref() != Some(wanted.as_str())
                        || this.knowledge.root_key.clone().unwrap_or_default() != root_key
                    {
                        return;
                    }
                    match result {
                        Ok(doc) => this.knowledge.documents.insert(
                            &root_key,
                            DocumentBuffer::new(doc.path, doc.content, doc.revision),
                        ),
                        Err(e) => this.knowledge.content_error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn toggle_knowledge_fold(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.knowledge.collapsed.remove(&key) {
            self.knowledge.collapsed.insert(key);
        }
        cx.notify();
    }
}

// ─── Rendering ──────────────────────────────────────────────────────────────

impl HarnessPane {
    fn kn_section_label(&self, label: String, cx: &Context<Self>) -> AnyElement {
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

    fn kn_muted(&self, text: impl Into<SharedString>, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .px(px(6.0))
            .py(px(3.0))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(text.into())
            .into_any_element()
    }

    fn kn_fact(&self, label: &str, value: String, cx: &Context<Self>) -> AnyElement {
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

    fn kn_diagnostic(&self, d: &Diagnostic, cx: &Context<Self>) -> AnyElement {
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

    fn render_knowledge_root_row(
        &self,
        root: &KnowledgeRoot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let selected = self.knowledge.root_key.as_deref() == Some(root.key.as_str());
        let key = root.key.clone();
        h_flex()
            .id(SharedString::from(format!("knowledge-root-{}", root.key)))
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
                    .bg(rgb(health_color(root, &t))),
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
                    .child(root.name.clone()),
            )
            .when_some(root.git.as_ref().and_then(sync_badge), |d, badge| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.warning))
                        .child(badge),
                )
            })
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(kind_label(root.kind)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.select_knowledge_root(key.clone(), cx);
                }),
            )
            .into_any_element()
    }

    /// A disclosure row: a kind group or a doc folder.
    fn render_fold_row(
        &self,
        key: String,
        label: String,
        count: Option<usize>,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let collapsed = self.knowledge.collapsed.contains(&key);
        let toggle = key.clone();
        h_flex()
            .id(SharedString::from(format!("knowledge-fold-{key}")))
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
                    this.toggle_knowledge_fold(toggle.clone(), cx);
                }),
            )
            .into_any_element()
    }

    fn render_entry_row(
        &self,
        entry: &KnowledgeEntry,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let selected = self.knowledge.selected.as_deref() == Some(entry.path.as_str());
        let path = entry.path.clone();
        let warned = !entry.status.is_empty();
        h_flex()
            .id(SharedString::from(format!(
                "knowledge-entry-{}",
                entry.path
            )))
            .cursor_pointer()
            .w_full()
            .min_w_0()
            .items_center()
            .gap(px(6.0))
            .pl(px(20.0 + 12.0 * depth as f32))
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
                        "{}{}",
                        self.unsaved_marker(HarnessSection::Knowledge, &entry.path)
                            .unwrap_or_default(),
                        entry.title
                    )),
            )
            .when(warned, |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.warning))
                        .child("!"),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.open_knowledge_file(path.clone(), cx);
                }),
            )
            .into_any_element()
    }

    /// Left column: roots, the filter, then the open root's entries by kind.
    fn render_knowledge_tree(
        &self,
        stores: &KnowledgeStores,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let mut col = v_flex()
            .id("knowledge-tree")
            .w(px(TREE_WIDTH))
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll()
            .px(px(6.0))
            .pb(px(10.0))
            .border_r_1()
            .border_color(rgb(t.border));

        col = col.child(self.kn_section_label("Roots".into(), cx));
        for root in &stores.roots {
            col = col.child(self.render_knowledge_root_row(root, cx));
        }
        let missing: Vec<_> = stores
            .pointers
            .iter()
            .filter(|p| p.root_key.is_none())
            .collect();
        if !missing.is_empty() {
            col = col.child(self.kn_section_label("Followed, not here".into(), cx));
            for p in missing {
                col = col.child(self.kn_muted(format!("{} → {}", p.project, p.store_id), cx));
            }
        }

        let Some(tree) = self.knowledge.tree.clone() else {
            if self.knowledge.loading {
                col = col.child(self.kn_muted("Loading…", cx));
            }
            return col.into_any_element();
        };

        col = col.child(
            h_flex()
                .pt(px(10.0))
                .gap(px(4.0))
                .items_center()
                .child(
                    okena_ui::input::input_container(&t, None)
                        .flex_1()
                        .min_w_0()
                        .px(px(6.0))
                        .py(px(4.0))
                        .child(SimpleInput::new(&self.knowledge.filter).text_size(ui_text_md(cx))),
                )
                .child(self.add_button(
                    "knowledge-new-entry",
                    "New entry",
                    |this, window, cx| {
                        this.open_new_form(
                            HarnessSection::Knowledge,
                            super::file_ops::NewItem::Knowledge(KnowledgeKind::Doc),
                            window,
                            cx,
                        )
                    },
                    cx,
                )),
        );
        col = col.children(self.render_new_form(HarnessSection::Knowledge, cx));
        for d in &tree.status {
            col = col.child(
                div()
                    .px(px(6.0))
                    .pt(px(6.0))
                    .child(self.kn_diagnostic(d, cx)),
            );
        }

        let filter = self.knowledge.filter.read(cx).value().to_string();
        let groups = group_entries(&tree.entries, &filter, &self.knowledge.collapsed);
        if groups.is_empty() {
            col = col.child(self.kn_muted(
                if tree.entries.is_empty() {
                    "Nothing here yet — add Markdown under docs/, skills/, agents/ or templates/."
                } else {
                    "No entries match."
                },
                cx,
            ));
        }
        for group in groups {
            col = col.child(div().pt(px(6.0)).child(self.render_fold_row(
                group.kind.folder().to_string(),
                group.kind.label().to_string(),
                Some(group.count),
                0,
                cx,
            )));
            for row in group.rows {
                col = col.child(match row {
                    Row::Folder { key, name, depth } => {
                        self.render_fold_row(key, name, None, depth + 1, cx)
                    }
                    Row::Entry { entry, depth } => self.render_entry_row(entry, depth, cx),
                });
            }
        }
        col.into_any_element()
    }

    /// Right column with nothing open: what this root is, its sync state, its
    /// uncommitted changes and its problems.
    fn render_knowledge_overview(
        &self,
        root: &KnowledgeRoot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let mut col = v_flex()
            .id("knowledge-root-overview")
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
                            .child(root.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(kind_label(root.kind)),
                    ),
            );
        if let Some(description) = root.description.clone() {
            col = col.child(
                div()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(description),
            );
        }
        if let Some(id) = root.store_id.clone() {
            col = col.child(self.kn_fact("Id", id, cx));
        }
        col = col.child(self.kn_fact("Path", root.path.clone(), cx));
        if let Some(remote) = root.remote.clone() {
            col = col.child(self.kn_fact("Remote", remote, cx));
        }
        col = col.child(self.kn_fact("Entries", counts_line(root), cx));
        if !root.used_by.is_empty() {
            col = col.child(self.kn_fact("Used by", root.used_by.join(", "), cx));
        }

        if let Some(git) = root.git.clone() {
            col = col.child(self.render_store_git(StoreSection::Knowledge, &git, cx));
        }

        if !root.status.is_empty() {
            col = col.child(
                div()
                    .pt(px(8.0))
                    .child(self.kn_section_label("Problems".into(), cx)),
            );
            for d in &root.status {
                col = col.child(self.kn_diagnostic(d, cx));
            }
        }
        col.child(
            div()
                .pt(px(12.0))
                .text_size(ui_text_md(cx))
                .text_color(rgb(t.text_muted))
                .child("Select an entry to read it."),
        )
        .into_any_element()
    }

    /// Facts above an opened entry: what it is and what an agent picks it by.
    fn render_entry_meta(&self, entry: &KnowledgeEntry, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let mut col = v_flex().gap(px(6.0)).pb(px(12.0)).child(
            h_flex()
                .gap(px(8.0))
                .items_center()
                .child(
                    div()
                        .text_size(ui_text(15.0, cx))
                        .text_color(rgb(t.text_primary))
                        .child(entry.title.clone()),
                )
                .child(self.chip(entry_kind_label(entry.kind).to_string(), t.text_muted, cx)),
        );
        if let Some(description) = entry.description.clone() {
            col = col.child(
                div()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(description),
            );
        }
        if !entry.tags.is_empty() {
            col = col.child(
                h_flex().gap(px(4.0)).flex_wrap().children(
                    entry
                        .tags
                        .iter()
                        .map(|tag| self.chip(tag.clone(), t.border_active, cx)),
                ),
            );
        }
        if !entry.flows.is_empty() {
            col = col.child(self.kn_fact("For", entry.flows.join(", "), cx));
        }
        if !entry.variables.is_empty() {
            col = col.child(
                self.kn_fact(
                    "Variables",
                    entry
                        .variables
                        .iter()
                        .map(|v| format!("{{{v}}}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                    cx,
                ),
            );
        }
        if !entry.files.is_empty() {
            let mut files = v_flex().gap(px(1.0));
            for file in &entry.files {
                let path = file.clone();
                let label = format!(
                    "{}{}",
                    self.unsaved_marker(HarnessSection::Knowledge, file)
                        .unwrap_or_default(),
                    file.strip_prefix(entry.path.trim_end_matches("SKILL.md"))
                        .unwrap_or(file)
                );
                files = files.child(
                    div()
                        .id(SharedString::from(format!("knowledge-file-{file}")))
                        .cursor_pointer()
                        .text_size(ui_text_md(cx))
                        .text_color(rgb(t.text_secondary))
                        .hover(|s| s.text_color(rgb(t.text_primary)))
                        .child(label)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                this.open_knowledge_file(path.clone(), cx);
                            }),
                        ),
                );
            }
            col = col.child(
                h_flex()
                    .gap(px(10.0))
                    .items_start()
                    .child(
                        div()
                            .w(px(72.0))
                            .flex_shrink_0()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child("Files"),
                    )
                    .child(files),
            );
        }
        for d in &entry.status {
            col = col.child(self.kn_diagnostic(d, cx));
        }
        col.into_any_element()
    }

    /// Right column: the opened file, or the root's overview.
    fn render_knowledge_document(
        &self,
        root: Option<&KnowledgeRoot>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let Some(path) = self.knowledge.selected.clone() else {
            return match root {
                Some(root) => self.render_knowledge_overview(root, cx),
                None => v_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(t.text_muted))
                            .child("Select a root."),
                    )
                    .into_any_element(),
            };
        };

        let section = HarnessSection::Knowledge;
        let buffer = self.open_buffer(section);
        let body: AnyElement = match buffer {
            // The source carries the frontmatter the meta block is drawn from,
            // so editing shows the editor alone.
            Some(buffer) if buffer.editing() => self
                .render_document_editor(section, buffer, cx)
                .unwrap_or_else(|| self.info_banner("Loading…".into(), cx)),
            _ => {
                let entry = self
                    .knowledge
                    .tree
                    .as_ref()
                    .and_then(|tree| tree.entry(&path))
                    .cloned();
                let mut page = v_flex()
                    .w_full()
                    .max_w(okena_markdown::DOC_MAX_WIDTH)
                    .min_w_0();
                if let Some(entry) = &entry {
                    page = page.child(self.render_entry_meta(entry, cx));
                }
                page = if let Some(err) = &self.knowledge.content_error {
                    page.child(self.error_banner(err.clone(), cx))
                } else {
                    match buffer {
                        Some(buffer) => page.children(self.render_document_preview(buffer, cx)),
                        None => page.child(self.info_banner("Loading…".into(), cx)),
                    }
                };
                v_flex()
                    .id("knowledge-document-body")
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

        let header = match root {
            Some(root) => format!("{} · {path}", root.name),
            None => path,
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
                    .children(self.render_document_controls(section, cx))
                    .children(self.render_file_controls(section, cx))
                    .child(self.small_button(
                        "knowledge-close-entry",
                        "Overview",
                        cx.listener(|this, _, _window, cx| {
                            this.knowledge.clear_selection();
                            cx.notify();
                        }),
                        cx,
                    )),
            )
            .children(self.render_file_op_bar(section, cx))
            .children(save_error)
            .child(body)
            .into_any_element()
    }

    /// Nothing discovered: say what a store is and where to add one.
    fn render_no_knowledge(&self, stores: &KnowledgeStores, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        v_flex()
            .p(px(20.0))
            .gap(px(8.0))
            .max_w(px(720.0))
            .child(
                div()
                    .text_size(ui_text(15.0, cx))
                    .text_color(rgb(t.text_primary))
                    .child("No knowledge stores yet"),
            )
            .child(self.field_hint_text(
                "A knowledge store is a git repository of your team's engineering \
                 docs, skills, agents and prompt templates, in docs/, skills/, agents/ \
                 and templates/ folders. Clone your team's, add a checkout you already \
                 have, or create one in Settings → Knowledge. A project can also keep \
                 its own under .okena/knowledge/.",
                cx,
            ))
            .children(stores.status.iter().map(|d| self.kn_diagnostic(d, cx)))
            .children(
                stores
                    .pointers
                    .iter()
                    .flat_map(|p| p.status.iter())
                    .map(|d| self.kn_diagnostic(d, cx)),
            )
            .child(self.field_hint_text(&format!("Store registry: {}", stores.registry_path), cx))
            .child(h_flex().pt(px(6.0)).child(self.small_button(
                "knowledge-open-settings",
                "Open knowledge settings",
                cx.listener(|this, _, _window, cx| this.open_settings("knowledge", cx)),
                cx,
            )))
            .into_any_element()
    }

    fn field_hint_text(&self, hint: &str, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(hint.to_string())
            .into_any_element()
    }

    pub(super) fn render_knowledge_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_document_input(HarnessSection::Knowledge, window, cx);
        let t = theme(cx);
        let view = v_flex().size_full();

        let Some(stores) = self.knowledge.stores.clone() else {
            if let Some(err) = self.knowledge.error.clone() {
                return view
                    .child(self.error_banner(err, cx))
                    .child(div().px(px(12.0)).child(self.small_button(
                        "knowledge-retry",
                        "Retry",
                        cx.listener(|this, _, _window, cx| this.refresh_knowledge(cx)),
                        cx,
                    )))
                    .into_any_element();
            }
            return view
                .child(self.info_banner("Loading knowledge…".into(), cx))
                .into_any_element();
        };

        // Writing with an agent takes the whole view, like drafting a spec
        // change does.
        if self.knowledge_draft.open {
            return view
                .child(self.render_knowledge_draft_form(&stores, cx))
                .into_any_element();
        }

        let mut actions = vec![
            self.toolbar_icon(
                "knowledge-settings",
                "icons/settings.svg",
                "Knowledge settings",
                cx.listener(|this, _, _window, cx| this.open_settings("knowledge", cx)),
                cx,
            ),
            self.small_button(
                "knowledge-refresh",
                if self.knowledge.loading {
                    "Refreshing…"
                } else {
                    "Refresh"
                },
                cx.listener(|this, _, _window, cx| this.refresh_knowledge(cx)),
                cx,
            ),
        ];
        // Only offered where knowledge could be written.
        if stores.roots.iter().any(|r| r.healthy) {
            actions.push(
                div()
                    .id("knowledge-new-with-agent")
                    .cursor_pointer()
                    .flex_shrink_0()
                    .px(px(12.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .bg(rgb(t.button_primary_bg))
                    .hover(|s| s.bg(rgb(t.button_primary_hover)))
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(t.button_primary_fg))
                    .child("New with agent")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.open_knowledge_draft(cx)),
                    )
                    .into_any_element(),
            );
        }
        let mut view = view.child(self.render_toolbar(actions, cx));
        if let Some(notice) = self.knowledge_draft.notice.clone() {
            view = view.child(self.info_banner(notice, cx));
        }
        if let Some(err) = self.knowledge.error.clone() {
            view = view.child(self.error_banner(err, cx));
        }
        if stores.roots.is_empty() {
            return view
                .child(self.render_no_knowledge(&stores, cx))
                .into_any_element();
        }

        let open_root = self
            .knowledge
            .root_key
            .as_deref()
            .and_then(|k| stores.root(k))
            .cloned();
        view.child(
            h_flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .bg(rgb(t.bg_primary))
                .child(self.render_knowledge_tree(&stores, cx))
                .child(self.render_knowledge_document(open_root.as_ref(), cx)),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{Group, Row, entry_matches, group_entries};
    use okena_core::knowledge::{KnowledgeEntry, KnowledgeKind};
    use std::collections::HashSet;

    fn entry(kind: KnowledgeKind, name: &str) -> KnowledgeEntry {
        KnowledgeEntry {
            kind,
            path: format!("{}/{name}.md", kind.folder()),
            name: name.into(),
            title: name.rsplit('/').next().unwrap_or(name).into(),
            description: None,
            tags: Vec::new(),
            files: Vec::new(),
            flows: Vec::new(),
            variables: Vec::new(),
            status: Vec::new(),
        }
    }

    /// A compact picture of the rows: `+folder` / `name`, indented by depth.
    fn outline(groups: &[Group]) -> Vec<String> {
        groups
            .iter()
            .flat_map(|g| {
                std::iter::once(format!("[{}] {}", g.kind.label(), g.count)).chain(
                    g.rows.iter().map(|row| match row {
                        Row::Folder { name, depth, .. } => {
                            format!("{}+{name}", "  ".repeat(*depth))
                        }
                        Row::Entry { entry, depth } => {
                            format!("{}{}", "  ".repeat(*depth), entry.title)
                        }
                    }),
                )
            })
            .collect()
    }

    fn sample() -> Vec<KnowledgeEntry> {
        vec![
            entry(KnowledgeKind::Template, "task-start"),
            entry(KnowledgeKind::Doc, "principles"),
            entry(KnowledgeKind::Doc, "ci/pipeline"),
            entry(KnowledgeKind::Doc, "ci/release/tagging"),
            entry(KnowledgeKind::Doc, "architecture/services"),
            entry(KnowledgeKind::Doc, "ci/flaky-tests"),
            entry(KnowledgeKind::Skill, "release"),
        ]
    }

    #[test]
    fn kinds_come_in_fixed_order_and_docs_nest_under_folders_once() {
        let entries = sample();
        let groups = group_entries(&entries, "", &HashSet::new());
        assert_eq!(
            outline(&groups),
            [
                "[Docs] 5",
                "+architecture",
                "  services",
                "+ci",
                "  flaky-tests",
                "  pipeline",
                "  +release",
                "    tagging",
                "principles",
                "[Skills] 1",
                "release",
                "[Templates] 1",
                "task-start",
            ]
        );
    }

    #[test]
    fn folding_hides_rows_but_keeps_the_fold_and_its_count() {
        let entries = sample();
        let collapsed: HashSet<String> = ["docs/ci".to_string(), "skills".to_string()].into();
        let groups = group_entries(&entries, "", &collapsed);
        assert_eq!(
            outline(&groups),
            [
                "[Docs] 5",
                "+architecture",
                "  services",
                "+ci",
                "principles",
                "[Skills] 1",
                "[Templates] 1",
                "task-start",
            ]
        );
    }

    #[test]
    fn filtering_matches_case_insensitively_and_ignores_folds() {
        let mut entries = sample();
        entries[1].description = Some("How we REVIEW code".into());
        entries[6].tags = vec!["Deploy".into()];
        let collapsed: HashSet<String> = ["docs/ci".to_string(), "docs".to_string()].into();

        assert_eq!(
            outline(&group_entries(&entries, "review", &collapsed)),
            ["[Docs] 1", "principles"]
        );
        assert_eq!(
            outline(&group_entries(&entries, "deploy", &collapsed)),
            ["[Skills] 1", "release"]
        );
        assert_eq!(
            outline(&group_entries(&entries, "TAGGING", &collapsed)),
            ["[Docs] 1", "+ci", "  +release", "    tagging"]
        );
        assert!(group_entries(&entries, "nothing-like-this", &collapsed).is_empty());
        assert!(entry_matches(&entries[0], "   "));
    }
}
