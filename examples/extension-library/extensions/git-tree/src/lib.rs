//! git-tree: a repository's contents as a tree with a detail pane and charts,
//! read from a configured local clone or a GitHub remote through `gh`.

use std::collections::BTreeMap;

use okena_extension_api::{
    self as okena, Command, Extension, Info, Query, Refresh, Tone, host, serde_json, ui,
};
use serde::Deserialize;

/// Folders listed but not opened: they hold build output or history.
const SKIPPED: &[&str] = &[".git", "target", "node_modules", ".venv", "dist", "build"];
const MAX_ENTRIES: usize = 4_000;

#[derive(Default, Deserialize)]
struct Config {
    #[serde(default)]
    source: String,
    #[serde(default)]
    local_path: String,
    #[serde(default)]
    remote_repo: String,
    #[serde(default)]
    remote_ref: String,
    #[serde(default = "default_depth")]
    max_depth: f64,
}

fn default_depth() -> f64 {
    4.0
}

/// One file or folder, by its path from the root.
#[derive(Clone, Debug)]
struct Entry {
    path: String,
    is_dir: bool,
    size: u64,
}

struct Listing {
    title: String,
    entries: Vec<Entry>,
    truncated: bool,
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

fn extension_of(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_lowercase(),
        _ => "(none)".into(),
    }
}

struct GitTree;

impl GitTree {
    fn list_local(root: &str, max_depth: usize) -> okena::Result<Listing> {
        let mut entries = Vec::new();
        let mut truncated = false;
        let mut stack = vec![(String::new(), 0usize)];
        while let Some((relative, depth)) = stack.pop() {
            let dir = if relative.is_empty() { root.to_string() } else { format!("{root}/{relative}") };
            for entry in host::read_dir(&dir)? {
                if entries.len() >= MAX_ENTRIES {
                    truncated = true;
                    break;
                }
                let path = if relative.is_empty() { entry.name.clone() } else { format!("{relative}/{}", entry.name) };
                if entry.is_dir && depth + 1 < max_depth && !SKIPPED.contains(&entry.name.as_str()) {
                    stack.push((path.clone(), depth + 1));
                }
                entries.push(Entry { path, is_dir: entry.is_dir, size: entry.size });
            }
        }
        Ok(Listing { title: root.to_string(), entries, truncated })
    }

    fn list_remote(repo: &str, git_ref: &str, max_depth: usize) -> okena::Result<Listing> {
        let git_ref = if git_ref.is_empty() {
            Command::new("gh")
                .args(["api", &format!("repos/{repo}"), "--jq", ".default_branch"])
                .run()?
                .trim()
                .to_string()
        } else {
            git_ref.to_string()
        };
        #[derive(Deserialize)]
        struct Tree {
            tree: Vec<Item>,
            #[serde(default)]
            truncated: bool,
        }
        #[derive(Deserialize)]
        struct Item {
            path: String,
            #[serde(rename = "type")]
            kind: String,
            #[serde(default)]
            size: u64,
        }
        let raw = Command::new("gh")
            .args(["api", &format!("repos/{repo}/git/trees/{git_ref}?recursive=1")])
            .timeout_ms(120_000)
            .run()?;
        let tree: Tree = serde_json::from_str(&raw).map_err(|e| format!("gh returned something unexpected: {e}"))?;
        let entries: Vec<Entry> = tree
            .tree
            .into_iter()
            .filter(|i| i.path.split('/').count() <= max_depth)
            .take(MAX_ENTRIES)
            .map(|i| Entry { is_dir: i.kind == "tree", path: i.path, size: i.size })
            .collect();
        Ok(Listing { title: format!("{repo}@{git_ref}"), truncated: tree.truncated || entries.len() >= MAX_ENTRIES, entries })
    }

    fn listing() -> okena::Result<Listing> {
        let config: Config = host::config()?;
        let depth = (config.max_depth.max(1.0) as usize).min(12);
        match config.source.as_str() {
            "remote" => {
                let repo = config.remote_repo.trim();
                if !repo.contains('/') {
                    return Err("set GitHub repository to owner/name in the extension's settings".into());
                }
                Self::list_remote(repo, config.remote_ref.trim(), depth)
            }
            _ => {
                let path = config.local_path.trim();
                if path.is_empty() {
                    return Err("set Local clone in the extension's settings".into());
                }
                Self::list_local(path, depth)
            }
        }
    }
}

impl Extension for GitTree {
    fn new() -> Self {
        GitTree
    }

    fn describe(&self) -> Info {
        Info::new().query(Query::new("summary", "Files, folders and bytes by file type"))
    }

    fn refresh(&mut self) -> okena::Result<Refresh> {
        let mut listing = Self::listing()?;
        listing.entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.path.cmp(&b.path)));

        // The tree: every entry under its parent folder.
        let mut tree = ui::Tree::new("files").with_detail_pane().empty_text("The repository is empty");
        let mut folders: BTreeMap<String, ui::ItemId> = BTreeMap::new();
        let mut by_extension: BTreeMap<String, u64> = BTreeMap::new();
        let mut per_depth: BTreeMap<usize, u64> = BTreeMap::new();
        let (mut files, mut dirs, mut bytes) = (0u64, 0u64, 0u64);
        // Parents first, so every folder exists before what it holds.
        let mut ordered = listing.entries.clone();
        ordered.sort_by_key(|e| e.path.matches('/').count());
        for entry in &ordered {
            let (parent, name) = match entry.path.rsplit_once('/') {
                Some((parent, name)) => (Some(parent), name),
                None => (None, entry.path.as_str()),
            };
            let parent_id = parent.and_then(|p| folders.get(p).copied());
            let depth = entry.path.matches('/').count();
            let mut item = ui::TreeItem::new(&entry.path, name).detail(ui::Field::new("Path", &entry.path));
            if entry.is_dir {
                dirs += 1;
                item = item.detail(ui::Field::new("Type", "folder"));
                if depth == 0 {
                    item = item.expanded();
                }
            } else {
                files += 1;
                bytes += entry.size;
                *by_extension.entry(extension_of(&entry.path)).or_default() += entry.size;
                *per_depth.entry(depth).or_default() += 1;
                item = item
                    .hint(human(entry.size))
                    .detail(ui::Field::new("Type", "file"))
                    .detail(ui::Field::new("Size", human(entry.size)))
                    .detail(ui::Field::new("File type", extension_of(&entry.path)));
                if entry.size > 1024 * 1024 {
                    item = item.badge("large", Tone::Warning);
                }
            }
            let id = tree.add(parent_id, item);
            if entry.is_dir {
                folders.insert(entry.path.clone(), id);
            }
        }

        let mut top: Vec<(String, u64)> = by_extension.into_iter().collect();
        top.sort_by(|a, b| b.1.cmp(&a.1));
        top.truncate(10);
        let bar = ui::Chart::new()
            .title("Bytes by file type")
            .series("bytes", top.iter().map(|(ext, size)| (ext.clone(), *size as f64)))
            .bar();
        let line = ui::Chart::new()
            .title("Files per folder depth")
            .series("files", per_depth.iter().map(|(d, n)| (format!("depth {d}"), *n as f64)))
            .line();

        let mut view = ui::View::new();
        let heading = view.add(ui::heading(&listing.title));
        let note = view.add(if listing.truncated {
            ui::toned(format!("Showing the first {} entries.", listing.entries.len()), Tone::Warning)
        } else {
            ui::muted(format!("{} entries", listing.entries.len()))
        });
        let stats = view.add(ui::stats([
            ui::Stat::new("Files", files.to_string()),
            ui::Stat::new("Folders", dirs.to_string()),
            ui::Stat::new("Size", human(bytes)),
        ]));
        let tree = view.add(tree.build());
        let bar = view.add(bar);
        let line = view.add(line);
        let charts = view.columns([bar, line]);
        let charts = view.section("Charts", [charts], true);
        let root = view.stack([heading, note, stats, tree, charts]);
        Ok(Refresh::new(view.finish(root)).status(format!("{files} files"), None))
    }

    fn query(&mut self, id: &str, _args: serde_json::Value) -> okena::Result<serde_json::Value> {
        match id {
            "summary" => {
                let listing = Self::listing()?;
                let files = listing.entries.iter().filter(|e| !e.is_dir).count();
                let bytes: u64 = listing.entries.iter().map(|e| e.size).sum();
                Ok(serde_json::json!({ "root": listing.title, "files": files, "bytes": bytes }))
            }
            other => Err(format!("unknown query {other}")),
        }
    }
}

okena::register_extension!(GitTree);
