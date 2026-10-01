//! What each agent session has used (`ProjectData::agent_usage`): its tokens,
//! and its cost where the agent CLI reports one.
//!
//! Only the daemon can read this: the figures are in files the agent CLI
//! writes on the machine it runs on, which a client may not be. Okena counts
//! and prices nothing itself — every number here is one the CLI wrote down.
//!
//! * **Claude Code** writes each reply's usage into the conversation's
//!   transcript, and its subagents' into files beside it. Its cost is not in
//!   any file while it runs; the status line okena injects hands it over
//!   (`ActionRequest::AgentUsageReport`), along with where the transcript is.
//! * **Codex** writes a running total into its rollout file after every turn.
//! * **Copilot** writes only each reply's output tokens while it runs, and the
//!   whole session's totals when it shuts down — so its figure is output alone
//!   until then, and says so ([`AgentUsage::output_only`]).
//!
//! Neither Codex nor Copilot reports a cost in dollars, so they show none.
//!
//! Claude's conversation is the one okena named at launch. Codex and Copilot
//! name their own, so theirs is taken to be the conversation in the session's
//! directory written to since the agent started — and nothing is shown when
//! two agents of a kind share a directory, where that would be a guess.
//!
//! The figure is kept on the session and saved with it, so it outlives the
//! agent, the daemon and the files it was read from: a stopped or closed
//! session keeps showing what it used.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use okena_core::agent_usage::AgentUsage;
use okena_core::shell::ShellType;
use okena_hooks::{HookMonitor, HookRunner};
use okena_workspace::settings::AppSettings;
use okena_workspace::state::Workspace;
use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::watch;

use crate::workspace_cx::DaemonWorkspaceCx;

/// How often the session files are read. Each pass only reads what was
/// appended, and a figure changes once a turn at most.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// The agent CLIs whose records okena knows how to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Agent {
    Claude,
    Codex,
    Copilot,
}

impl Agent {
    /// The agent `command` launches, when okena can read its usage.
    fn of(command: &str) -> Option<Self> {
        match okena_core::agents::command_name(command).as_str() {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "copilot" => Some(Self::Copilot),
            _ => None,
        }
    }
}

/// A token count read from a session's files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tokens {
    pub tokens: u64,
    /// Whether the count is output alone; see [`AgentUsage::output_only`].
    pub output_only: bool,
}

/// A running count over one conversation's files, fed a line at a time so a
/// long transcript is read once and then only as it grows.
#[derive(Debug)]
pub(crate) enum Counter {
    Claude(ClaudeCount),
    Codex(CodexCount),
    Copilot(CopilotCount),
}

impl Counter {
    pub(crate) fn new(agent: Agent) -> Self {
        match agent {
            Agent::Claude => Self::Claude(ClaudeCount::default()),
            Agent::Codex => Self::Codex(CodexCount::default()),
            Agent::Copilot => Self::Copilot(CopilotCount::default()),
        }
    }

    pub(crate) fn feed(&mut self, line: &str) {
        match self {
            Self::Claude(c) => c.feed(line),
            Self::Codex(c) => c.feed(line),
            Self::Copilot(c) => c.feed(line),
        }
    }

    /// What has been counted, or `None` when the files held no usage at all.
    pub(crate) fn tokens(&self) -> Option<Tokens> {
        let (tokens, output_only) = match self {
            Self::Claude(c) => (c.total, false),
            Self::Codex(c) => (c.total, false),
            Self::Copilot(c) => (c.settled + c.live_output, c.shutdowns == 0),
        };
        (tokens > 0).then_some(Tokens {
            tokens,
            output_only,
        })
    }
}

/// Claude Code's transcript: one line per content block of a reply, each
/// carrying that reply's whole usage — so a reply with three blocks is three
/// lines with the same message id, and counts once.
#[derive(Debug, Default)]
pub(crate) struct ClaudeCount {
    by_message: HashMap<String, u64>,
    total: u64,
}

impl ClaudeCount {
    fn feed(&mut self, line: &str) {
        // Most lines are tool output with no usage in them; skip the parse.
        if !line.contains("\"usage\"") {
            return;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let Some(usage) = entry.pointer("/message/usage") else {
            return;
        };
        let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
        let tokens = count("input_tokens")
            + count("cache_creation_input_tokens")
            + count("cache_read_input_tokens")
            + count("output_tokens");
        match entry.pointer("/message/id").and_then(Value::as_str) {
            // The last line of a reply has its final usage.
            Some(id) => {
                let before = self.by_message.insert(id.to_string(), tokens).unwrap_or(0);
                self.total = self.total - before + tokens;
            }
            None => self.total += tokens,
        }
    }
}

/// Codex's rollout: a `token_count` event after each turn, holding the
/// session's total so far. The last one is the figure.
#[derive(Debug, Default)]
pub(crate) struct CodexCount {
    total: u64,
}

impl CodexCount {
    fn feed(&mut self, line: &str) {
        if !line.contains("\"token_count\"") {
            return;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            return;
        };
        if entry.pointer("/payload/type").and_then(Value::as_str) != Some("token_count") {
            return;
        }
        // `info` is null on the events that only carry rate limits.
        let Some(total) = entry.pointer("/payload/info/total_token_usage") else {
            return;
        };
        let count = |key: &str| total.get(key).and_then(Value::as_u64);
        // Codex's own total: input, cached input included, and output.
        let tokens = count("total_tokens").unwrap_or_else(|| {
            count("input_tokens").unwrap_or(0) + count("output_tokens").unwrap_or(0)
        });
        if tokens > 0 {
            self.total = tokens;
        }
    }
}

/// Copilot's event log: each reply's output tokens as it goes, and at
/// shutdown the whole run's totals per model. A resumed session logs a
/// shutdown per run, so the runs add up.
#[derive(Debug, Default)]
pub(crate) struct CopilotCount {
    /// Input and output of the runs that have shut down.
    settled: u64,
    /// Output of the replies since the last shutdown.
    live_output: u64,
    shutdowns: u32,
}

impl CopilotCount {
    fn feed(&mut self, line: &str) {
        let shutdown = line.contains("\"session.shutdown\"");
        if !shutdown && !line.contains("\"outputTokens\"") {
            return;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("assistant.message") => {
                self.live_output += event
                    .pointer("/data/outputTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
            }
            Some("session.shutdown") => {
                let Some(models) = event
                    .pointer("/data/modelMetrics")
                    .and_then(Value::as_object)
                else {
                    return;
                };
                // `inputTokens` already holds the cache reads.
                let run: u64 = models
                    .values()
                    .filter_map(|m| m.get("usage"))
                    .map(|usage| {
                        let count =
                            |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
                        count("inputTokens") + count("outputTokens")
                    })
                    .sum();
                self.settled += run;
                self.live_output = 0;
                self.shutdowns += 1;
            }
            _ => {}
        }
    }
}

/// Where each agent CLI keeps its records.
#[derive(Clone, Debug)]
pub(crate) struct Homes {
    pub claude: PathBuf,
    pub codex: PathBuf,
    pub copilot: PathBuf,
}

impl Homes {
    /// The directories the agents okena launches write to: Claude's as the
    /// PTYs are pointed at it, the others from their own overrides.
    fn resolve(settings: &AppSettings) -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let from_env = |name: &str, default: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(default))
        };
        Self {
            claude: okena_workspace::claude_env::resolve_claude_dir(settings),
            codex: from_env("CODEX_HOME", ".codex"),
            copilot: from_env("COPILOT_HOME", ".copilot"),
        }
    }
}

/// One session's agent, as far as finding its records needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    /// The agent pane's terminal. A new one is a new launch.
    pub terminal_id: String,
    pub agent: Agent,
    /// The directory the agent runs in.
    pub cwd: String,
    /// The conversation okena named at launch, for Claude.
    pub conversation: Option<String>,
    /// Where Claude's status line said its transcript is.
    pub transcript: Option<PathBuf>,
    /// Whether no other session runs the same agent in the same directory.
    pub alone: bool,
}

/// The conversation id a Claude launch names: `--session-id` on a fresh
/// start, `--resume` once okena has restarted it.
fn conversation_id(args: &[String]) -> Option<String> {
    args.windows(2)
        .find(|w| w[0] == "--session-id" || w[0] == "--resume")
        .map(|w| w[1].clone())
        .filter(|id| !id.starts_with('-'))
}

/// The agent each open session runs, by session id.
///
/// The agent is the command okena launched in the session's agent pane; a
/// session without that pane is read off its terminals' launch commands.
/// Sessions whose agent is not running have no terminal and are left out.
pub(crate) fn targets(
    workspace: &Workspace,
    transcripts: &HashMap<String, PathBuf>,
) -> HashMap<String, Target> {
    let mut found: HashMap<String, Target> = HashMap::new();
    for project in workspace.projects() {
        if !project.is_any_agent_session() {
            continue;
        }
        let Some(layout) = project.layout.as_ref() else {
            continue;
        };
        let candidates = if layout.agent_terminal_path().is_some() {
            layout.agent_terminal_id().into_iter().collect()
        } else {
            layout.collect_terminal_ids()
        };
        for terminal_id in candidates {
            let ShellType::Custom { path, args } = project.terminal_shell(&terminal_id) else {
                continue;
            };
            let Some(agent) = Agent::of(&path) else {
                continue;
            };
            found.insert(
                project.id.clone(),
                Target {
                    transcript: transcripts.get(&terminal_id).cloned(),
                    terminal_id,
                    agent,
                    cwd: project.path.clone(),
                    conversation: (agent == Agent::Claude)
                        .then(|| conversation_id(&args))
                        .flatten(),
                    alone: true,
                },
            );
            break;
        }
    }
    let mut sharing: HashMap<(Agent, String), usize> = HashMap::new();
    for target in found.values() {
        *sharing
            .entry((target.agent, target.cwd.clone()))
            .or_default() += 1;
    }
    for target in found.values_mut() {
        target.alone = sharing[&(target.agent, target.cwd.clone())] == 1;
    }
    found
}

/// The first line of `path`, parsed as JSON.
fn first_line(path: &Path) -> Option<Value> {
    let mut line = String::new();
    BufReader::new(File::open(path).ok()?)
        .read_line(&mut line)
        .ok()?;
    serde_json::from_str(&line).ok()
}

/// Whether two directories are the same one, through symlinks: macOS hands
/// out `/tmp` and records `/private/tmp`.
fn same_dir(a: &str, b: &str) -> bool {
    let real = |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p));
    a == b || real(a) == real(b)
}

fn modified(path: &Path) -> Option<SystemTime> {
    path.metadata().and_then(|m| m.modified()).ok()
}

/// The transcript of the Claude conversation `id`, wherever under `home` it
/// was filed: Claude names the folder after the directory it ran in.
fn claude_transcript(home: &Path, id: &str) -> Option<PathBuf> {
    let file = format!("{id}.jsonl");
    std::fs::read_dir(home.join("projects"))
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(&file))
        .find(|path| path.is_file())
}

/// The files of a Claude conversation: its transcript, and one per subagent
/// it started, kept in a folder named after the transcript.
fn claude_files(transcript: &Path) -> Vec<PathBuf> {
    let mut files = vec![transcript.to_path_buf()];
    let subagents = transcript.with_extension("").join("subagents");
    if let Ok(entries) = std::fs::read_dir(subagents) {
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .collect();
        found.sort();
        files.extend(found);
    }
    files
}

/// The Copilot event log last written to since `since` by a session that
/// started in `cwd`.
fn copilot_events(home: &Path, cwd: &str, since: SystemTime) -> Option<PathBuf> {
    std::fs::read_dir(home.join("session-state"))
        .ok()?
        .flatten()
        .map(|entry| entry.path().join("events.jsonl"))
        .filter_map(|events| Some((modified(&events)?, events)))
        .filter(|(at, _)| *at >= since)
        .filter(|(_, events)| {
            first_line(events)
                .as_ref()
                .and_then(|start| start.pointer("/data/context/cwd"))
                .and_then(Value::as_str)
                .is_some_and(|dir| same_dir(dir, cwd))
        })
        .max_by_key(|(at, _)| *at)
        .map(|(_, events)| events)
}

/// Every Codex rollout under `dir`: `sessions/YYYY/MM/DD/rollout-*.jsonl`.
fn codex_rollouts(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                codex_rollouts(&path, depth - 1, found);
            }
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
        {
            found.push(path);
        }
    }
}

/// The Codex rollout last written to since `since` by a session that started
/// in `cwd`.
fn codex_rollout(home: &Path, cwd: &str, since: SystemTime) -> Option<PathBuf> {
    let mut rollouts = Vec::new();
    codex_rollouts(&home.join("sessions"), 3, &mut rollouts);
    rollouts
        .into_iter()
        .filter_map(|rollout| Some((modified(&rollout)?, rollout)))
        .filter(|(at, _)| *at >= since)
        .filter(|(_, rollout)| {
            first_line(rollout)
                .as_ref()
                .and_then(|meta| meta.pointer("/payload/cwd"))
                .and_then(Value::as_str)
                .is_some_and(|dir| same_dir(dir, cwd))
        })
        .max_by_key(|(at, _)| *at)
        .map(|(_, rollout)| rollout)
}

/// The files holding `target`'s conversation, its main one first. Empty when
/// the agent has written nothing yet, or which conversation is its own
/// cannot be told.
fn conversation_files(target: &Target, homes: &Homes, since: SystemTime) -> Vec<PathBuf> {
    match target.agent {
        Agent::Claude => target
            .transcript
            .clone()
            .filter(|path| path.is_file())
            .or_else(|| {
                target
                    .conversation
                    .as_deref()
                    .and_then(|id| claude_transcript(&homes.claude, id))
            })
            .map(|transcript| claude_files(&transcript))
            .unwrap_or_default(),
        Agent::Codex if target.alone => codex_rollout(&homes.codex, &target.cwd, since)
            .into_iter()
            .collect(),
        Agent::Copilot if target.alone => copilot_events(&homes.copilot, &target.cwd, since)
            .into_iter()
            .collect(),
        Agent::Codex | Agent::Copilot => Vec::new(),
    }
}

/// Feed `counter` the whole lines appended to `path` past `offset`, and
/// return where the next read starts. `None` when the file is shorter than
/// what was already read: it was replaced, and the count is of something else.
fn read_appended(path: &Path, offset: u64, counter: &mut Counter) -> Option<u64> {
    let Ok(mut file) = File::open(path) else {
        return Some(offset);
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len < offset {
        return None;
    }
    if len == offset || file.seek(SeekFrom::Start(offset)).is_err() {
        return Some(offset);
    }
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut at = offset;
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            // A line still being written is left for the next read.
            Ok(read) if read > 0 && line.ends_with(b"\n") => {
                at += read as u64;
                if let Ok(text) = std::str::from_utf8(&line) {
                    counter.feed(text.trim_end());
                }
            }
            _ => break,
        }
    }
    Some(at)
}

/// What is being read for one session, kept between passes.
#[derive(Debug)]
struct Reading {
    target: Target,
    /// When this launch was first seen. Codex and Copilot conversations
    /// written to before it are someone else's, or an earlier launch's.
    since: SystemTime,
    /// The conversation's main file; a different one starts the count over.
    main: Option<PathBuf>,
    counter: Counter,
    /// How far into each file the count has read.
    offsets: HashMap<PathBuf, u64>,
}

impl Reading {
    fn new(target: Target, since: SystemTime) -> Self {
        Self {
            counter: Counter::new(target.agent),
            target,
            since,
            main: None,
            offsets: HashMap::new(),
        }
    }

    /// Whether `target` is the launch being read. The transcript the status
    /// line names can arrive after the first read, and is not a new launch.
    fn is_for(&self, target: &Target) -> bool {
        self.target.terminal_id == target.terminal_id
            && self.target.agent == target.agent
            && self.target.conversation == target.conversation
    }

    fn start_over(&mut self) {
        self.counter = Counter::new(self.target.agent);
        self.offsets.clear();
    }

    /// Read what the conversation's files have gained and return the count.
    fn read(&mut self, homes: &Homes) -> Option<Tokens> {
        let files = conversation_files(&self.target, homes, self.since);
        let main = files.first().cloned();
        // The agent has not written yet, or its file is briefly out of
        // sight: what was counted stands.
        if main.is_none() {
            return self.counter.tokens();
        }
        if main != self.main {
            self.start_over();
            self.main = main;
        }
        for attempt in 0..2 {
            let mut replaced = false;
            for file in &files {
                let offset = self.offsets.get(file).copied().unwrap_or(0);
                match read_appended(file, offset, &mut self.counter) {
                    Some(next) => {
                        self.offsets.insert(file.clone(), next);
                    }
                    None => {
                        replaced = true;
                        break;
                    }
                }
            }
            if !replaced || attempt == 1 {
                break;
            }
            self.start_over();
        }
        self.counter.tokens()
    }
}

/// The session's usage with `read` in it, when that changes how it is shown.
/// The cost is the status line's to set, and is kept.
pub(crate) fn with_tokens(current: Option<&AgentUsage>, read: Tokens) -> Option<AgentUsage> {
    let next = AgentUsage {
        tokens: read.tokens,
        output_only: read.output_only,
        cost_usd: current.and_then(|usage| usage.cost_usd),
    };
    (!current.cloned().unwrap_or_default().shown_as(&next)).then_some(next)
}

/// The session's usage with `cost_usd` in it, when that changes how it is
/// shown. The tokens are the poll's to set, and are kept.
pub(crate) fn with_cost(current: Option<&AgentUsage>, cost_usd: f64) -> Option<AgentUsage> {
    let next = AgentUsage {
        cost_usd: Some(cost_usd),
        ..current.cloned().unwrap_or_default()
    };
    (!current.cloned().unwrap_or_default().shown_as(&next)).then_some(next)
}

/// What the poll is reading for each session, and what Claude's status line
/// has said about where to look.
#[derive(Default)]
pub struct AgentUsageTracker {
    /// Terminal id → the transcript its Claude said it writes.
    transcripts: Mutex<HashMap<String, PathBuf>>,
    /// Session id → what is being read for it.
    readings: Mutex<HashMap<String, Reading>>,
}

impl AgentUsageTracker {
    /// Record where the Claude in `terminal_id` keeps its transcript.
    pub fn record_transcript(&self, terminal_id: &str, transcript: PathBuf) {
        self.transcripts
            .lock()
            .insert(terminal_id.to_string(), transcript);
    }

    fn transcripts(&self) -> HashMap<String, PathBuf> {
        self.transcripts.lock().clone()
    }

    /// Read every session's files and return each one's count, by session id.
    ///
    /// A session whose agent has stopped is read once more — Copilot writes
    /// its totals as it exits — and then let go; its figure stays on the
    /// session.
    pub(crate) fn read(
        &self,
        targets: HashMap<String, Target>,
        homes: &Homes,
        now: SystemTime,
    ) -> HashMap<String, Tokens> {
        let mut readings = self.readings.lock();
        for (session, target) in &targets {
            match readings.get_mut(session) {
                Some(reading) if reading.is_for(target) => reading.target = target.clone(),
                _ => {
                    readings.insert(session.clone(), Reading::new(target.clone(), now));
                }
            }
        }
        let mut counts = HashMap::new();
        readings.retain(|session, reading| {
            if let Some(tokens) = reading.read(homes) {
                counts.insert(session.clone(), tokens);
            }
            targets.contains_key(session)
        });
        let live: Vec<&str> = targets.values().map(|t| t.terminal_id.as_str()).collect();
        self.transcripts
            .lock()
            .retain(|terminal, _| live.contains(&terminal.as_str()));
        counts
    }
}

/// Put `counts` on their sessions. Returns whether any shown figure changed.
pub(crate) fn apply(workspace: &mut Workspace, counts: &HashMap<String, Tokens>) -> bool {
    let mut changed = false;
    for project in workspace.data.projects.iter_mut() {
        let Some(read) = counts.get(&project.id) else {
            continue;
        };
        if let Some(next) = with_tokens(project.agent_usage.as_ref(), *read) {
            project.agent_usage = Some(next);
            changed = true;
        }
    }
    changed
}

/// Keep every agent session's usage current until the daemon shuts down.
///
/// Must run on the daemon's `LocalSet`, like the other reactor tasks. The
/// files are read on the blocking pool, off the workspace lock.
pub async fn run_agent_usage_poll(
    tracker: Arc<AgentUsageTracker>,
    workspace: Arc<Mutex<Workspace>>,
    settings: Arc<Mutex<AppSettings>>,
    workspace_tick: watch::Sender<u64>,
    hook_runner: Option<HookRunner>,
    hook_monitor: Option<HookMonitor>,
) {
    loop {
        let targets = targets(&workspace.lock(), &tracker.transcripts());
        let homes = Homes::resolve(&settings.lock());
        let reader = tracker.clone();
        let counts = tokio::task::spawn_blocking(move || {
            reader.read(targets, &homes, SystemTime::now())
        })
        .await;
        match counts {
            Ok(counts) => {
                let mut ws = workspace.lock();
                if apply(&mut ws, &counts) {
                    let mut cx =
                        DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
                    ws.notify_data(&mut cx);
                }
            }
            Err(e) => log::warn!("agent usage poll failed: {e}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_state::{LayoutNode, ProjectData};
    use serde_json::json;
    use std::io::Write;

    fn feed_all(agent: Agent, lines: &[Value]) -> Option<Tokens> {
        let mut counter = Counter::new(agent);
        for line in lines {
            counter.feed(&line.to_string());
        }
        counter.tokens()
    }

    fn full(tokens: u64) -> Option<Tokens> {
        Some(Tokens {
            tokens,
            output_only: false,
        })
    }

    /// One content block of a Claude reply, as the transcript holds it.
    fn claude_reply(id: &str, input: u64, cache_write: u64, cache_read: u64, output: u64) -> Value {
        json!({
            "type": "assistant", "sessionId": "c1",
            "message": {
                "id": id, "model": "claude-opus-5-5", "role": "assistant",
                "usage": {
                    "input_tokens": input,
                    "cache_creation_input_tokens": cache_write,
                    "cache_read_input_tokens": cache_read,
                    "output_tokens": output,
                    "service_tier": "standard",
                },
            },
        })
    }

    fn codex_count(total: Value) -> Value {
        json!({
            "timestamp": "2026-09-30T10:00:00Z", "type": "event_msg",
            "payload": { "type": "token_count", "info": total, "rate_limits": {} },
        })
    }

    fn copilot_reply(output: u64) -> Value {
        json!({ "type": "assistant.message", "data": { "messageId": "m", "outputTokens": output } })
    }

    fn copilot_shutdown(models: Value) -> Value {
        json!({
            "type": "session.shutdown",
            "data": { "shutdownType": "routine", "totalPremiumRequests": 3, "modelMetrics": models },
        })
    }

    fn write_lines(path: &Path, lines: &[Value]) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("open");
        for line in lines {
            writeln!(file, "{line}").expect("write");
        }
    }

    #[test]
    fn a_claude_reply_counts_once_with_its_cache_tokens() {
        let counted = feed_all(
            Agent::Claude,
            &[
                json!({ "type": "user", "message": { "role": "user", "content": "go" } }),
                // One reply, three content blocks: the same usage three times.
                claude_reply("msg_1", 2, 22_201, 25_653, 668),
                claude_reply("msg_1", 2, 22_201, 25_653, 668),
                claude_reply("msg_1", 2, 22_201, 25_653, 668),
                claude_reply("msg_2", 10, 0, 48_000, 90),
                json!({ "type": "attachment", "note": "mentions \"usage\" in passing" }),
            ],
        );
        assert_eq!(counted, full(2 + 22_201 + 25_653 + 668 + 10 + 48_000 + 90));
    }

    #[test]
    fn a_claude_replys_last_line_has_its_final_usage() {
        // Written while streaming with the output so far, then with all of it.
        let counted = feed_all(
            Agent::Claude,
            &[
                claude_reply("msg_1", 100, 0, 0, 1),
                claude_reply("msg_1", 100, 0, 0, 250),
            ],
        );
        assert_eq!(counted, full(350));
    }

    #[test]
    fn codex_reports_its_own_running_total() {
        let total = |input: u64, cached: u64, output: u64, total: u64| {
            json!({
                "total_token_usage": {
                    "input_tokens": input, "cached_input_tokens": cached,
                    "output_tokens": output, "reasoning_output_tokens": 5, "total_tokens": total,
                },
                "last_token_usage": { "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 },
            })
        };
        let counted = feed_all(
            Agent::Codex,
            &[
                json!({ "type": "session_meta", "payload": { "id": "x", "cwd": "/w" } }),
                codex_count(total(1000, 600, 50, 1050)),
                // Rate limits alone, between turns.
                codex_count(Value::Null),
                codex_count(total(5000, 4200, 320, 5320)),
                json!({ "type": "response_item", "payload": { "type": "message" } }),
            ],
        );
        assert_eq!(counted, full(5320), "the last total, not the sum of them");

        // A total without `total_tokens` is its input and output.
        let older = feed_all(
            Agent::Codex,
            &[codex_count(json!({
                "total_token_usage": { "input_tokens": 700, "cached_input_tokens": 500, "output_tokens": 30 },
            }))],
        );
        assert_eq!(older, full(730));
    }

    #[test]
    fn copilot_is_output_alone_until_it_shuts_down() {
        let live = [copilot_reply(276), copilot_reply(158)];
        assert_eq!(
            feed_all(Agent::Copilot, &live),
            Some(Tokens {
                tokens: 434,
                output_only: true
            })
        );

        let shutdown = copilot_shutdown(json!({
            "claude-opus-4.6": {
                "requests": { "count": 201, "cost": 15 },
                "usage": {
                    "inputTokens": 15_175_542, "outputTokens": 98_754,
                    "cacheReadTokens": 14_397_658, "cacheWriteTokens": 0, "reasoningTokens": 0,
                },
            },
            "claude-haiku-4.5": {
                "requests": { "count": 54, "cost": 0 },
                "usage": { "inputTokens": 1_781_072, "outputTokens": 40_105, "cacheReadTokens": 1_444_344 },
            },
        }));
        let mut ended = live.to_vec();
        ended.push(shutdown);
        let settled = 15_175_542 + 98_754 + 1_781_072 + 40_105;
        assert_eq!(
            feed_all(Agent::Copilot, &ended),
            full(settled),
            "the run's totals replace the output counted while it ran"
        );

        // Resumed: what it says next is on top of the run that ended.
        ended.push(copilot_reply(1000));
        assert_eq!(feed_all(Agent::Copilot, &ended), full(settled + 1000));
        ended.push(copilot_shutdown(json!({
            "gpt-5": { "usage": { "inputTokens": 20_000, "outputTokens": 1000 } },
        })));
        assert_eq!(feed_all(Agent::Copilot, &ended), full(settled + 21_000));
    }

    #[test]
    fn records_without_usage_count_as_nothing() {
        for agent in [Agent::Claude, Agent::Codex, Agent::Copilot] {
            let mut counter = Counter::new(agent);
            for line in [
                "",
                "not json",
                r#"{"type":"user","message":{"content":"\"usage\" \"token_count\" \"outputTokens\""}}"#,
                r#"{"type":"session.start","data":{"context":{"cwd":"/w"}}}"#,
                r#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#,
                r#"{"type":"assistant","message":{"id":"m","usage":{"input_tokens":0,"output_tokens":0}}}"#,
            ] {
                counter.feed(line);
            }
            assert_eq!(counter.tokens(), None, "{agent:?}");
        }
    }

    #[test]
    fn only_whole_lines_are_read_and_only_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.jsonl");
        let mut counter = Counter::new(Agent::Claude);
        write_lines(&path, &[claude_reply("m1", 100, 0, 0, 10)]);
        // The next reply, caught half-written.
        let next = claude_reply("m2", 200, 0, 0, 20).to_string();
        let (head, tail) = next.split_at(40);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(head.as_bytes()))
            .expect("append");

        let at = read_appended(&path, 0, &mut counter).expect("not replaced");
        assert_eq!(counter.tokens(), full(110));
        assert_eq!(read_appended(&path, at, &mut counter), Some(at), "nothing new");
        assert_eq!(counter.tokens(), full(110));

        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(format!("{tail}\n").as_bytes()))
            .expect("append");
        let end = read_appended(&path, at, &mut counter).expect("not replaced");
        assert_eq!(counter.tokens(), full(330));
        assert_eq!(end, path.metadata().expect("meta").len());

        // Shorter than what was read: not the file that was counted.
        std::fs::write(&path, "{}\n").expect("replace");
        assert_eq!(read_appended(&path, end, &mut counter), None);
        // A file that is not there yet is not an error.
        assert_eq!(
            read_appended(&dir.path().join("absent"), 0, &mut counter),
            Some(0)
        );
    }

    fn homes(root: &Path) -> Homes {
        Homes {
            claude: root.join("claude"),
            codex: root.join("codex"),
            copilot: root.join("copilot"),
        }
    }

    fn target(agent: Agent, terminal: &str, cwd: &str) -> Target {
        Target {
            terminal_id: terminal.into(),
            agent,
            cwd: cwd.into(),
            conversation: None,
            transcript: None,
            alone: true,
        }
    }

    fn claude_target(terminal: &str, conversation: &str) -> Target {
        Target {
            conversation: Some(conversation.into()),
            ..target(Agent::Claude, terminal, "/work/a")
        }
    }

    fn earlier() -> SystemTime {
        SystemTime::now() - Duration::from_secs(60)
    }

    fn later() -> SystemTime {
        SystemTime::now() + Duration::from_secs(60)
    }

    #[test]
    fn a_claude_conversation_is_found_by_its_id_with_its_subagents() {
        let dir = tempfile::tempdir().expect("tempdir");
        let homes = homes(dir.path());
        let projects = homes.claude.join("projects");
        let transcript = projects.join("-work-a/c1.jsonl");
        write_lines(&transcript, &[claude_reply("m1", 100, 0, 0, 10)]);
        write_lines(
            &projects.join("-work-a/c1/subagents/agent-a1.jsonl"),
            &[claude_reply("s1", 1000, 0, 0, 100)],
        );
        // Another conversation in the same folder, and one elsewhere.
        write_lines(
            &projects.join("-work-a/c2.jsonl"),
            &[claude_reply("x1", 7, 0, 0, 7)],
        );
        write_lines(
            &projects.join("-work-b/c3.jsonl"),
            &[claude_reply("y1", 9, 0, 0, 9)],
        );

        let mut reading = Reading::new(claude_target("t1", "c1"), SystemTime::now());
        assert_eq!(reading.read(&homes), full(110 + 1100));
        assert_eq!(reading.main.as_deref(), Some(transcript.as_path()));

        // A subagent started later is picked up; nothing is counted twice.
        write_lines(
            &projects.join("-work-a/c1/subagents/agent-a2.jsonl"),
            &[claude_reply("s2", 5, 0, 0, 5)],
        );
        assert_eq!(reading.read(&homes), full(110 + 1100 + 10));

        // A conversation okena did not name, with nothing to say where it is.
        let mut unnamed = Reading::new(target(Agent::Claude, "t2", "/work/a"), earlier());
        assert_eq!(unnamed.read(&homes), None);
    }

    #[test]
    fn the_transcript_claude_names_is_read_over_a_search() {
        let dir = tempfile::tempdir().expect("tempdir");
        let homes = homes(dir.path());
        let named = dir.path().join("elsewhere/zz.jsonl");
        write_lines(&named, &[claude_reply("m1", 40, 0, 0, 2)]);
        let mut continued = target(Agent::Claude, "t1", "/work/a");
        continued.transcript = Some(named);
        assert_eq!(
            Reading::new(continued, SystemTime::now()).read(&homes),
            full(42)
        );
    }

    #[test]
    fn copilot_and_codex_are_found_by_directory_and_time() {
        let dir = tempfile::tempdir().expect("tempdir");
        let homes = homes(dir.path());
        let cwd = dir.path().join("work");
        std::fs::create_dir_all(&cwd).expect("cwd");
        let cwd = cwd.to_string_lossy().into_owned();
        let start = |cwd: &str| json!({ "type": "session.start", "data": { "context": { "cwd": cwd } } });
        let meta = |cwd: &str| json!({ "type": "session_meta", "payload": { "id": "r", "cwd": cwd } });

        write_lines(
            &homes.copilot.join("session-state/mine/events.jsonl"),
            &[start(&cwd), copilot_reply(300)],
        );
        write_lines(
            &homes.copilot.join("session-state/other/events.jsonl"),
            &[start("/somewhere/else"), copilot_reply(9000)],
        );
        // A session that never spoke has a folder and no event log.
        std::fs::create_dir_all(homes.copilot.join("session-state/empty")).expect("dir");
        write_lines(
            &homes.codex.join("sessions/2026/09/30/rollout-2026-09-30T10-00-00-r.jsonl"),
            &[
                meta(&cwd),
                codex_count(json!({ "total_token_usage": { "total_tokens": 4321 } })),
            ],
        );
        write_lines(
            &homes.codex.join("sessions/2026/09/29/rollout-2026-09-29T10-00-00-q.jsonl"),
            &[
                meta("/somewhere/else"),
                codex_count(json!({ "total_token_usage": { "total_tokens": 99 } })),
            ],
        );

        let read = |agent: Agent, since: SystemTime, alone: bool| {
            let mut t = target(agent, "t1", &cwd);
            t.alone = alone;
            Reading::new(t, since).read(&homes)
        };
        assert_eq!(
            read(Agent::Copilot, earlier(), true),
            Some(Tokens {
                tokens: 300,
                output_only: true
            })
        );
        assert_eq!(read(Agent::Codex, earlier(), true), full(4321));
        // Written before this launch: an earlier conversation in the directory.
        assert_eq!(read(Agent::Copilot, later(), true), None);
        assert_eq!(read(Agent::Codex, later(), true), None);
        // Two of a kind in one directory: whose it is would be a guess.
        assert_eq!(read(Agent::Copilot, earlier(), false), None);
        assert_eq!(read(Agent::Codex, earlier(), false), None);
    }

    #[test]
    fn each_session_reads_its_own_and_rises_as_it_works() {
        let dir = tempfile::tempdir().expect("tempdir");
        let homes = homes(dir.path());
        let projects = homes.claude.join("projects");
        let a = projects.join("-work-a/ca.jsonl");
        let b = projects.join("-work-b/cb.jsonl");
        write_lines(&a, &[claude_reply("a1", 1000, 0, 0, 100)]);
        write_lines(&b, &[claude_reply("b1", 50, 0, 0, 5)]);

        let tracker = AgentUsageTracker::default();
        let targets: HashMap<String, Target> = [
            ("sa".to_string(), claude_target("ta", "ca")),
            ("sb".to_string(), claude_target("tb", "cb")),
            // Launched, and has not written anything.
            ("sc".to_string(), claude_target("tc", "cc")),
        ]
        .into_iter()
        .collect();
        let now = SystemTime::now();

        let first = tracker.read(targets.clone(), &homes, now);
        assert_eq!(first.get("sa").copied(), full(1100));
        assert_eq!(first.get("sb").copied(), full(55));
        assert!(!first.contains_key("sc"), "no data is no figure");

        write_lines(&a, &[claude_reply("a2", 2000, 0, 0, 200)]);
        let second = tracker.read(targets.clone(), &homes, now);
        assert_eq!(second.get("sa").copied(), full(3300));
        assert_eq!(second.get("sb").copied(), full(55));

        // `sa`'s agent stops: its file is read once more, then let go.
        write_lines(&a, &[claude_reply("a3", 1, 0, 0, 1)]);
        let mut running = targets.clone();
        running.remove("sa");
        let third = tracker.read(running.clone(), &homes, now);
        assert_eq!(third.get("sa").copied(), full(3302));
        let fourth = tracker.read(running, &homes, now);
        assert!(!fourth.contains_key("sa"));
        assert_eq!(fourth.get("sb").copied(), full(55));
    }

    #[test]
    fn a_new_launch_starts_its_count_over() {
        let dir = tempfile::tempdir().expect("tempdir");
        let homes = homes(dir.path());
        let projects = homes.claude.join("projects");
        write_lines(
            &projects.join("-work-a/c1.jsonl"),
            &[claude_reply("m1", 5000, 0, 0, 500)],
        );
        write_lines(
            &projects.join("-work-a/c2.jsonl"),
            &[claude_reply("n1", 30, 0, 0, 3)],
        );
        let tracker = AgentUsageTracker::default();
        let run = |target: Target| {
            let targets = [("s".to_string(), target)].into_iter().collect();
            tracker.read(targets, &homes, SystemTime::now())["s"]
        };
        assert_eq!(Some(run(claude_target("t1", "c1"))), full(5500));
        // Restarted and resumed: the same conversation in a new terminal.
        assert_eq!(Some(run(claude_target("t2", "c1"))), full(5500));
        // Started again from its brief: a new conversation.
        assert_eq!(Some(run(claude_target("t3", "c2"))), full(33));
    }

    #[test]
    fn the_status_lines_transcript_is_kept_while_its_terminal_runs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let homes = homes(dir.path());
        let named = dir.path().join("zz.jsonl");
        write_lines(&named, &[claude_reply("m1", 40, 0, 0, 2)]);
        let tracker = AgentUsageTracker::default();
        tracker.record_transcript("t1", named.clone());

        let mut workspace = Workspace::new(test_support_data(vec![session("s", "t1", "claude", &[])]));
        let targets = targets(&workspace, &tracker.transcripts());
        assert_eq!(targets["s"].transcript.as_deref(), Some(named.as_path()));
        let counts = tracker.read(targets, &homes, SystemTime::now());
        assert_eq!(counts.get("s").copied(), full(42));
        assert!(apply(&mut workspace, &counts));
        assert_eq!(workspace.projects()[0].agent_usage.as_ref().map(|u| u.tokens), Some(42));

        // The terminal is gone: what it said about its transcript goes too.
        tracker.read(HashMap::new(), &homes, SystemTime::now());
        assert!(tracker.transcripts().is_empty());
    }

    fn test_support_data(projects: Vec<ProjectData>) -> okena_state::WorkspaceData {
        let mut data = crate::test_support::empty_workspace_data();
        data.project_order = projects.iter().map(|p| p.id.clone()).collect();
        data.projects = projects;
        data
    }

    /// An agent session whose agent pane runs `command`.
    fn session(id: &str, terminal: &str, command: &str, args: &[&str]) -> ProjectData {
        let mut project: ProjectData = serde_json::from_value(json!({
            "id": id, "name": id, "path": "/work/shared", "custom_session": "go",
        }))
        .expect("project");
        project.layout = Some(LayoutNode::Terminal {
            terminal_id: Some(terminal.into()),
            minimized: false,
            detached: false,
            shell_type: ShellType::Custom {
                path: command.into(),
                args: args.iter().map(|a| a.to_string()).collect(),
            },
            zoom_level: 1.0,
            agent: true,
        });
        project
    }

    #[test]
    fn a_sessions_agent_is_the_command_in_its_agent_pane() {
        let mut stopped = session("stopped", "t9", "claude", &["--session-id", "c9"]);
        if let Some(LayoutNode::Terminal { terminal_id, .. }) = stopped.layout.as_mut() {
            *terminal_id = None;
        }
        let mut plain: ProjectData = serde_json::from_value(json!({
            "id": "repo", "name": "repo", "path": "/work/repo",
        }))
        .expect("project");
        plain.layout = session("x", "t8", "claude", &[]).layout;
        let mut alone = session("copilot-alone", "t5", "copilot", &[]);
        alone.path = "/work/alone".into();

        let workspace = Workspace::new(test_support_data(vec![
            session("fresh", "t1", "/usr/local/bin/claude", &["--session-id", "c1", "Work on X"]),
            session("resumed", "t2", "claude", &["--resume", "c2", "--model", "opus"]),
            session("continued", "t3", "claude", &["--continue"]),
            session("codex-a", "t4", "codex", &["-c", "x=1"]),
            session("codex-b", "t6", "codex", &[]),
            alone,
            session("shell", "t7", "/bin/zsh", &[]),
            session("aider", "t10", "aider", &[]),
            stopped,
            plain,
        ]));
        let found = targets(&workspace, &HashMap::new());

        let mut ids: Vec<&str> = found.keys().map(String::as_str).collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            ["codex-a", "codex-b", "continued", "copilot-alone", "fresh", "resumed"],
            "not a shell, an unknown agent, a stopped agent or an ordinary project"
        );
        assert_eq!(found["fresh"].agent, Agent::Claude);
        assert_eq!(found["fresh"].conversation.as_deref(), Some("c1"));
        assert_eq!(found["fresh"].terminal_id, "t1");
        assert_eq!(found["resumed"].conversation.as_deref(), Some("c2"));
        assert_eq!(found["continued"].conversation, None);
        assert_eq!(found["codex-a"].conversation, None, "codex names its own");
        // The two Codex sessions in one directory cannot be told apart.
        assert!(found["copilot-alone"].alone);
        assert!(!found["codex-a"].alone && !found["codex-b"].alone);
    }

    #[test]
    fn a_figure_changes_only_when_it_reads_differently() {
        let read = |tokens: u64| Tokens {
            tokens,
            output_only: false,
        };
        let first = with_tokens(None, read(1_234_000)).expect("the first figure");
        assert_eq!(first.tokens, 1_234_000);
        assert_eq!(first.cost_usd, None);

        // 1.2M either way: nothing to send.
        assert_eq!(with_tokens(Some(&first), read(1_236_000)), None);
        // The cost is the status line's, and survives a new token count.
        let priced = with_cost(Some(&first), 1.5).expect("a cost is new");
        let grown = with_tokens(Some(&priced), read(1_400_000)).expect("1.4M");
        assert_eq!(grown.cost_usd, Some(1.5));
        assert_eq!(grown.tokens, 1_400_000);
        // And the tokens survive a new cost.
        assert_eq!(with_cost(Some(&grown), 1.502), None, "$1.50 either way");
        let dearer = with_cost(Some(&grown), 2.25).expect("$2.25");
        assert_eq!(dearer.tokens, 1_400_000);
        // Copilot's totals arriving: the same digits would still be a change.
        let live = with_tokens(
            None,
            Tokens {
                tokens: 500,
                output_only: true,
            },
        )
        .expect("live");
        assert!(with_tokens(Some(&live), read(500)).is_some());
        // A cost before any tokens are known.
        let early = with_cost(None, 0.25).expect("a cost alone");
        assert_eq!((early.tokens, early.cost_usd), (0, Some(0.25)));
        // Nothing spent and nothing known: nothing to show, so nothing to say.
        assert_eq!(with_cost(None, 0.0), None);
        // A fresh conversation has spent nothing: the last one's cost goes.
        assert_eq!(
            with_cost(Some(&dearer), 0.0).and_then(|u| u.cost_label()),
            None
        );
    }

    #[test]
    fn a_closed_or_stopped_session_keeps_its_last_figure() {
        let mut closed = session("closed", "t1", "claude", &["--session-id", "c1"]);
        closed.closed_at = Some(5);
        closed.agent_usage = Some(AgentUsage {
            tokens: 9000,
            output_only: false,
            cost_usd: Some(0.4),
        });
        if let Some(LayoutNode::Terminal { terminal_id, .. }) = closed.layout.as_mut() {
            *terminal_id = None;
        }
        let running = session("running", "t2", "claude", &["--session-id", "c2"]);
        let mut workspace = Workspace::new(test_support_data(vec![closed, running]));

        let dir = tempfile::tempdir().expect("tempdir");
        let tracker = AgentUsageTracker::default();
        let counts = tracker.read(
            targets(&workspace, &HashMap::new()),
            &homes(dir.path()),
            SystemTime::now(),
        );
        assert!(counts.is_empty(), "nothing on disk for either");
        assert!(!apply(&mut workspace, &counts));
        let usage = |ws: &Workspace, id: &str| {
            ws.projects()
                .iter()
                .find(|p| p.id == id)
                .and_then(|p| p.agent_usage.clone())
        };
        assert_eq!(
            usage(&workspace, "closed"),
            Some(AgentUsage {
                tokens: 9000,
                output_only: false,
                cost_usd: Some(0.4)
            })
        );
        assert_eq!(usage(&workspace, "running"), None, "no figure, no placeholder");
    }
}
