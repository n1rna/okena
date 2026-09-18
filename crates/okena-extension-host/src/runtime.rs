//! Running one extension's component in wasmtime.
//!
//! An extension runs with no WASI access of its own: no files, no network,
//! no environment. Everything it does to the machine goes through the host
//! calls below, each checked against what the user approved. A trap — a
//! panic, running past its compute budget or its memory — fails that one
//! call; the next call starts a fresh instance.

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use okena_core::extension as api;
use parking_lot::Mutex;
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder, UpdateDeadline};
use wasmtime_wasi::p2::{OutputStream, Pollable, StreamError};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::bindings::Extension as Bindings;
use crate::bindings::okena::extension::{host, types, ui_v1};
use crate::convert;
use crate::exec;
use crate::kv::KvStore;
use crate::permissions::{Denied, Guard};

/// How long an extension's own code may compute in one call. Time spent in
/// host calls — commands, files — does not count.
pub const COMPUTE_BUDGET: Duration = Duration::from_secs(10);
/// How much memory one instance may grow to.
pub const MEMORY_LIMIT: usize = 256 * 1024 * 1024;
/// A command's timeout when the extension gives none, and the most it may ask for.
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_COMMAND_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_READ_BYTES: u64 = 32 * 1024 * 1024;
const EPOCH_TICK: Duration = Duration::from_millis(10);
/// How much of the guest's stderr is kept, for the message of a trap.
const STDERR_TAIL: usize = 16 * 1024;

/// The wasmtime engine, shared by every extension. A background thread
/// advances its epoch so long-running guest code can be interrupted.
#[derive(Clone)]
pub struct Runtime {
    engine: Engine,
    linker: Arc<Linker<Ctx>>,
}

impl Runtime {
    pub fn new() -> Result<Self, String> {
        let mut config = Config::new();
        config.epoch_interruption(true);
        let engine = Engine::new(&config).map_err(|e| format!("starting wasmtime: {e}"))?;

        let mut linker = Linker::<Ctx>::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|e| format!("linking WASI: {e}"))?;
        Bindings::add_to_linker::<Ctx, HasSelf<Ctx>>(&mut linker, |ctx| ctx)
            .map_err(|e| format!("linking the okena interface: {e}"))?;

        let ticker = engine.weak();
        std::thread::Builder::new()
            .name("okena-ext-epoch".into())
            .spawn(move || {
                while let Some(engine) = ticker.upgrade() {
                    engine.increment_epoch();
                    drop(engine);
                    std::thread::sleep(EPOCH_TICK);
                }
            })
            .map_err(|e| format!("starting the epoch thread: {e}"))?;

        Ok(Self {
            engine,
            linker: Arc::new(linker),
        })
    }

    /// Compiles a component. Slow for large ones; call it off the reactor.
    pub fn compile(&self, wasm: &[u8]) -> Result<Component, String> {
        Component::new(&self.engine, wasm).map_err(|e| format!("not a valid extension component: {e}"))
    }
}

/// A project, as `projects()` hands it to an extension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostProject {
    pub id: String,
    pub name: String,
    pub path: String,
}

/// The projects okena has, for `projects()`.
pub type ProjectsFn = Arc<dyn Fn() -> Vec<HostProject> + Send + Sync>;

/// Called with each refusal, so the host can show it on the extension.
pub type RefusalSink = Arc<dyn Fn(String) + Send + Sync>;

/// Everything an instance needs from okena.
pub struct Environment {
    pub extension_id: String,
    pub guard: Guard,
    pub config: serde_json::Value,
    pub kv_path: PathBuf,
    pub projects: ProjectsFn,
    pub refusals: RefusalSink,
}

/// Why a call to an extension failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// The extension returned an error.
    Failed(String),
    /// It crashed: a panic, its compute budget or its memory ran out. The
    /// next call starts a fresh instance.
    Trapped(String),
}

impl CallError {
    pub fn message(&self) -> String {
        match self {
            CallError::Failed(m) => m.clone(),
            CallError::Trapped(m) => format!("the extension crashed: {m}"),
        }
    }
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// One extension's running component.
pub struct Instance {
    runtime: Runtime,
    component: Component,
    shared: Shared,
    live: Option<(Store<Ctx>, Bindings)>,
}

/// What survives re-instantiation after a trap.
struct Shared {
    extension_id: String,
    guard: Guard,
    config: serde_json::Value,
    kv: Arc<Mutex<KvStore>>,
    projects: ProjectsFn,
    refusals: RefusalSink,
}

impl Instance {
    pub fn new(runtime: &Runtime, component: Component, env: Environment) -> Result<Self, String> {
        let mut instance = Self {
            runtime: runtime.clone(),
            component,
            shared: Shared {
                extension_id: env.extension_id,
                guard: env.guard,
                config: env.config,
                kv: Arc::new(Mutex::new(KvStore::open(env.kv_path))),
                projects: env.projects,
                refusals: env.refusals,
            },
            live: None,
        };
        instance.instantiate()?;
        Ok(instance)
    }

    /// Replaces the configuration the extension reads.
    pub fn set_config(&mut self, config: serde_json::Value) {
        if let Some((store, _)) = &mut self.live {
            store.data_mut().config = config.clone();
        }
        self.shared.config = config;
    }

    pub fn set_guard(&mut self, guard: Guard) {
        if let Some((store, _)) = &mut self.live {
            store.data_mut().guard = guard.clone();
        }
        self.shared.guard = guard;
    }

    fn instantiate(&mut self) -> Result<(), String> {
        let stderr = TailSink::default();
        let mut wasi = WasiCtx::builder();
        wasi.stderr(stderr.clone())
            .allow_tcp(false)
            .allow_udp(false)
            .allow_ip_name_lookup(false);
        let ctx = Ctx {
            wasi: wasi.build(),
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new()
                .memory_size(MEMORY_LIMIT)
                .instances(16)
                .tables(64)
                .memories(16)
                .build(),
            extension_id: self.shared.extension_id.clone(),
            guard: self.shared.guard.clone(),
            config: self.shared.config.clone(),
            kv: self.shared.kv.clone(),
            projects: self.shared.projects.clone(),
            refusals: self.shared.refusals.clone(),
            call_started: Instant::now(),
            host_time: Duration::ZERO,
            stderr,
        };
        let mut store = Store::new(&self.runtime.engine, ctx);
        store.limiter(|ctx| &mut ctx.limits);
        store.epoch_deadline_callback(|ctx| {
            let data = ctx.data();
            let computing = data.call_started.elapsed().saturating_sub(data.host_time);
            if computing > COMPUTE_BUDGET {
                Err(wasmtime::format_err!(
                    "it computed for more than {}s without returning",
                    COMPUTE_BUDGET.as_secs()
                ))
            } else {
                Ok(UpdateDeadline::Continue(1))
            }
        });
        store.set_epoch_deadline(1);
        begin_call(&mut store);
        let bindings = Bindings::instantiate(&mut store, &self.component, &self.runtime.linker)
            .map_err(|e| trap_message(&e, &store))?;
        self.live = Some((store, bindings));
        Ok(())
    }

    /// Runs `f` against a live instance, starting one if the last call trapped.
    fn call<R>(
        &mut self,
        f: impl FnOnce(&mut Store<Ctx>, &Bindings) -> wasmtime::Result<Result<R, String>>,
    ) -> Result<R, CallError> {
        if self.live.is_none() {
            self.instantiate().map_err(CallError::Trapped)?;
        }
        let Some((store, bindings)) = self.live.as_mut() else {
            return Err(CallError::Trapped("the extension could not be started".into()));
        };
        begin_call(store);
        match f(store, bindings) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(message)) => Err(CallError::Failed(message)),
            Err(trap) => {
                let message = trap_message(&trap, store);
                log::warn!("extension {} trapped: {message}", self.shared.extension_id);
                // A trapped instance refuses every later call; drop it.
                self.live = None;
                Err(CallError::Trapped(message))
            }
        }
    }

    pub fn describe(&mut self) -> Result<(Vec<api::ExtActionDef>, Vec<api::ExtQueryDef>), CallError> {
        self.call(|store, b| b.okena_extension_guest().call_describe(store).map(Ok))
            .map(convert::info)
    }

    pub fn refresh(&mut self) -> Result<(api::ExtView, Option<api::ExtStatus>), CallError> {
        self.call(|store, b| b.okena_extension_guest().call_refresh(store))
            .map(|out| (convert::view(out.view), out.status.map(convert::status)))
    }

    pub fn run_action(
        &mut self,
        action: &str,
        items: &[String],
        inputs: &[(String, String)],
        invoker: api::Invoker,
    ) -> Result<api::ExtActionOutcome, CallError> {
        let request = crate::bindings::exports::okena::extension::guest::ActionRequest {
            action: action.to_string(),
            items: items.to_vec(),
            inputs: inputs.to_vec(),
            invoker: convert::invoker(invoker),
        };
        self.call(|store, b| b.okena_extension_guest().call_run_action(store, &request))
            .map(convert::outcome)
    }

    /// Answers a query; `args` and the answer are JSON text.
    pub fn query(&mut self, id: &str, args: &str) -> Result<String, CallError> {
        self.call(|store, b| b.okena_extension_guest().call_query(store, id, args))
    }
}

fn begin_call(store: &mut Store<Ctx>) {
    let ctx = store.data_mut();
    ctx.call_started = Instant::now();
    ctx.host_time = Duration::ZERO;
    store.set_epoch_deadline(1);
}

/// A trap's message: the guest's panic message when it printed one, else
/// wasmtime's reason.
fn trap_message(error: &wasmtime::Error, store: &Store<Ctx>) -> String {
    let stderr = store.data().stderr.contents();
    let panic = stderr
        .lines()
        .skip_while(|line| !line.contains("panicked at"))
        .nth(1)
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string);
    if let Some(panic) = panic {
        return format!("it panicked: {panic}");
    }
    // A refused memory.grow makes Rust's allocator abort after saying so.
    if stderr.contains("memory allocation of") {
        return format!("it used more than {} MB of memory", MEMORY_LIMIT / (1024 * 1024));
    }
    if let Some(trap) = error.downcast_ref::<wasmtime::Trap>() {
        return match trap {
            wasmtime::Trap::Interrupt => format!(
                "it computed for more than {}s without returning",
                COMPUTE_BUDGET.as_secs()
            ),
            other => other.to_string(),
        };
    }
    // The epoch callback's error sits under wasmtime's backtrace context.
    let cause = error.root_cause().to_string();
    if cause.contains("growing memory") || cause.contains("memory") && cause.contains("limit") {
        return format!("it used more than {} MB of memory", MEMORY_LIMIT / (1024 * 1024));
    }
    cause
}

/// The store's data: WASI, limits, and what host calls check against.
pub(crate) struct Ctx {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    extension_id: String,
    guard: Guard,
    config: serde_json::Value,
    kv: Arc<Mutex<KvStore>>,
    projects: ProjectsFn,
    refusals: RefusalSink,
    call_started: Instant,
    host_time: Duration,
    stderr: TailSink,
}

impl WasiView for Ctx {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl Ctx {
    /// Times a host call so it does not count against the compute budget.
    fn timed<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let started = Instant::now();
        let result = f(self);
        self.host_time += started.elapsed();
        result
    }

    /// A denial as the extension sees it; refusals are also reported.
    fn deny(&self, denied: Denied) -> String {
        if let Denied::Refused(message) = &denied {
            log::warn!("extension {}: {message}", self.extension_id);
            (self.refusals)(message.clone());
        }
        denied.message().to_string()
    }
}

impl types::Host for Ctx {}
impl ui_v1::Host for Ctx {}

impl host::Host for Ctx {
    fn run_command(&mut self, command: types::Command) -> Result<types::CommandOutput, String> {
        self.timed(|ctx| {
            let program = ctx.guard.command(&command.program).map_err(|d| ctx.deny(d))?;
            let cwd = match &command.cwd {
                Some(dir) => Some(ctx.guard.cwd(dir, &ctx.config).map_err(|d| ctx.deny(d))?),
                None => None,
            };
            let timeout = command
                .timeout_ms
                .map(|ms| Duration::from_millis(ms.into()))
                .unwrap_or(DEFAULT_COMMAND_TIMEOUT)
                .min(MAX_COMMAND_TIMEOUT);
            let done = exec::run(exec::Run {
                program: &program,
                args: &command.args,
                cwd: cwd.as_deref(),
                env: &command.env,
                stdin: command.stdin.as_deref(),
                timeout,
                search_path: ctx.guard.search_path(),
            })?;
            let mut stderr = done.stderr;
            if done.timed_out {
                stderr.push_str(&format!("\n[okena: killed after {}s]", timeout.as_secs()));
            }
            Ok(types::CommandOutput {
                exit_code: done.exit_code,
                stdout: done.stdout,
                stderr,
            })
        })
    }

    fn read_file(&mut self, path: String) -> Result<Vec<u8>, String> {
        self.timed(|ctx| {
            let resolved = ctx.guard.path(&path, &ctx.config).map_err(|d| ctx.deny(d))?;
            let meta = std::fs::metadata(&resolved).map_err(|e| format!("cannot read `{path}`: {e}"))?;
            if meta.len() > MAX_READ_BYTES {
                return Err(format!(
                    "`{path}` is larger than {} MB",
                    MAX_READ_BYTES / (1024 * 1024)
                ));
            }
            std::fs::read(&resolved).map_err(|e| format!("cannot read `{path}`: {e}"))
        })
    }

    fn read_dir(&mut self, path: String) -> Result<Vec<types::DirEntry>, String> {
        self.timed(|ctx| {
            let resolved = ctx.guard.path(&path, &ctx.config).map_err(|d| ctx.deny(d))?;
            let entries = std::fs::read_dir(&resolved).map_err(|e| format!("cannot list `{path}`: {e}"))?;
            let mut out: Vec<types::DirEntry> = entries
                .filter_map(Result::ok)
                .map(|entry| {
                    let meta = entry.metadata().ok();
                    types::DirEntry {
                        name: entry.file_name().to_string_lossy().into_owned(),
                        is_dir: meta.as_ref().is_some_and(|m| m.is_dir()),
                        size: meta.map_or(0, |m| m.len()),
                    }
                })
                .collect();
            out.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(out)
        })
    }

    fn kv_get(&mut self, key: String) -> Option<String> {
        self.timed(|ctx| ctx.kv.lock().get(&key))
    }

    fn kv_set(&mut self, key: String, value: String) -> Result<(), String> {
        self.timed(|ctx| ctx.kv.lock().set(&key, &value))
    }

    fn kv_delete(&mut self, key: String) {
        self.timed(|ctx| ctx.kv.lock().delete(&key))
    }

    fn kv_keys(&mut self) -> Vec<String> {
        self.timed(|ctx| ctx.kv.lock().keys())
    }

    fn log(&mut self, level: types::LogLevel, message: String) {
        let level = match level {
            types::LogLevel::Trace => log::Level::Trace,
            types::LogLevel::Debug => log::Level::Debug,
            types::LogLevel::Info => log::Level::Info,
            types::LogLevel::Warn => log::Level::Warn,
            types::LogLevel::Error => log::Level::Error,
        };
        let message: String = message.chars().take(4096).collect();
        log::log!(level, "extension {}: {message}", self.extension_id);
    }

    fn config(&mut self) -> String {
        self.config.to_string()
    }

    fn projects(&mut self) -> Vec<types::Project> {
        self.timed(|ctx| {
            (ctx.projects)()
                .into_iter()
                .map(|p| types::Project {
                    id: p.id,
                    name: p.name,
                    path: p.path,
                })
                .collect()
        })
    }
}

/// A stderr that keeps only its last [`STDERR_TAIL`] bytes and never fails
/// a write — a failing stderr would turn a chatty extension's `eprintln!`
/// into a panic.
#[derive(Clone, Default)]
struct TailSink(Arc<Mutex<VecDeque<u8>>>);

impl TailSink {
    fn push(&self, bytes: &[u8]) {
        let mut tail = self.0.lock();
        tail.extend(bytes);
        let excess = tail.len().saturating_sub(STDERR_TAIL);
        tail.drain(..excess);
    }

    fn contents(&self) -> String {
        let tail = self.0.lock();
        let (a, b) = tail.as_slices();
        let mut bytes = a.to_vec();
        bytes.extend_from_slice(b);
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl wasmtime_wasi::cli::IsTerminal for TailSink {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl wasmtime_wasi::cli::StdoutStream for TailSink {
    fn p2_stream(&self) -> Box<dyn OutputStream> {
        Box::new(self.clone())
    }

    fn async_stream(&self) -> Box<dyn tokio::io::AsyncWrite + Send + Sync> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl Pollable for TailSink {
    async fn ready(&mut self) {}
}

impl OutputStream for TailSink {
    fn write(&mut self, bytes: bytes::Bytes) -> Result<(), StreamError> {
        self.push(&bytes);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), StreamError> {
        Ok(())
    }

    fn check_write(&mut self) -> Result<usize, StreamError> {
        Ok(64 * 1024)
    }
}

impl tokio::io::AsyncWrite for TailSink {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.push(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
