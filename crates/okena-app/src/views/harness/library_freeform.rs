//! What a `freeform` origin holds, in the Library: its markdown files, under
//! the folders they sit in.
//!
//! A freeform origin promises no layout, so its tree is the folder's own: no
//! kinds, no changes, no archive, and nothing to override. A document is
//! added with the tree's `+` and written in the editor, or written by an
//! agent started with "New", whose Drafting row stands here until its files
//! show up.
//!
//! The page around this — the origin list, loading, the document panel — is
//! `library_view.rs`, shared with every other origin type.

use gpui::*;
use okena_core::library::{FreeformDoc, FreeformTree};
use std::collections::HashSet;

use super::HarnessPane;

/// One line of a freeform origin's tree.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FreeformRow<'a> {
    /// A folder, keyed by its path for collapsing.
    Folder {
        key: String,
        name: String,
        depth: usize,
    },
    Document {
        doc: &'a FreeformDoc,
        depth: usize,
    },
}

/// The documents nested under one folder row per directory of their paths.
///
/// `documents` is sorted by path, as the daemon lists them, which is what
/// puts every file of a folder together. A folded folder hides everything
/// beneath it, its own sub-folders included.
pub(crate) fn freeform_rows<'a>(
    documents: &'a [FreeformDoc],
    collapsed: &HashSet<String>,
) -> Vec<FreeformRow<'a>> {
    let mut rows = Vec::new();
    let mut open: Vec<&str> = Vec::new();
    for doc in documents {
        let folders: Vec<&str> = doc
            .path
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
            let key = folders[..=depth].join("/");
            if depth >= shared && !hidden {
                rows.push(FreeformRow::Folder {
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
            rows.push(FreeformRow::Document {
                doc,
                depth: folders.len(),
            });
        }
    }
    rows
}

impl HarnessPane {
    /// The rows under the origin list for a freeform origin: its documents.
    pub(super) fn render_freeform_entries(
        &self,
        tree: &FreeformTree,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows = vec![self.tree_heading_with_add(
            "Documents",
            "freeform-new-document",
            "New document",
            |this, window, cx| {
                this.open_new_form(super::file_ops::NewItem::Freeform, window, cx)
            },
            cx,
        )];
        rows.extend(self.render_new_form(cx));
        rows.extend(self.render_freeform_drafts(tree, cx));
        for d in &tree.status {
            rows.push(
                div()
                    .px(px(6.0))
                    .pt(px(6.0))
                    .child(self.diagnostic_row(d, cx))
                    .into_any_element(),
            );
        }
        if tree.documents.is_empty() {
            rows.push(self.muted_line("No markdown here yet — add a document with +.", cx));
        }
        for row in freeform_rows(&tree.documents, &self.library.collapsed) {
            rows.push(match row {
                FreeformRow::Folder { key, name, depth } => {
                    self.render_fold_row(key, name, None, depth, cx)
                }
                FreeformRow::Document { doc, depth } => self.render_document_row(
                    &doc.path,
                    &doc.title,
                    20.0 + 12.0 * depth as f32,
                    Vec::new(),
                    cx,
                ),
            });
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::{FreeformRow, freeform_rows};
    use okena_core::library::FreeformDoc;
    use std::collections::HashSet;

    fn docs(paths: &[&str]) -> Vec<FreeformDoc> {
        paths
            .iter()
            .map(|p| FreeformDoc {
                path: (*p).to_string(),
                title: p.rsplit('/').next().unwrap_or(p).trim_end_matches(".md").to_string(),
            })
            .collect()
    }

    /// A compact picture of the rows: `+folder` / `title`, indented by depth.
    fn outline(rows: &[FreeformRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                FreeformRow::Folder { name, depth, .. } => {
                    format!("{}+{name}", "  ".repeat(*depth))
                }
                FreeformRow::Document { doc, depth } => {
                    format!("{}{}", "  ".repeat(*depth), doc.title)
                }
            })
            .collect()
    }

    fn sample() -> Vec<FreeformDoc> {
        docs(&[
            "README.md",
            "adr/0001-daemon.md",
            "adr/0002-windows.md",
            "workflows/release.md",
            "workflows/release/hotfix.md",
        ])
    }

    #[test]
    fn documents_nest_under_the_folders_of_their_paths_each_listed_once() {
        let documents = sample();
        assert_eq!(
            outline(&freeform_rows(&documents, &HashSet::new())),
            [
                "README",
                "+adr",
                "  0001-daemon",
                "  0002-windows",
                "+workflows",
                "  release",
                "  +release",
                "    hotfix",
            ]
        );
    }

    #[test]
    fn a_folded_folder_hides_what_is_under_it_and_stays_listed() {
        let documents = sample();
        let collapsed: HashSet<String> = ["workflows".to_string()].into();
        assert_eq!(
            outline(&freeform_rows(&documents, &collapsed)),
            ["README", "+adr", "  0001-daemon", "  0002-windows", "+workflows"]
        );
        // Folding a sub-folder leaves its siblings showing.
        let collapsed: HashSet<String> = ["workflows/release".to_string()].into();
        assert_eq!(
            outline(&freeform_rows(&documents, &collapsed)),
            [
                "README",
                "+adr",
                "  0001-daemon",
                "  0002-windows",
                "+workflows",
                "  release",
                "  +release",
            ]
        );
    }

    #[test]
    fn folders_are_keyed_by_their_whole_path_so_two_of_one_name_fold_apart() {
        let documents = docs(&["a/notes/x.md", "b/notes/y.md"]);
        let keys: Vec<String> = freeform_rows(&documents, &HashSet::new())
            .into_iter()
            .filter_map(|row| match row {
                FreeformRow::Folder { key, .. } => Some(key),
                FreeformRow::Document { .. } => None,
            })
            .collect();
        assert_eq!(keys, ["a", "a/notes", "b", "b/notes"]);
    }
}
