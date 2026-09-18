//! A string store per extension, kept in one JSON file in its data folder
//! and deleted with it.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// The most an extension may keep, keys and values together.
const MAX_BYTES: usize = 8 * 1024 * 1024;

pub struct KvStore {
    path: PathBuf,
    entries: BTreeMap<String, String>,
}

impl KvStore {
    /// Opens the store at `path`. A missing or unreadable file starts empty;
    /// the next write replaces it.
    pub fn open(path: PathBuf) -> Self {
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self { path, entries }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.entries.get(key).cloned()
    }

    pub fn keys(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let size_after = self.size() - self.entries.get(key).map_or(0, |v| key.len() + v.len())
            + key.len()
            + value.len();
        if size_after > MAX_BYTES {
            return Err(format!(
                "storage is full: it holds at most {} MB",
                MAX_BYTES / (1024 * 1024)
            ));
        }
        self.entries.insert(key.to_string(), value.to_string());
        self.save()
    }

    pub fn delete(&mut self, key: &str) {
        if self.entries.remove(key).is_some()
            && let Err(e) = self.save()
        {
            log::warn!("extension storage {}: {e}", self.path.display());
        }
    }

    fn size(&self) -> usize {
        self.entries.iter().map(|(k, v)| k.len() + v.len()).sum()
    }

    fn save(&self) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string(&self.entries).map_err(|e| e.to_string())?;
        // Write then rename, so a crash never leaves half a file.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("saving storage: {e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("saving storage: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_survive_reopening_and_deletes_stick() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data").join("kv.json");
        let mut kv = KvStore::open(path.clone());
        kv.set("a", "1").expect("set");
        kv.set("b", "2").expect("set");
        kv.delete("a");

        let kv = KvStore::open(path);
        assert_eq!(kv.get("a"), None);
        assert_eq!(kv.get("b").as_deref(), Some("2"));
        assert_eq!(kv.keys(), vec!["b".to_string()]);
    }

    #[test]
    fn a_write_past_the_cap_is_refused_and_replacing_counts_the_old_value_out() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut kv = KvStore::open(dir.path().join("kv.json"));
        let big = "x".repeat(MAX_BYTES - 10);
        kv.set("k", &big).expect("fits");
        assert!(kv.set("other", "0123456789").is_err());
        // Replacing the big value with a small one frees the room.
        kv.set("k", "small").expect("replace");
        kv.set("other", "0123456789").expect("fits now");
    }
}
