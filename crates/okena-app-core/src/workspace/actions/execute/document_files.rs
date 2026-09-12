//! Create, rename and delete for the files the Specs and Knowledge trees list.
//!
//! Each section resolves its root its own way; what happens inside the root is
//! the same for both, and so are the checks. A path that must name an existing
//! file goes through the section's `resolve_document` — canonical, so neither
//! `..` nor a symlink leads out — and a path for something new goes through
//! `okena_core::fs::resolve_new_path`. Nothing here replaces a file that
//! exists, and delete removes one file, never a folder.

use super::ActionResult;
use okena_core::fs;
use std::path::{Path, PathBuf};

/// A section's check that `path` names an existing file inside `root`.
pub(super) type ResolveExisting = fn(&Path, &str) -> Result<PathBuf, String>;

fn io_error(verb: &str, path: &str, e: std::io::Error) -> ActionResult {
    ActionResult::Err(match e.kind() {
        std::io::ErrorKind::AlreadyExists => format!("`{path}` already exists"),
        _ => format!("could not {verb} `{path}`: {e}"),
    })
}

/// Create `path` holding `content`. Replies with the normalized path and the
/// new file's revision.
pub(super) fn create_file(
    root_key: &str,
    root: &Path,
    path: &str,
    content: &str,
    max_bytes: u64,
) -> ActionResult {
    if content.len() as u64 > max_bytes {
        return ActionResult::Err(format!(
            "`{path}` is too large to save ({} KB)",
            content.len() / 1024
        ));
    }
    let target = match fs::resolve_new_path(root, path) {
        Ok(t) => t,
        Err(e) => return ActionResult::Err(e),
    };
    let rel = fs::normalize_relative(path).unwrap_or_default();
    match fs::create_new_file(&target, content) {
        Ok(revision) => ActionResult::Ok(Some(serde_json::json!({
            "root": root_key,
            "path": rel,
            "revision": revision,
        }))),
        Err(e) => io_error("create", &rel, e),
    }
}

/// Create the folder `path`, with the folders above it.
pub(super) fn create_folder(root_key: &str, root: &Path, path: &str) -> ActionResult {
    let target = match fs::resolve_new_path(root, path) {
        Ok(t) => t,
        Err(e) => return ActionResult::Err(e),
    };
    let rel = fs::normalize_relative(path).unwrap_or_default();
    match std::fs::create_dir_all(&target) {
        Ok(()) => ActionResult::Ok(Some(serde_json::json!({
            "root": root_key,
            "path": rel,
        }))),
        Err(e) => io_error("create", &rel, e),
    }
}

/// Move the file at `from` to `to`. Replies with the normalized new path.
pub(super) fn rename(
    root_key: &str,
    root: &Path,
    resolve: ResolveExisting,
    from: &str,
    to: &str,
) -> ActionResult {
    // The move acts on the path as named, not on where a symlink points: the
    // canonical check only proves it is a file inside the root.
    let from_rel = match fs::normalize_relative(from) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    if let Err(e) = resolve(root, &from_rel) {
        return ActionResult::Err(e);
    }
    let target = match fs::resolve_new_path(root, to) {
        Ok(t) => t,
        Err(e) => return ActionResult::Err(e),
    };
    let to_rel = fs::normalize_relative(to).unwrap_or_default();
    match fs::rename_without_replacing(&root.join(&from_rel), &target) {
        Ok(()) => ActionResult::Ok(Some(serde_json::json!({
            "root": root_key,
            "from": from_rel,
            "path": to_rel,
        }))),
        Err(e) => io_error("rename", &to_rel, e),
    }
}

/// Delete the file at `path`.
pub(super) fn delete(
    root_key: &str,
    root: &Path,
    resolve: ResolveExisting,
    path: &str,
) -> ActionResult {
    let rel = match fs::normalize_relative(path) {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(e),
    };
    // `resolve_document` refuses anything but a file, so a folder never goes.
    if let Err(e) = resolve(root, &rel) {
        return ActionResult::Err(e);
    }
    match std::fs::remove_file(root.join(&rel)) {
        Ok(()) => ActionResult::Ok(Some(serde_json::json!({
            "root": root_key,
            "path": rel,
        }))),
        Err(e) => io_error("delete", &rel, e),
    }
}
