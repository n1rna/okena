//! Freeform Library origins on disk: any folder of markdown (QBL-440).
//!
//! The third origin type, beside knowledge stores and OpenSpec roots
//! ([`crate::library`]). It promises nothing about what is inside — no layout,
//! no identity file — so all there is to say about one is which markdown files
//! it holds and whether a path names a file inside it. Both live here, below
//! everything that needs them: the daemon lists and reads an origin through
//! them, and the launch-context index reads the same list.

use crate::diagnostic::Diagnostic;
use crate::library::{FreeformDoc, FreeformTree};
use std::path::{Path, PathBuf};

/// Most documents one origin lists. A folder past this is not a set of notes;
/// the rest are still reachable by path and by the daemon's own search.
pub const MAX_DOCUMENTS: usize = 2000;

/// How deep the walk goes below the origin.
const MAX_DEPTH: usize = 12;

/// How much of a file is read to find its title.
const HEAD_BYTES: usize = 4 * 1024;

/// Folders the walk never enters: build output and dependency trees, which
/// hold markdown nobody wrote.
const SKIPPED_DIRS: &[&str] = &["node_modules", "target", "dist", "build", "vendor"];

/// Whether `path` is a markdown file by its extension.
pub fn is_markdown(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|e| matches!(e.as_str(), "md" | "markdown" | "mdx"))
}

/// Resolve a client-supplied relative path to an existing file inside `root`.
///
/// Checked on the canonical path, so neither `..` nor a symlink can escape.
pub fn resolve_document(root: &Path, path: &str) -> Result<PathBuf, String> {
    let real = root
        .join(path)
        .canonicalize()
        .map_err(|_| format!("no such document: {path}"))?;
    let real_root = root
        .canonicalize()
        .map_err(|e| format!("origin is unreadable: {e}"))?;
    if !real.starts_with(&real_root) {
        return Err("path is outside the origin".into());
    }
    if !real.is_file() {
        return Err(format!("not a file: {path}"));
    }
    Ok(real)
}

/// Every markdown file under `root`, sorted by path.
pub fn read_tree(root: &Path) -> FreeformTree {
    let mut documents = Vec::new();
    let mut truncated = false;
    walk(root, root, 0, &mut documents, &mut truncated);
    documents.sort_by(|a: &FreeformDoc, b| a.path.cmp(&b.path));
    let mut status = Vec::new();
    if truncated {
        status.push(
            Diagnostic::warning(
                "document_limit",
                format!("Only the first {MAX_DOCUMENTS} documents are listed."),
            )
            .with_fix("Add a narrower folder as the origin."),
        );
    }
    FreeformTree {
        root_key: String::new(),
        root: root.to_string_lossy().into_owned(),
        documents,
        status,
    }
}

fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<FreeformDoc>, truncated: &mut bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    // Sorted, so the cap keeps the same documents every time.
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Hidden names are never listed: `.git`, `.obsidian`, editor state.
        if name.starts_with('.') {
            continue;
        }
        // `file_type` does not follow symlinks, and neither does the walk: a
        // link could lead outside the origin, which a read would then refuse.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            if depth < MAX_DEPTH && !SKIPPED_DIRS.contains(&name.as_str()) {
                walk(root, &path, depth + 1, out, truncated);
            }
        } else if kind.is_file() && is_markdown(&path) {
            if out.len() >= MAX_DOCUMENTS {
                *truncated = true;
                return;
            }
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            out.push(FreeformDoc {
                title: title_of(&path, &name),
                path: rel,
            });
        }
        if *truncated {
            return;
        }
    }
}

/// What to call a document: frontmatter `title`, else its first `#` heading,
/// else its file name without the extension. Only the head of the file is
/// read, so a listing stays cheap.
fn title_of(path: &Path, file_name: &str) -> String {
    use std::io::Read;
    let fallback = || {
        Path::new(file_name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| file_name.to_string())
    };
    let mut head = vec![0u8; HEAD_BYTES];
    let read = std::fs::File::open(path)
        .and_then(|mut f| f.read(&mut head))
        .unwrap_or(0);
    let head = String::from_utf8_lossy(&head[..read]);
    let mut lines = head.lines();
    let mut first = lines.next();
    if first.map(str::trim) == Some("---") {
        // Frontmatter: a `title:` in it wins, and the body starts after it.
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
            if let Some(title) = line.strip_prefix("title:") {
                let title = title.trim().trim_matches(['"', '\'']).trim();
                if !title.is_empty() {
                    return title.to_string();
                }
            }
        }
        first = lines.next();
    }
    first
        .into_iter()
        .chain(lines)
        .find_map(|line| line.strip_prefix("# ").map(str::trim))
        .filter(|title| !title.is_empty())
        .map(str::to_string)
        .unwrap_or_else(fallback)
}


#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "okena-core-freeform-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn markdown_is_told_by_its_extension_whatever_its_case() {
        for yes in ["a.md", "B.MD", "notes/c.markdown", "d.mdx"] {
            assert!(is_markdown(Path::new(yes)), "{yes}");
        }
        for no in ["run.sh", "md", "README", "a.md.bak"] {
            assert!(!is_markdown(Path::new(no)), "{no}");
        }
    }

    #[test]
    fn the_cap_keeps_the_first_documents_and_says_so() {
        let root = tmpdir("cap");
        for n in 0..MAX_DOCUMENTS + 3 {
            write(&root.join(format!("notes/{n:05}.md")), "");
        }
        let tree = read_tree(&root);
        assert_eq!(tree.documents.len(), MAX_DOCUMENTS);
        assert_eq!(tree.documents[0].path, "notes/00000.md");
        assert_eq!(tree.status[0].code, "document_limit");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_document_resolves_only_to_a_file_inside_the_origin() {
        let root = tmpdir("resolve");
        write(&root.join("a/b.md"), "x");
        write(&root.parent().unwrap().join("okena-core-freeform-outside.md"), "secret");
        assert!(resolve_document(&root, "a/b.md").is_ok());
        assert!(resolve_document(&root, "a").unwrap_err().contains("not a file"));
        assert!(resolve_document(&root, "a/gone.md").unwrap_err().contains("no such document"));
        assert!(
            resolve_document(&root, "../okena-core-freeform-outside.md")
                .unwrap_err()
                .contains("outside the origin")
        );
        std::fs::remove_file(root.parent().unwrap().join("okena-core-freeform-outside.md")).ok();
        std::fs::remove_dir_all(&root).ok();
    }
}
