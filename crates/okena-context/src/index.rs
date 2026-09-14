//! The live index the daemon searches, backed by fff.
//!
//! A root's items are read the first time a search reaches it and cached.
//! While cached, an fff `FilePicker` watches the root, and any change under it
//! makes the next search read the root again — so editing `project-map.yaml`
//! or adding a doc shows up without a restart. A root nobody has searched for
//! a while is dropped with its picker, so a workspace of a hundred projects
//! does not keep a hundred watchers alive.
//!
//! Matching is fff's (`neo_frizbee`) over an item's title, its map id or path
//! and its owner; frecency is fff's LMDB tracker, fed by [`ContextIndex::record_hit`].

use crate::catalog::{Catalog, CatalogRoot, RootKind};
use fff_search::{
    FFFMode, FilePicker, FilePickerOptions, FrecencyTracker, SharedFilePicker, SharedFrecency,
    WatchId, WatchOptions,
};
use okena_core::context::{
    ContextItem, ContextKind, ContextRef, ContextSearchResult, UnmappedProject,
};
use okena_core::project_map::MapStatus;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Results returned when the caller does not say.
pub const DEFAULT_LIMIT: usize = 50;
/// A root unsearched for this long loses its cache and its watcher.
const IDLE: Duration = Duration::from_secs(10 * 60);
/// Read a root again after this long even without a watcher event: fff does
/// not watch gitignored paths, and a root at `$HOME` gets no picker at all.
const STALE: Duration = Duration::from_secs(30);

/// A watched directory and the change counter its watcher bumps.
struct Watch {
    picker: SharedFilePicker,
    changes: Arc<AtomicU64>,
    subscription: Option<WatchId>,
}

struct Cached {
    items: Vec<ContextItem>,
    map_status: Option<MapStatus>,
    read_at: Instant,
    /// The watch counter when read; a higher one means the root changed.
    changes_seen: u64,
    used_at: Instant,
}

#[derive(Default)]
struct State {
    /// By root kind and directory. A project's map root and its knowledge
    /// root are one directory read two ways.
    cached: HashMap<(RootKind, Option<PathBuf>, String), Cached>,
    /// By directory, shared by every root read from it.
    watches: HashMap<PathBuf, Watch>,
}

pub struct ContextIndex {
    frecency: SharedFrecency,
    state: Mutex<State>,
    stale: Duration,
    idle: Duration,
}

impl ContextIndex {
    /// An index whose frecency persists in `frecency_dir`. Without one — or
    /// when the database cannot be opened — search still works, unranked by
    /// use.
    pub fn open(frecency_dir: Option<&Path>) -> Self {
        let frecency = SharedFrecency::default();
        if let Some(dir) = frecency_dir {
            let opened = std::fs::create_dir_all(dir)
                .map_err(|e| e.to_string())
                .and_then(|()| FrecencyTracker::open(dir).map_err(|e| e.to_string()))
                .and_then(|tracker| frecency.init(tracker).map_err(|e| e.to_string()));
            if let Err(e) = opened {
                log::warn!("[context] frecency unavailable at {}: {e}", dir.display());
            }
        }
        Self {
            frecency,
            state: Mutex::new(State::default()),
            stale: STALE,
            idle: IDLE,
        }
    }

    /// Search every root of `catalog`.
    ///
    /// Items from roots `chosen` owns or follows come first, then the rest;
    /// within each band, match quality plus frecency. With `scoped`, roots
    /// outside `chosen` are not searched at all — an agent's own lookup.
    pub fn search(
        &self,
        catalog: &Catalog,
        query: &str,
        chosen: &[String],
        scoped: bool,
        limit: usize,
    ) -> ContextSearchResult {
        let query = query.trim();
        let mut unmapped = Vec::new();
        let mut candidates: Vec<ContextItem> = Vec::new();
        for root in &catalog.roots {
            let is_chosen = Catalog::is_chosen(root, chosen);
            if scoped && !is_chosen {
                continue;
            }
            let (items, map_status) = self.items_of(root);
            if root.kind == RootKind::Map
                && is_chosen
                && map_status == Some(MapStatus::NotScanned)
                && let Some(project) = root.owner.owner.project_id()
            {
                unmapped.push(project.to_string());
            }
            candidates.extend(items.into_iter().map(|mut item| {
                item.chosen = is_chosen;
                item
            }));
        }
        self.sweep();

        let scores = self.scores(query, &candidates);
        let mut ranked: Vec<(ContextItem, i64)> = candidates
            .into_iter()
            .zip(scores)
            .filter_map(|(item, score)| score.map(|s| (item, s)))
            .collect();
        ranked.sort_by(|(a, sa), (b, sb)| {
            b.chosen
                .cmp(&a.chosen)
                .then(sb.cmp(sa))
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        ranked.truncate(limit);

        ContextSearchResult {
            items: ranked.into_iter().map(|(item, _)| item).collect(),
            // In the order the user chose them.
            unmapped: chosen
                .iter()
                .filter(|id| unmapped.contains(id))
                .filter_map(|id| catalog.project(id))
                .map(|p| UnmappedProject {
                    project_id: p.id.clone(),
                    name: p.name.clone(),
                })
                .collect(),
        }
    }

    /// Record that `item` was added to a launch.
    pub fn record_hit(&self, catalog: &Catalog, item: &ContextRef) {
        let Some(item) = catalog.resolve(std::slice::from_ref(item)).pop() else {
            return;
        };
        if let Ok(guard) = self.frecency.read()
            && let Some(tracker) = guard.as_ref()
            && let Err(e) = tracker.track_access(&frecency_key(&item))
        {
            log::warn!("[context] could not record a hit: {e}");
        }
    }

    /// Each candidate's score, `None` when it does not match. An empty query
    /// matches everything, ordered by frecency alone.
    fn scores(&self, query: &str, items: &[ContextItem]) -> Vec<Option<i64>> {
        let frecency: Vec<i64> = match self.frecency.read() {
            Ok(guard) => match guard.as_ref() {
                Some(tracker) => items
                    .iter()
                    .map(|item| tracker.get_access_score(&frecency_key(item), FFFMode::Ai))
                    .collect(),
                None => vec![0; items.len()],
            },
            Err(_) => vec![0; items.len()],
        };
        if query.is_empty() {
            return frecency.into_iter().map(Some).collect();
        }

        let config = neo_frizbee::Config {
            // A typo per four characters typed, at most two: more and a short
            // query matches everything.
            max_typos: Some((query.chars().count() / 4).min(2) as u16),
            sort: false,
            ..Default::default()
        };
        let mut best: Vec<Option<i64>> = vec![None; items.len()];
        let mut consider = |haystacks: Vec<&str>, weight: i64| {
            for m in neo_frizbee::match_list(query, &haystacks, &config) {
                let slot = &mut best[m.index as usize];
                let score = i64::from(m.score) * weight / 4;
                *slot = Some(slot.map_or(score, |s| s.max(score)));
            }
        };
        // The title is what a person searches by; an id or path and the
        // owner's name still find it, weighted below.
        consider(items.iter().map(|i| i.title.as_str()).collect(), 4);
        consider(
            items
                .iter()
                .map(|i| i.map_id.as_deref().unwrap_or(&i.reference.locator))
                .collect(),
            3,
        );
        let with_owner: Vec<String> = items
            .iter()
            .map(|i| format!("{} {}", i.owner_name, i.title))
            .collect();
        consider(with_owner.iter().map(String::as_str).collect(), 3);

        best.into_iter()
            .zip(frecency)
            .map(|(score, f)| score.map(|s| s + f))
            .collect()
    }

    /// `root`'s items, read again when it changed, went stale or was never
    /// read.
    fn items_of(&self, root: &CatalogRoot) -> (Vec<ContextItem>, Option<MapStatus>) {
        let dir = root.path.clone();
        let key = (root.kind, dir.clone(), owner_key(root));
        let mut state = self.state.lock();
        let changes = dir
            .as_deref()
            .map(|d| self.watch(&mut state, watch_dir(root.kind, d)))
            .unwrap_or(0);
        let now = Instant::now();
        let fresh = state.cached.get(&key).is_some_and(|c| {
            c.changes_seen == changes && now.duration_since(c.read_at) < self.stale
        });
        if !fresh {
            let (items, map_status) = Catalog::items(root);
            state.cached.insert(
                key.clone(),
                Cached {
                    items,
                    map_status,
                    read_at: now,
                    changes_seen: changes,
                    used_at: now,
                },
            );
        }
        let cached = state.cached.get_mut(&key).expect("just inserted");
        cached.used_at = now;
        (cached.items.clone(), cached.map_status)
    }

    /// Watch `dir`, returning its change count so far.
    ///
    /// The picker scans in the background; its subscription can only be made
    /// once the watcher is up, so it is retried on every search until then.
    /// Making it counts as a change: anything written before it was missed.
    fn watch(&self, state: &mut State, dir: PathBuf) -> u64 {
        if !state.watches.contains_key(&dir) {
            let Some(picker) = self.picker(&dir) else {
                return 0;
            };
            state.watches.insert(
                dir.clone(),
                Watch {
                    picker,
                    changes: Arc::new(AtomicU64::new(0)),
                    subscription: None,
                },
            );
        }
        let watch = state.watches.get_mut(&dir).expect("just inserted");
        if watch.subscription.is_none() {
            let counter = watch.changes.clone();
            if let Ok(id) = watch
                .picker
                .watch("", WatchOptions::default(), move |_, events| {
                    if !events.is_empty() {
                        counter.fetch_add(1, Ordering::Relaxed);
                    }
                })
            {
                watch.subscription = Some(id);
                watch.changes.fetch_add(1, Ordering::Relaxed);
            }
        }
        watch.changes.load(Ordering::Relaxed)
    }

    fn picker(&self, dir: &Path) -> Option<SharedFilePicker> {
        if !dir.is_dir() {
            return None;
        }
        let picker = SharedFilePicker::default();
        let options = FilePickerOptions {
            base_path: dir.to_string_lossy().into_owned(),
            mode: FFFMode::Ai,
            watch: true,
            enable_mmap_cache: false,
            enable_content_indexing: false,
            ..Default::default()
        };
        match FilePicker::new_with_shared_state(picker.clone(), self.frecency.clone(), options) {
            Ok(()) => Some(picker),
            // fff refuses `/` and `$HOME`; such a root is read on staleness.
            Err(e) => {
                log::debug!("[context] no watcher for {}: {e}", dir.display());
                None
            }
        }
    }

    /// Drop roots nobody searched for a while, and watchers nothing uses.
    fn sweep(&self) {
        let mut state = self.state.lock();
        let now = Instant::now();
        let idle = self.idle;
        state
            .cached
            .retain(|_, cached| now.duration_since(cached.used_at) < idle);
        let live: Vec<PathBuf> = state
            .cached
            .keys()
            .filter_map(|(kind, dir, _)| dir.as_deref().map(|d| watch_dir(*kind, d)))
            .collect();
        state.watches.retain(|dir, watch| {
            let keep = live.contains(dir);
            if !keep && let Some(id) = watch.subscription.take() {
                watch.picker.unwatch(id);
            }
            keep
        });
    }

    #[cfg(test)]
    fn watchers(&self) -> usize {
        self.state.lock().watches.len()
    }
}

/// The directory whose changes matter to a root: an OpenSpec root's
/// `openspec/`, not the whole repository around it.
fn watch_dir(kind: RootKind, dir: &Path) -> PathBuf {
    match kind {
        RootKind::Spec => dir.join(okena_openspec::root::OPENSPEC_DIR),
        RootKind::Map | RootKind::Knowledge => dir.to_path_buf(),
    }
}

fn owner_key(root: &CatalogRoot) -> String {
    format!("{:?}", root.owner.owner)
}

/// What frecency is recorded under. A map entry's file is shared by every
/// entry without a doc, so its id is part of the key.
fn frecency_key(item: &ContextItem) -> PathBuf {
    match (&item.reference.kind, &item.map_id) {
        (ContextKind::MapEntry, Some(id)) => PathBuf::from(format!("{}#{id}", item.path)),
        _ => PathBuf::from(&item.path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::tests::world;
    use crate::items::fixtures::{MAP, write};
    use okena_core::context::ContextOwner;

    fn titles(result: &ContextSearchResult) -> Vec<(String, String, bool)> {
        result
            .items
            .iter()
            .map(|i| (i.title.clone(), i.owner_name.clone(), i.chosen))
            .collect()
    }

    #[test]
    fn a_chosen_projects_item_ranks_above_an_equal_match_elsewhere() {
        let w = world();
        let index = ContextIndex::open(None);
        // Both projects map an identical `Checkout` area.
        let got = index.search(&w.catalog, "checkout", &["p-billing".into()], false, 50);
        let checkouts: Vec<_> = titles(&got)
            .into_iter()
            .filter(|(t, _, _)| t == "Checkout")
            .collect();
        assert_eq!(
            checkouts,
            [
                ("Checkout".into(), "billing".into(), true),
                ("Checkout".into(), "shop".into(), false),
            ]
        );
        // And the other way round.
        let got = index.search(&w.catalog, "checkout", &["p-shop".into()], false, 50);
        assert_eq!(got.items[0].owner_name, "shop");
        assert!(got.items[0].chosen);
    }

    #[test]
    fn a_followed_stores_items_rank_with_the_project_that_follows_it() {
        let w = world();
        let index = ContextIndex::open(None);
        let got = index.search(&w.catalog, "principles", &["p-shop".into()], false, 50);
        assert_eq!(got.items[0].title, "Engineering principles");
        assert!(got.items[0].chosen);
        let got = index.search(&w.catalog, "principles", &["p-billing".into()], false, 50);
        // Still findable, just not first-band.
        assert_eq!(got.items[0].title, "Engineering principles");
        assert!(!got.items[0].chosen);
    }

    #[test]
    fn a_scoped_search_sees_only_the_chosen_roots() {
        let w = world();
        let index = ContextIndex::open(None);
        let got = index.search(&w.catalog, "", &["p-billing".into()], true, 50);
        assert!(!got.items.is_empty());
        assert!(got.items.iter().all(|i| i.chosen));
        assert!(!got.items.iter().any(|i| i.owner_name == "acme"));
    }

    #[test]
    fn a_chosen_unscanned_project_is_reported_and_others_are_not() {
        let w = world();
        let index = ContextIndex::open(None);
        let got = index.search(
            &w.catalog,
            "",
            &["p-bare".into(), "p-shop".into()],
            false,
            50,
        );
        assert_eq!(
            got.unmapped,
            [UnmappedProject {
                project_id: "p-bare".into(),
                name: "bare".into()
            }]
        );
        let got = index.search(&w.catalog, "", &["p-shop".into()], false, 50);
        assert!(got.unmapped.is_empty());
    }

    #[test]
    fn typing_narrows_and_matches_ids_and_owners_too() {
        let w = world();
        let index = ContextIndex::open(None);
        let got = index.search(&w.catalog, "orders-db", &[], false, 50);
        assert!(!got.items.is_empty());
        assert!(got.items.iter().all(|i| i.title == "orders-db"));
        let got = index.search(&w.catalog, "zzzz-nothing", &[], false, 50);
        assert!(got.items.is_empty());
        assert!(index.search(&w.catalog, "", &[], false, 3).items.len() == 3);
    }

    #[test]
    fn a_hit_raises_an_item_within_its_band() {
        let w = world();
        let db = tempfile::tempdir().unwrap();
        let index = ContextIndex::open(Some(db.path()));
        let catalog_item = |title: &str| {
            index
                .search(&w.catalog, "", &[], false, 500)
                .items
                .into_iter()
                .find(|i| i.title == title && i.owner_name == "shop")
                .unwrap()
        };
        let basket = catalog_item("Basket");
        let before = index.search(&w.catalog, "", &[], false, 500);
        let first_before = before.items[0].reference.clone();
        for _ in 0..3 {
            index.record_hit(&w.catalog, &basket.reference);
        }
        let after = index.search(&w.catalog, "", &[], false, 500);
        assert_eq!(after.items[0].reference, basket.reference);
        assert_ne!(first_before, basket.reference);
    }

    #[test]
    fn editing_a_map_shows_up_without_rebuilding_the_index() {
        let w = world();
        let index = ContextIndex {
            // Only the watcher may notice.
            stale: Duration::from_secs(3600),
            ..ContextIndex::open(None)
        };
        let shop = [String::from("p-shop")];
        assert!(
            index
                .search(&w.catalog, "ledger", &shop, true, 50)
                .items
                .is_empty()
        );
        // Wait for the watcher, then search once so the subscription is made.
        let knowledge = w
            .dir
            .path()
            .canonicalize()
            .unwrap()
            .join("shop/.okena/knowledge");
        {
            let state = index.state.lock();
            state.watches[&knowledge]
                .picker
                .wait_for_watcher(Duration::from_secs(10));
        }
        index.search(&w.catalog, "ledger", &shop, true, 50);

        let edited = MAP.replace(
            "areas:\n",
            "areas:\n  - id: ledger\n    name: Ledger\n    description: Books.\n    paths: [src/ledger]\n",
        );
        write(&knowledge, "project-map.yaml", &edited);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let got = index.search(&w.catalog, "ledger", &shop, true, 50);
            if let Some(item) = got.items.first() {
                assert_eq!(item.map_id.as_deref(), Some("area:ledger"));
                assert_eq!(item.reference.owner, ContextOwner::project("p-shop"));
                break;
            }
            assert!(Instant::now() < deadline, "the edit never showed up");
            std::thread::sleep(Duration::from_millis(100));
        }

        // A new knowledge doc in a followed store shows up the same way.
        let store = w.dir.path().canonicalize().unwrap().join("acme");
        index.search(&w.catalog, "", &shop, true, 50);
        {
            let state = index.state.lock();
            state.watches[&store]
                .picker
                .wait_for_watcher(Duration::from_secs(10));
        }
        index.search(&w.catalog, "", &shop, true, 50);
        write(&store, "docs/ci.md", "---\ntitle: CI notes\n---\n");
        let deadline = Instant::now() + Duration::from_secs(10);
        while index
            .search(&w.catalog, "CI notes", &shop, true, 50)
            .items
            .first()
            .is_none_or(|i| i.title != "CI notes")
        {
            assert!(Instant::now() < deadline, "the new doc never showed up");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    fn idle_roots_let_go_of_their_watchers() {
        let w = world();
        let index = ContextIndex {
            idle: Duration::from_millis(0),
            ..ContextIndex::open(None)
        };
        index.search(&w.catalog, "", &[], false, 50);
        assert_eq!(index.watchers(), 0);
        let index = ContextIndex::open(None);
        index.search(&w.catalog, "", &[], false, 50);
        assert!(index.watchers() > 0);
    }
}
