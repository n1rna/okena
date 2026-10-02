//! What a `knowledge` origin holds, in the Library: its entries by kind, and
//! the facts above an opened one.
//!
//! Knowledge origins follow ADR-0003's layout — `docs/`, `skills/`, `agents/`,
//! `templates/` — so their tree is grouped by kind rather than by folder. They
//! are also the one origin type that layers: an entry here may be overriding
//! okena's default, or be overridden by an origin above it, and the row says
//! so (`knowledge_override.rs`).
//!
//! The page around this — the origin list, loading, the document panel — is
//! `library_view.rs`, shared with every other origin type.

use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::knowledge::{KnowledgeEntry, KnowledgeKind, KnowledgeTree};
use std::collections::HashSet;

use super::HarnessPane;

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
    /// Its entries, counted before folding.
    pub(crate) count: usize,
    pub(crate) rows: Vec<Row<'a>>,
}

/// Kinds listed with their entries nested under folder rows.
///
/// Docs and templates are the kinds whose names carry a path: `ci/pipeline`,
/// and — since QBL-427 — `partials/context` and `briefs/task-start`. Showing
/// those flat made the partials look like a pile of oddly named templates and
/// left the briefs with nothing to tell them apart. Skills and agents are one
/// name each, so a folder row would only add a level to click through.
const NESTED_KINDS: &[KnowledgeKind] = &[KnowledgeKind::Doc, KnowledgeKind::Template];

/// The list: kinds in their fixed order, empty kinds left out, docs and
/// templates nested under folder rows.
///
/// Always the whole root: narrowing is the island's, which lists its matches
/// across every root in place of this tree (`doc_search.rs`).
pub(crate) fn group_entries<'a>(
    entries: &'a [KnowledgeEntry],
    collapsed: &HashSet<String>,
) -> Vec<Group<'a>> {
    let mut groups = Vec::new();
    for kind in KnowledgeKind::all() {
        let mut matching: Vec<&KnowledgeEntry> =
            entries.iter().filter(|e| e.kind == kind).collect();
        if matching.is_empty() {
            continue;
        }
        matching.sort_by(|a, b| a.path.cmp(&b.path));
        let count = matching.len();
        let mut rows = Vec::new();
        if !collapsed.contains(kind.folder()) {
            if NESTED_KINDS.contains(&kind) {
                nest(kind.folder(), &matching, collapsed, &mut rows);
            } else {
                rows.extend(
                    matching
                        .into_iter()
                        .map(|entry| Row::Entry { entry, depth: 0 }),
                );
            }
        }
        groups.push(Group { kind, count, rows });
    }
    groups
}

/// Nest `entries` under one folder row per directory of their names.
///
/// `kind_folder` is the kind's folder on disk (`docs`, `templates`), which
/// prefixes every fold key so two kinds cannot collapse each other's folders.
fn nest<'a>(
    kind_folder: &str,
    entries: &[&'a KnowledgeEntry],
    collapsed: &HashSet<String>,
    rows: &mut Vec<Row<'a>>,
) {
    let mut open: Vec<&str> = Vec::new();
    for entry in entries {
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
            let key = format!("{kind_folder}/{}", folders[..=depth].join("/"));
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
            hidden = hidden || collapsed.contains(&key);
        }
        if !hidden {
            rows.push(Row::Entry {
                entry,
                depth: folders.len(),
            });
        }
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

// ─── Rendering ──────────────────────────────────────────────────────────────

impl HarnessPane {
    fn render_entry_row(
        &self,
        entry: &KnowledgeEntry,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let selected = self.library.selected.as_deref() == Some(entry.path.as_str());
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
                        self.unsaved_marker(&entry.path).unwrap_or_default(),
                        entry.title
                    )),
            )
            // A template one of your roots overrides is marked here, so the
            // list says which briefs you have taken over without opening one
            // (QBL-426).
            .children(self.render_layering_badge(&entry.path, cx))
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
                    this.open_library_file(path.clone(), cx);
                }),
            )
            .into_any_element()
    }

    /// The rows under the origin list for a knowledge origin: its entries by
    /// kind.
    pub(super) fn render_knowledge_entries(
        &self,
        tree: &KnowledgeTree,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        // No `+` in okena's own store, which is rewritten on every start: a
        // new entry there would not survive it.
        let builtin = self.library_open_origin().is_some_and(|o| o.builtin);
        rows.push(if builtin {
            self.section_label("Entries", cx)
        } else {
            self.tree_heading_with_add(
                "Entries",
                "knowledge-new-entry",
                "New entry",
                |this, window, cx| {
                    this.open_new_form(
                        super::file_ops::NewItem::Knowledge(KnowledgeKind::Doc),
                        window,
                        cx,
                    )
                },
                cx,
            )
        });
        rows.extend(self.render_new_form(cx));
        for d in &tree.status {
            rows.push(
                div()
                    .px(px(6.0))
                    .pt(px(6.0))
                    .child(self.diagnostic_row(d, cx))
                    .into_any_element(),
            );
        }

        let groups = group_entries(&tree.entries, &self.library.collapsed);
        if groups.is_empty() {
            rows.push(self.muted_line(
                "Nothing here yet — add Markdown under docs/, skills/, agents/ or templates/.",
                cx,
            ));
        }
        // Above the entries, under the origin: where a draft will land is the
        // agent's call, so the origin is as near as okena can place it.
        rows.extend(self.render_knowledge_drafts(tree, cx));
        for group in groups {
            rows.push(
                div()
                    .pt(px(6.0))
                    .child(self.render_fold_row(
                        group.kind.folder().to_string(),
                        group.kind.label().to_string(),
                        Some(group.count),
                        0,
                        cx,
                    ))
                    .into_any_element(),
            );
            for row in group.rows {
                rows.push(match row {
                    Row::Folder { key, name, depth } => {
                        self.render_fold_row(key, name, None, depth + 1, cx)
                    }
                    Row::Entry { entry, depth } => self.render_entry_row(entry, depth, cx),
                });
            }
        }
        rows
    }

    /// Facts above an opened entry: what it is and what an agent picks it by.
    pub(super) fn render_entry_meta(
        &self,
        entry: &KnowledgeEntry,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
            col = col.child(self.fact_row("For", entry.flows.join(", "), cx));
        }
        if !entry.variables.is_empty() {
            col = col.child(
                self.fact_row(
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
                    self.unsaved_marker(file).unwrap_or_default(),
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
                                this.open_library_file(path.clone(), cx);
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
        // Every root holding a copy of this file, in layering order, with the
        // one a launch reads marked (QBL-426).
        col = col.children(self.render_layering_list(&entry.path, cx));
        for d in &entry.status {
            col = col.child(self.diagnostic_row(d, cx));
        }
        col.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{Group, Row, group_entries};
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
            models: Default::default(),
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
        let groups = group_entries(&entries, &HashSet::new());
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
        let groups = group_entries(&entries, &collapsed);
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
    fn partials_and_briefs_nest_under_their_own_folders() {
        // QBL-427: a partial is `partials/<name>` and a brief
        // `briefs/<flow>`, so the templates group nests exactly as docs do
        // instead of listing every partial flat beside the briefs.
        let entries = vec![
            entry(KnowledgeKind::Template, "briefs/task-create"),
            entry(KnowledgeKind::Template, "briefs/doc-refine"),
            entry(KnowledgeKind::Template, "partials/context"),
            entry(KnowledgeKind::Template, "partials/coordinate-child"),
            entry(KnowledgeKind::Template, "house-style"),
        ];
        assert_eq!(
            outline(&group_entries(&entries, &HashSet::new())),
            [
                "[Templates] 5",
                "+briefs",
                "  doc-refine",
                "  task-create",
                "house-style",
                "+partials",
                "  context",
                "  coordinate-child",
            ]
        );

        // Folded by the same key the docs folders use, prefixed with the
        // kind's folder so `docs/briefs` and `templates/briefs` are distinct.
        let collapsed: HashSet<String> = ["templates/partials".to_string()].into();
        assert_eq!(
            outline(&group_entries(&entries, &collapsed)),
            [
                "[Templates] 5",
                "+briefs",
                "  doc-refine",
                "  task-create",
                "house-style",
                "+partials",
            ]
        );

        // Skills and agents stay flat: their names carry no path.
        let flat = vec![
            entry(KnowledgeKind::Skill, "release"),
            entry(KnowledgeKind::Agent, "reviewer"),
        ];
        assert_eq!(
            outline(&group_entries(&flat, &HashSet::new())),
            ["[Skills] 1", "release", "[Agents] 1", "reviewer"]
        );
    }
}
