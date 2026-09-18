//! The extension host the daemon runs: every installed extension, a worker
//! thread for each enabled one, and install / update / remove.
//!
//! Each worker owns its extension's WASM instance and runs its calls one at
//! a time, so a slow or stuck extension only ever holds up itself. Every
//! method here blocks; the daemon calls them off its reactor.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use okena_core::extension::{
    ApiExtension, ExtActionDef, ExtActionOutcome, ExtInstallPreview, ExtPermissions,
    ExtQueryDef, ExtRefusal, ExtRunState, ExtSource, ExtStatus, ExtToolStatus, ExtUpdate,
    ExtView, Invoker,
};
use parking_lot::Mutex;

use crate::deps;
use crate::exec::SearchPath;
use crate::install::{self, Prepared};
use crate::manifest::Manifest;
use crate::permissions::Guard;
use crate::runtime::{CallError, Environment, Instance, ProjectsFn, Runtime};
use crate::store::{self, Dirs, InstalledRecord, now_ms};

/// How many refusals an extension keeps on show.
const MAX_REFUSALS: usize = 10;
/// How long a caller waits for an action or query before giving up on it.
const CALL_TIMEOUT: Duration = Duration::from_secs(6 * 60);

/// What the host needs from the daemon.
pub struct HostConfig {
    pub dirs: Dirs,
    pub search_path: SearchPath,
    pub projects: ProjectsFn,
    /// Called whenever what [`ExtensionHost::snapshot`] returns changed.
    pub on_change: Arc<dyn Fn() + Send + Sync>,
    /// Ids taken by extensions compiled into okena.
    pub reserved_ids: Vec<String>,
}

/// The settings that drive the host: which extensions are on, and each
/// one's saved configuration.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostSettings {
    pub enabled: HashSet<String>,
    pub configs: HashMap<String, serde_json::Value>,
}

pub struct ExtensionHost {
    config: Arc<HostConfig>,
    runtime: Runtime,
    entries: Arc<Mutex<BTreeMap<String, Entry>>>,
    workers: Mutex<HashMap<String, Worker>>,
    settings: Mutex<HostSettings>,
    /// Serialises install, update, reload and remove, which share the git
    /// cache and the registry.
    lifecycle: Mutex<()>,
}

/// One installed extension: its record, manifest, and what it last showed.
struct Entry {
    record: InstalledRecord,
    manifest: Manifest,
    enabled: bool,
    run: RunInfo,
    update: Option<ExtUpdate>,
}

#[derive(Default)]
struct RunInfo {
    state: ExtRunState,
    tools: Vec<ExtToolStatus>,
    view: Option<ExtView>,
    status: Option<ExtStatus>,
    actions: Vec<ExtActionDef>,
    queries: Vec<ExtQueryDef>,
    refreshing: bool,
    refreshed_at_ms: Option<u64>,
    refresh_error: Option<String>,
    refusals: Vec<ExtRefusal>,
}

struct Worker {
    jobs: Sender<Job>,
    thread: Option<std::thread::JoinHandle<()>>,
}

enum Job {
    Refresh,
    Recheck,
    SetConfig(serde_json::Value),
    Action {
        action: String,
        items: Vec<String>,
        inputs: Vec<(String, String)>,
        invoker: Invoker,
        reply: Sender<Result<ExtActionOutcome, String>>,
    },
    Query {
        id: String,
        args: String,
        reply: Sender<Result<String, String>>,
    },
    Stop,
}

impl Drop for ExtensionHost {
    fn drop(&mut self) {
        let workers: Vec<Worker> = self.workers.lock().drain().map(|(_, w)| w).collect();
        for worker in workers {
            stop_worker(worker);
        }
    }
}

fn stop_worker(mut worker: Worker) {
    let _ = worker.jobs.send(Job::Stop);
    if let Some(thread) = worker.thread.take() {
        // The worker finishes its current call first; don't block on a
        // stuck one — its compute budget ends it soon enough.
        std::thread::spawn(move || {
            let _ = thread.join();
        });
    }
}

impl ExtensionHost {
    /// Loads the registry. Nothing runs until [`apply_settings`](Self::apply_settings).
    pub fn new(config: HostConfig) -> Result<Self, String> {
        let runtime = Runtime::new()?;
        let mut entries = BTreeMap::new();
        for (id, record) in store::load_registry(&config.dirs) {
            let manifest_path = config.dirs.installed(&id);
            match Manifest::load(&manifest_path) {
                Ok(manifest) => {
                    entries.insert(
                        id,
                        Entry {
                            record,
                            manifest,
                            enabled: false,
                            run: RunInfo::default(),
                            update: None,
                        },
                    );
                }
                Err(e) => log::error!("extension {id} is installed but unreadable: {e}"),
            }
        }
        Ok(Self {
            config: Arc::new(config),
            runtime,
            entries: Arc::new(Mutex::new(entries)),
            workers: Mutex::new(HashMap::new()),
            settings: Mutex::new(HostSettings::default()),
            lifecycle: Mutex::new(()),
        })
    }

    pub fn installed_ids(&self) -> Vec<String> {
        self.entries.lock().keys().cloned().collect()
    }

    pub fn is_installed(&self, id: &str) -> bool {
        self.entries.lock().contains_key(id)
    }

    /// Every installed extension, as clients see it.
    pub fn snapshot(&self) -> Vec<ApiExtension> {
        let settings = self.settings.lock().clone();
        self.entries
            .lock()
            .values()
            .map(|entry| to_api(entry, &settings))
            .collect()
    }

    pub fn extension(&self, id: &str) -> Option<ApiExtension> {
        let settings = self.settings.lock().clone();
        self.entries.lock().get(id).map(|e| to_api(e, &settings))
    }

    /// Starts, stops and reconfigures workers to match `settings`.
    pub fn apply_settings(&self, settings: HostSettings) {
        let previous = std::mem::replace(&mut *self.settings.lock(), settings.clone());
        let ids = self.installed_ids();
        for id in ids {
            let enabled = settings.enabled.contains(&id);
            let config = self.effective_config(&id, &settings);
            let running = self.workers.lock().contains_key(&id);
            match (enabled, running) {
                (true, false) => self.start_worker(&id),
                (false, true) => self.stop(&id),
                (true, true) => {
                    let before = self.effective_config(&id, &previous);
                    if before != config
                        && let Some(worker) = self.workers.lock().get(&id)
                    {
                        let _ = worker.jobs.send(Job::SetConfig(config));
                    }
                }
                (false, false) => {}
            }
        }
        (self.config.on_change)();
    }

    fn effective_config(&self, id: &str, settings: &HostSettings) -> serde_json::Value {
        self.entries
            .lock()
            .get(id)
            .map(|e| e.manifest.effective_config(settings.configs.get(id)))
            .unwrap_or_default()
    }

    fn start_worker(&self, id: &str) {
        let Some((manifest, record)) = self
            .entries
            .lock()
            .get_mut(id)
            .map(|e| {
                e.enabled = true;
                e.run = RunInfo {
                    state: ExtRunState::Starting,
                    ..RunInfo::default()
                };
                (e.manifest.clone(), e.record.clone())
            })
        else {
            return;
        };
        let config = self.effective_config(id, &self.settings.lock().clone());
        let (jobs, rx) = mpsc::channel();
        let context = WorkerContext {
            id: id.to_string(),
            manifest,
            approved: record.approved,
            config,
            host: self.config.clone(),
            runtime: self.runtime.clone(),
            entries: self.entries.clone(),
        };
        let thread = std::thread::Builder::new()
            .name(format!("okena-ext-{id}"))
            .spawn(move || context.run(rx));
        match thread {
            Ok(thread) => {
                self.workers.lock().insert(
                    id.to_string(),
                    Worker {
                        jobs,
                        thread: Some(thread),
                    },
                );
            }
            Err(e) => self.set_state(id, ExtRunState::Failed { message: format!("cannot start: {e}") }),
        }
    }

    fn stop(&self, id: &str) {
        if let Some(worker) = self.workers.lock().remove(id) {
            stop_worker(worker);
        }
        if let Some(entry) = self.entries.lock().get_mut(id) {
            entry.enabled = false;
            entry.run = RunInfo::default();
        }
    }

    fn set_state(&self, id: &str, state: ExtRunState) {
        if let Some(entry) = self.entries.lock().get_mut(id) {
            entry.run.state = state;
        }
        (self.config.on_change)();
    }

    fn send(&self, id: &str, job: Job) -> Result<(), String> {
        let workers = self.workers.lock();
        let worker = workers
            .get(id)
            .ok_or_else(|| format!("extension `{id}` is not enabled"))?;
        worker
            .jobs
            .send(job)
            .map_err(|_| format!("extension `{id}` has stopped"))
    }

    /// Refreshes now, or re-runs the start-up checks if it is not ready.
    pub fn refresh(&self, id: &str) -> Result<(), String> {
        self.send(id, Job::Refresh)
    }

    /// Runs the dependency check again, and starts the extension if it passes.
    pub fn recheck(&self, id: &str) -> Result<(), String> {
        self.send(id, Job::Recheck)
    }

    pub fn run_action(
        &self,
        id: &str,
        action: &str,
        items: Vec<String>,
        inputs: Vec<(String, String)>,
        invoker: Invoker,
    ) -> Result<ExtActionOutcome, String> {
        let (reply, answer) = mpsc::channel();
        self.send(
            id,
            Job::Action {
                action: action.to_string(),
                items,
                inputs,
                invoker,
                reply,
            },
        )?;
        answer
            .recv_timeout(CALL_TIMEOUT)
            .map_err(|_| format!("`{action}` did not finish in time"))?
    }

    pub fn query(&self, id: &str, query: &str, args: &str) -> Result<String, String> {
        let (reply, answer) = mpsc::channel();
        self.send(
            id,
            Job::Query {
                id: query.to_string(),
                args: args.to_string(),
                reply,
            },
        )?;
        answer
            .recv_timeout(CALL_TIMEOUT)
            .map_err(|_| format!("query `{query}` did not finish in time"))?
    }

    // ─── Install, update, remove ────────────────────────────────────────────

    /// Fetches `source` and returns what the user must approve.
    pub fn preview_install(&self, source: &ExtSource) -> Result<ExtInstallPreview, String> {
        let _guard = self.lifecycle.lock();
        let prepared = install::prepare(&self.config.dirs, source, &self.config.search_path)?;
        self.check_id(&prepared.manifest.id)?;
        let mut preview = prepared.preview(&self.config.search_path);
        preview.installed = self.is_installed(&preview.id);
        Ok(preview)
    }

    /// Installs `source` at `commit` (from the preview) with the permissions
    /// the user approved, which must be exactly what the manifest asks for.
    /// A `Local` source has no commit.
    pub fn install(
        &self,
        source: &ExtSource,
        commit: Option<&str>,
        approved: &ExtPermissions,
    ) -> Result<InstalledRecord, String> {
        let _guard = self.lifecycle.lock();
        let mut prepared = install::prepare(&self.config.dirs, &pin(source, commit), &self.config.search_path)?;
        self.check_id(&prepared.manifest.id)?;
        keep_ref(source, &mut prepared.source);
        if let (Some(commit), ExtSource::Git { commit: got, .. }) = (commit, &prepared.source)
            && got != commit
        {
            return Err(format!(
                "the source moved from {} to {} since you reviewed it; review it again",
                short(commit),
                short(got)
            ));
        }
        if !covers(approved, &prepared.manifest.permissions) {
            return Err("the permissions approved are not the ones this extension asks for; review it again".into());
        }
        self.place(&prepared, prepared.manifest.permissions.clone())
    }

    /// Builds (or copies) the component, checks it loads, and swaps it in.
    fn place(&self, prepared: &Prepared, approved: ExtPermissions) -> Result<InstalledRecord, String> {
        let id = prepared.manifest.id.clone();
        let wasm = install::component_bytes(prepared, &self.config.dirs, &self.config.search_path)?;
        // Refuse a component that does not compile before touching the
        // installed copy.
        self.runtime.compile(&wasm)?;

        let was_running = self.workers.lock().contains_key(&id);
        self.stop(&id);
        let record = install::place(&self.config.dirs, prepared, &wasm, approved)?;
        {
            let mut entries = self.entries.lock();
            entries.insert(
                id.clone(),
                Entry {
                    record: record.clone(),
                    manifest: prepared.manifest.clone(),
                    enabled: false,
                    run: RunInfo::default(),
                    update: None,
                },
            );
            let records: BTreeMap<_, _> = entries
                .iter()
                .map(|(id, e)| (id.clone(), e.record.clone()))
                .collect();
            store::save_registry(&self.config.dirs, &records)?;
        }
        if was_running || self.settings.lock().enabled.contains(&id) {
            self.start_worker(&id);
        }
        (self.config.on_change)();
        Ok(record)
    }

    fn check_id(&self, id: &str) -> Result<(), String> {
        if self.config.reserved_ids.iter().any(|r| r == id) {
            Err(format!("the id `{id}` belongs to an extension built into okena"))
        } else {
            Ok(())
        }
    }

    /// Asks each git-installed extension's remote whether its ref moved.
    /// Returns the ids with an update available.
    pub fn check_updates(&self) -> Vec<String> {
        let sources: Vec<(String, ExtSource)> = self
            .entries
            .lock()
            .iter()
            .map(|(id, e)| (id.clone(), e.record.source.clone()))
            .collect();
        let mut available = Vec::new();
        for (id, source) in sources {
            let ExtSource::Git { url, git_ref, commit, .. } = &source else {
                continue;
            };
            let latest = match install::remote_commit(url, git_ref.as_deref(), &self.config.search_path) {
                Ok(Some(latest)) => latest,
                Ok(None) => continue,
                Err(e) => {
                    log::warn!("checking extension {id} for updates: {e}");
                    continue;
                }
            };
            if &latest == commit {
                if let Some(entry) = self.entries.lock().get_mut(&id) {
                    entry.update = None;
                }
                continue;
            }
            let update = match self.preview_update(&id) {
                Ok(preview) => ExtUpdate {
                    commit: match &preview.source {
                        ExtSource::Git { commit, .. } => commit.clone(),
                        ExtSource::Local { .. } => latest.clone(),
                    },
                    version: preview.version,
                    added_permissions: preview.added_permissions.unwrap_or_default(),
                },
                Err(e) => {
                    log::warn!("reading the update of extension {id}: {e}");
                    ExtUpdate {
                        commit: latest,
                        version: String::new(),
                        added_permissions: ExtPermissions::default(),
                    }
                }
            };
            if let Some(entry) = self.entries.lock().get_mut(&id) {
                entry.update = Some(update);
            }
            available.push(id);
        }
        (self.config.on_change)();
        available
    }

    /// Fetches the newest commit of an installed extension's ref (or reads
    /// its local folder) and returns what updating would change.
    pub fn preview_update(&self, id: &str) -> Result<ExtInstallPreview, String> {
        let _guard = self.lifecycle.lock();
        let (source, approved) = self.source_of(id)?;
        let prepared = install::prepare(&self.config.dirs, &unpin(&source), &self.config.search_path)?;
        if prepared.manifest.id != id {
            return Err(format!(
                "the source now holds extension `{}`, not `{id}`",
                prepared.manifest.id
            ));
        }
        let mut preview = prepared.preview(&self.config.search_path);
        preview.installed = true;
        preview.added_permissions = Some(prepared.manifest.permissions.added_since(&approved));
        Ok(preview)
    }

    /// Updates to `commit` (from [`preview_update`](Self::preview_update)).
    /// When the new version asks for more than was approved, `approved` must
    /// cover it.
    pub fn update(
        &self,
        id: &str,
        commit: Option<&str>,
        approved: Option<&ExtPermissions>,
    ) -> Result<InstalledRecord, String> {
        let _guard = self.lifecycle.lock();
        let (source, previously) = self.source_of(id)?;
        let target = match (&source, commit) {
            (ExtSource::Git { .. }, Some(commit)) => pin(&unpin(&source), Some(commit)),
            _ => unpin(&source),
        };
        let mut prepared = install::prepare(&self.config.dirs, &target, &self.config.search_path)?;
        if prepared.manifest.id != id {
            return Err(format!("the source now holds `{}`, not `{id}`", prepared.manifest.id));
        }
        let added = prepared.manifest.permissions.added_since(&previously);
        if !added.is_empty() && !approved.is_some_and(|a| covers(a, &prepared.manifest.permissions)) {
            return Err(format!(
                "this update asks for more permissions ({}); approve them first",
                describe(&added)
            ));
        }
        keep_ref(&source, &mut prepared.source);
        self.place(&prepared, prepared.manifest.permissions.clone())
    }

    /// Rebuilds a locally installed extension from its folder and reloads
    /// it, keeping its data. Fails, asking for approval, if it now asks for
    /// more permissions.
    pub fn reload(&self, id: &str, approved: Option<&ExtPermissions>) -> Result<InstalledRecord, String> {
        match self.source_of(id)?.0 {
            ExtSource::Local { .. } => self.update(id, None, approved),
            ExtSource::Git { .. } => Err(format!("`{id}` was installed from git; update it instead")),
        }
    }

    /// Deletes the extension, its files and its data.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        let _guard = self.lifecycle.lock();
        if !self.is_installed(id) {
            return Err(format!("extension `{id}` is not installed"));
        }
        self.stop(id);
        let records = {
            let mut entries = self.entries.lock();
            entries.remove(id);
            entries
                .iter()
                .map(|(id, e)| (id.clone(), e.record.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        store::save_registry(&self.config.dirs, &records)?;
        for dir in [self.config.dirs.installed(id), self.config.dirs.data(id)] {
            if let Err(e) = std::fs::remove_dir_all(&dir)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                log::warn!("removing {}: {e}", dir.display());
            }
        }
        (self.config.on_change)();
        Ok(())
    }

    fn source_of(&self, id: &str) -> Result<(ExtSource, ExtPermissions), String> {
        self.entries
            .lock()
            .get(id)
            .map(|e| (e.record.source.clone(), e.record.approved.clone()))
            .ok_or_else(|| format!("extension `{id}` is not installed"))
    }
}

/// The source asking for `commit` instead of its ref.
fn pin(source: &ExtSource, commit: Option<&str>) -> ExtSource {
    match (source, commit) {
        (ExtSource::Git { url, path, .. }, Some(commit)) => ExtSource::Git {
            url: url.clone(),
            git_ref: Some(commit.to_string()),
            path: path.clone(),
            commit: String::new(),
        },
        _ => source.clone(),
    }
}

/// Records the ref `requested` asked for, not the commit it was pinned to
/// for fetching, so the ref's later moves show as updates.
fn keep_ref(requested: &ExtSource, prepared: &mut ExtSource) {
    if let (ExtSource::Git { git_ref, .. }, ExtSource::Git { git_ref: recorded, .. }) =
        (requested, prepared)
    {
        recorded.clone_from(git_ref);
    }
}

/// The source asking for its ref again, not the commit it was installed at.
fn unpin(source: &ExtSource) -> ExtSource {
    match source {
        ExtSource::Git { url, git_ref, path, .. } => ExtSource::Git {
            url: url.clone(),
            git_ref: git_ref.clone(),
            path: path.clone(),
            commit: String::new(),
        },
        local => local.clone(),
    }
}

/// `approved` grants everything `wanted` asks for.
fn covers(approved: &ExtPermissions, wanted: &ExtPermissions) -> bool {
    wanted.added_since(approved).is_empty()
}

fn describe(p: &ExtPermissions) -> String {
    let mut parts = Vec::new();
    if !p.commands.is_empty() {
        parts.push(format!("commands: {}", p.commands.join(", ")));
    }
    if !p.paths.is_empty() {
        parts.push(format!("paths: {}", p.paths.join(", ")));
    }
    if p.start_agents {
        parts.push("starting agents".into());
    }
    parts.join("; ")
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(10)]
}

fn to_api(entry: &Entry, settings: &HostSettings) -> ApiExtension {
    let m = &entry.manifest;
    let run = &entry.run;
    ApiExtension {
        id: m.id.clone(),
        name: m.name.clone(),
        version: m.version.clone(),
        description: m.description.clone(),
        enabled: settings.enabled.contains(&m.id),
        source: entry.record.source.clone(),
        update: entry.update.clone(),
        permissions: entry.record.approved.clone(),
        requires: m.requires.clone(),
        tools: run.tools.clone(),
        config_schema: m.config.clone(),
        view_title: m.view.as_ref().map(|v| v.title.clone()),
        refresh_interval_secs: m.refresh_interval().map_or(0, |d| d.as_secs()),
        state: if entry.enabled {
            run.state.clone()
        } else {
            ExtRunState::Disabled
        },
        view: run.view.clone(),
        status: run.status.clone(),
        actions: run.actions.clone(),
        queries: run.queries.clone(),
        refreshing: run.refreshing,
        refreshed_at_ms: run.refreshed_at_ms,
        refresh_error: run.refresh_error.clone(),
        refusals: run.refusals.clone(),
        pending_confirmations: Vec::new(),
    }
}

// ─── The worker ─────────────────────────────────────────────────────────────

struct WorkerContext {
    id: String,
    manifest: Manifest,
    approved: ExtPermissions,
    config: serde_json::Value,
    host: Arc<HostConfig>,
    runtime: Runtime,
    entries: Arc<Mutex<BTreeMap<String, Entry>>>,
}

impl WorkerContext {
    fn update(&self, f: impl FnOnce(&mut RunInfo)) {
        if let Some(entry) = self.entries.lock().get_mut(&self.id) {
            f(&mut entry.run);
        }
        (self.host.on_change)();
    }

    fn run(mut self, jobs: Receiver<Job>) {
        let mut instance: Option<Instance> = None;
        let mut ready = self.start(&mut instance);
        loop {
            let interval = self.manifest.refresh_interval().filter(|_| ready);
            let job = match interval {
                Some(interval) => match jobs.recv_timeout(interval) {
                    Ok(job) => job,
                    Err(RecvTimeoutError::Timeout) => Job::Refresh,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match jobs.recv() {
                    Ok(job) => job,
                    Err(_) => return,
                },
            };
            match job {
                Job::Stop => return,
                Job::Refresh if ready => self.refresh(&mut instance),
                Job::Refresh | Job::Recheck => ready = self.start(&mut instance),
                Job::SetConfig(config) => {
                    self.config = config.clone();
                    if let Some(instance) = &mut instance {
                        instance.set_config(config);
                    }
                    ready = self.start(&mut instance);
                }
                Job::Action {
                    action,
                    items,
                    inputs,
                    invoker,
                    reply,
                } => {
                    let result = match (&mut instance, ready) {
                        (Some(instance), true) => instance
                            .run_action(&action, &items, &inputs, invoker)
                            .map_err(|e| e.message()),
                        _ => Err(self.not_ready()),
                    };
                    let refresh = result.as_ref().is_ok_and(|o| o.refresh);
                    let _ = reply.send(result);
                    if refresh {
                        self.refresh(&mut instance);
                    }
                }
                Job::Query { id, args, reply } => {
                    let result = match (&mut instance, ready) {
                        (Some(instance), true) => instance.query(&id, &args).map_err(|e| e.message()),
                        _ => Err(self.not_ready()),
                    };
                    let _ = reply.send(result);
                }
            }
        }
    }

    fn not_ready(&self) -> String {
        let state = self
            .entries
            .lock()
            .get(&self.id)
            .map(|e| e.run.state.clone())
            .unwrap_or_default();
        match state {
            ExtRunState::MissingTools => format!("{} is missing required tools", self.manifest.name),
            ExtRunState::NeedsConfig { missing } => {
                format!("{} needs configuring: {}", self.manifest.name, missing.join(", "))
            }
            ExtRunState::Failed { message } => format!("{} failed to start: {message}", self.manifest.name),
            _ => format!("{} is not ready", self.manifest.name),
        }
    }

    /// The dependency check, the configuration check, loading, and the first
    /// refresh. Returns whether the extension is ready.
    fn start(&self, instance: &mut Option<Instance>) -> bool {
        self.update(|run| run.state = ExtRunState::Starting);
        let tools = deps::check_tools(&self.manifest.requires, &self.host.search_path);
        let tools_ok = deps::all_ok(&tools);
        self.update(|run| run.tools = tools);
        if !tools_ok {
            self.update(|run| {
                run.state = ExtRunState::MissingTools;
                run.view = None;
                run.status = None;
            });
            return false;
        }
        let missing = self.manifest.missing_config(&self.config);
        if !missing.is_empty() {
            self.update(|run| {
                run.state = ExtRunState::NeedsConfig { missing };
                run.view = None;
                run.status = None;
            });
            return false;
        }
        if instance.is_none() {
            match self.load() {
                Ok(loaded) => *instance = Some(loaded),
                Err(message) => {
                    log::error!("extension {} failed to load: {message}", self.id);
                    self.update(|run| run.state = ExtRunState::Failed { message });
                    return false;
                }
            }
        }
        let Some(loaded) = instance.as_mut() else {
            return false;
        };
        match loaded.describe() {
            Ok((actions, queries)) => self.update(|run| {
                run.actions = actions;
                run.queries = queries;
                run.state = ExtRunState::Ready;
            }),
            Err(e) => {
                let message = e.message();
                self.update(|run| run.state = ExtRunState::Failed { message });
                return false;
            }
        }
        self.refresh(instance);
        true
    }

    fn load(&self) -> Result<Instance, String> {
        let wasm = std::fs::read(self.host.dirs.installed_wasm(&self.id))
            .map_err(|e| format!("reading the component: {e}"))?;
        let component = self.runtime.compile(&wasm)?;
        let entries = self.entries.clone();
        let on_change = self.host.on_change.clone();
        let id = self.id.clone();
        Instance::new(
            &self.runtime,
            component,
            Environment {
                extension_id: self.id.clone(),
                guard: Guard::new(self.approved.clone(), self.host.search_path.clone()),
                config: self.config.clone(),
                kv_path: self.host.dirs.kv(&self.id),
                projects: self.host.projects.clone(),
                refusals: Arc::new(move |message| {
                    if let Some(entry) = entries.lock().get_mut(&id) {
                        let refusals = &mut entry.run.refusals;
                        refusals.push(ExtRefusal {
                            at_ms: now_ms(),
                            message,
                        });
                        let excess = refusals.len().saturating_sub(MAX_REFUSALS);
                        refusals.drain(..excess);
                    }
                    on_change();
                }),
            },
        )
    }

    fn refresh(&self, instance: &mut Option<Instance>) {
        let Some(loaded) = instance.as_mut() else {
            return;
        };
        self.update(|run| run.refreshing = true);
        let result = loaded.refresh();
        self.update(|run| {
            run.refreshing = false;
            match result {
                Ok((view, status)) => {
                    run.view = Some(view);
                    run.status = status;
                    run.refresh_error = None;
                    run.refreshed_at_ms = Some(now_ms());
                }
                Err(e) => {
                    if let CallError::Trapped(_) = e {
                        log::warn!("extension {} crashed refreshing: {e}", self.id);
                    }
                    run.refresh_error = Some(e.message());
                }
            }
        });
    }
}
