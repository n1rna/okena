//! Installed extensions on disk.
//!
//! ```text
//! <profile>/extensions/
//! ├── installed.json          the registry: source, resolved commit, approved permissions
//! ├── installed/<id>/         extension.toml + extension.wasm, as installed
//! ├── data/<id>/kv.json       the extension's storage; deleted with it
//! └── cache/
//!     ├── repos/<hash>/       git checkouts, one per source URL
//!     └── target/             the cargo target dir builds share
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use okena_core::extension::{ExtPermissions, ExtSource};
use serde::{Deserialize, Serialize};

use crate::manifest::{MANIFEST_FILE, PREBUILT_WASM};

#[derive(Clone, Debug)]
pub struct Dirs {
    root: PathBuf,
    build_target: Option<PathBuf>,
}

impl Dirs {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            build_target: None,
        }
    }

    /// Builds use `target` instead of the cache's own (tests share one).
    pub fn with_build_target(mut self, target: PathBuf) -> Self {
        self.build_target = Some(target);
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn registry(&self) -> PathBuf {
        self.root.join("installed.json")
    }

    pub fn installed(&self, id: &str) -> PathBuf {
        self.root.join("installed").join(id)
    }

    pub fn installed_manifest(&self, id: &str) -> PathBuf {
        self.installed(id).join(MANIFEST_FILE)
    }

    pub fn installed_wasm(&self, id: &str) -> PathBuf {
        self.installed(id).join(PREBUILT_WASM)
    }

    pub fn data(&self, id: &str) -> PathBuf {
        self.root.join("data").join(id)
    }

    pub fn kv(&self, id: &str) -> PathBuf {
        self.data(id).join("kv.json")
    }

    /// The checkout for `url`, one per URL.
    pub fn repo_cache(&self, url: &str) -> PathBuf {
        let name = okena_git::clone_dir_name(url).unwrap_or_else(|| "repo".into());
        let safe: String = name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        self.root
            .join("cache/repos")
            .join(format!("{safe}-{:016x}", fnv1a(url.trim().as_bytes())))
    }

    pub fn build_target(&self) -> PathBuf {
        self.build_target
            .clone()
            .unwrap_or_else(|| self.root.join("cache/target"))
    }
}

/// A stable hash for cache folder names.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// One installed extension, as the registry records it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledRecord {
    pub id: String,
    pub version: String,
    /// Where it came from, with the commit it was installed at.
    pub source: ExtSource,
    /// What the user approved. Enforced at runtime, whatever the manifest says.
    pub approved: ExtPermissions,
    #[serde(default)]
    pub installed_at_ms: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    version: u32,
    #[serde(default)]
    extensions: BTreeMap<String, InstalledRecord>,
}

/// Reads the registry. A missing file is an empty registry; an unreadable
/// one is logged and treated as empty so okena still starts.
pub fn load_registry(dirs: &Dirs) -> BTreeMap<String, InstalledRecord> {
    let path = dirs.registry();
    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<RegistryFile>(&text) {
            Ok(file) => file.extensions,
            Err(e) => {
                log::error!("extensions registry {} is unreadable: {e}", path.display());
                BTreeMap::new()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => {
            log::error!("cannot read extensions registry {}: {e}", path.display());
            BTreeMap::new()
        }
    }
}

pub fn save_registry(dirs: &Dirs, records: &BTreeMap<String, InstalledRecord>) -> Result<(), String> {
    let file = RegistryFile {
        version: 1,
        extensions: records.clone(),
    };
    let text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    write_atomic(&dirs.registry(), text.as_bytes())
}

/// Writes then renames, so a crash never leaves half a file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("writing {}: {e}", path.display()))
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_round_trips_and_a_corrupt_one_reads_as_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dirs = Dirs::new(dir.path().to_path_buf());
        assert!(load_registry(&dirs).is_empty());

        let mut records = BTreeMap::new();
        records.insert(
            "x".to_string(),
            InstalledRecord {
                id: "x".into(),
                version: "0.1.0".into(),
                source: ExtSource::Git {
                    url: "https://example.com/lib.git".into(),
                    git_ref: Some("main".into()),
                    path: Some("extensions/x".into()),
                    commit: "abc".into(),
                },
                approved: ExtPermissions::default(),
                installed_at_ms: 1,
            },
        );
        save_registry(&dirs, &records).expect("save");
        assert_eq!(load_registry(&dirs), records);

        std::fs::write(dir.path().join("installed.json"), "{nope").expect("write");
        assert!(load_registry(&dirs).is_empty());
    }

    #[test]
    fn each_url_gets_its_own_cache_folder() {
        let dirs = Dirs::new(PathBuf::from("/x"));
        let a = dirs.repo_cache("https://github.com/acme/lib.git");
        let b = dirs.repo_cache("git@github.com:acme/lib.git");
        assert_ne!(a, b);
        assert!(a.file_name().expect("name").to_string_lossy().starts_with("lib-"));
        assert_eq!(a, dirs.repo_cache(" https://github.com/acme/lib.git "));
    }
}
