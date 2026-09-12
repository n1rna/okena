//! Filesystem helpers for files people and other tools also touch.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Expand a leading `~`, the form people naturally type into a path field.
pub fn expand_home(p: &str) -> PathBuf {
    let t = p.trim();
    if let Some(home) = dirs::home_dir() {
        if t == "~" {
            return home;
        }
        if let Some(rest) = t.strip_prefix("~/").or_else(|| t.strip_prefix("~\\")) {
            return home.join(rest);
        }
    }
    PathBuf::from(t)
}

/// The canonical form of an existing path, or the path unchanged.
///
/// Registries compare checkouts by canonical path, or the same checkout reached
/// through a symlink reads as two. The Windows verbatim prefix `canonicalize`
/// adds is stripped: nothing a person or another tool writes carries it, so an
/// entry holding it would never match.
pub fn canonical(p: &Path) -> PathBuf {
    match p.canonicalize() {
        Ok(real) => strip_verbatim(real),
        Err(_) => p.to_path_buf(),
    }
}

fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\")
        && !rest.starts_with("UNC\\")
    {
        return PathBuf::from(rest);
    }
    p
}

/// Write through a sibling temp file and rename, so a reader never sees a
/// half-written file. `private` makes the file owner-only on Unix.
pub fn write_atomically(path: &Path, content: &str, private: bool) -> std::io::Result<()> {
    write_via_temp(path, content, private, None)
}

/// A fingerprint of a document's text, handed out with every read and required
/// back with every write.
///
/// FNV-1a rather than `DefaultHasher`, whose output may change between Rust
/// releases: only the daemon computes it, but a revision a client holds across
/// a daemon upgrade should still match an unchanged file. This detects edits
/// made elsewhere; it is not a security boundary.
pub fn content_revision(content: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in content.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}-{}", content.len())
}

/// Why [`replace_if_unchanged`] wrote nothing.
#[derive(Debug)]
pub enum ReplaceError {
    /// The file no longer holds the text the caller's revision was taken from.
    Changed,
    Io(std::io::Error),
}

impl ReplaceError {
    /// The message a person sees for `path`.
    pub fn describe(&self, path: &str) -> String {
        match self {
            Self::Changed => format!(
                "{path} changed on disk since it was opened, so nothing was saved — \
                 revert to load the new version"
            ),
            Self::Io(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                format!("{path} is not a text file")
            }
            Self::Io(e) => format!("could not save {path}: {e}"),
        }
    }
}

/// Replace an existing file's contents, but only while it still matches
/// `expected` (a [`content_revision`]). Returns the new revision.
///
/// Someone else — an agent in a terminal, most often — may have written the
/// file since it was read, and a stale buffer must not silently undo that. The
/// write is atomic and keeps the file's permissions, so a script stays
/// executable.
pub fn replace_if_unchanged(
    path: &Path,
    content: &str,
    expected: &str,
) -> Result<String, ReplaceError> {
    let current = std::fs::read_to_string(path).map_err(ReplaceError::Io)?;
    if content_revision(&current) != expected {
        return Err(ReplaceError::Changed);
    }
    let permissions = std::fs::metadata(path)
        .map_err(ReplaceError::Io)?
        .permissions();
    write_via_temp(path, content, false, Some(permissions)).map_err(ReplaceError::Io)?;
    Ok(content_revision(content))
}

fn write_via_temp(
    path: &Path,
    content: &str,
    private: bool,
    permissions: Option<std::fs::Permissions>,
) -> std::io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // pid + time + a process counter: unique across processes and across
    // threads writing the same file in one instant.
    let tmp = dir.join(format!(
        ".{name}.{}.{nanos}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            if private {
                options.mode(0o600);
            }
        }
        #[cfg(not(unix))]
        let _ = private;
        let mut file = options.open(&tmp)?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.write_all(content.as_bytes())?;
        match file.sync_all() {
            Err(e) if e.kind() == std::io::ErrorKind::Unsupported => {}
            other => other?,
        }
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_writes_replace_the_file_and_leave_no_temp_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested/registry.yaml");
        write_atomically(&path, "a", true).expect("first");
        write_atomically(&path, "b", true).expect("second");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "b");
        let names: Vec<_> = std::fs::read_dir(dir.path().join("nested"))
            .expect("dir")
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }

    #[cfg(unix)]
    #[test]
    fn private_atomic_writes_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("secret.yaml");
        write_atomically(&path, "x", true).expect("write");
        let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn revisions_follow_the_text_not_the_moment() {
        assert_eq!(content_revision("# Hi"), content_revision("# Hi"));
        assert_ne!(content_revision("# Hi"), content_revision("# Hi\n"));
        // Pinned: a revision a client holds must survive a daemon rebuild.
        assert_eq!(content_revision(""), "cbf29ce484222325-0");
    }

    #[test]
    fn a_replace_against_the_current_revision_writes_and_returns_the_next() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("proposal.md");
        std::fs::write(&path, "old").expect("seed");
        let next = replace_if_unchanged(&path, "new", &content_revision("old")).expect("replace");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "new");
        assert_eq!(next, content_revision("new"));
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .expect("dir")
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }

    #[test]
    fn a_replace_over_a_file_changed_since_it_was_read_writes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("proposal.md");
        std::fs::write(&path, "an agent's edit").expect("seed");
        let stale = content_revision("what the editor opened");
        assert!(matches!(
            replace_if_unchanged(&path, "mine", &stale),
            Err(ReplaceError::Changed)
        ));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "an agent's edit"
        );
        assert!(
            ReplaceError::Changed
                .describe("proposal.md")
                .contains("changed on disk")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_replace_keeps_an_executable_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("run.sh");
        std::fs::write(&path, "echo a").expect("seed");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        replace_if_unchanged(&path, "echo b", &content_revision("echo a")).expect("replace");
        let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn home_is_expanded_only_at_the_start() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        assert_eq!(expand_home("~"), home);
        assert_eq!(
            expand_home("  ~/knowledge/eng "),
            home.join("knowledge/eng")
        );
        assert_eq!(expand_home("/abs/~/x"), PathBuf::from("/abs/~/x"));
        assert_eq!(expand_home("~other/x"), PathBuf::from("~other/x"));
    }

    #[test]
    fn verbatim_prefixes_are_stripped_but_unc_paths_are_kept() {
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\C:\k\eng")),
            PathBuf::from(r"C:\k\eng")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\UNC\server\share")),
            PathBuf::from(r"\\?\UNC\server\share")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from("/k/eng")),
            PathBuf::from("/k/eng")
        );
    }
}
