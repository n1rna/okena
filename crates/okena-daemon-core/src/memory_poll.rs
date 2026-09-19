//! How much memory the daemon and its terminals hold
//! ([`ApiProcessMemory`], sent to clients inside `SystemStatsChanged`).
//!
//! Only the daemon can measure this: it owns the PTYs and knows their pids,
//! while a client is a thin mirror that may not even run on this machine. The
//! poll walks every terminal's process tree — its shell, an agent CLI in it,
//! and everything that spawns — and sums resident memory per project, so an
//! agent session's figure covers the split and tab shells beside the agent too.
//!
//! The daemon's own figure is its process alone. Its children are the
//! terminals (or their attach processes), already counted in the terminal
//! total; counting them again would double the bill.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use okena_core::api::ApiProcessMemory;
use okena_terminal::pty_manager::PtyManager;
use okena_workspace::state::Workspace;
use parking_lot::Mutex;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tokio::sync::watch;

/// How often the figures are measured. Memory moves slowly, and each pass
/// reads the whole process table, so this is looser than the 2s CPU refresh.
const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// One process, as the measurement needs it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ProcessEntry {
    pub parent: Option<u32>,
    /// Resident memory in bytes.
    pub bytes: u64,
}

/// What to measure: the processes each terminal is rooted at, and which
/// terminals each project owns.
#[derive(Default)]
pub(crate) struct MemoryTargets {
    /// Terminal id → the pids its tree hangs from. More than one with a session
    /// backend: the attach process the PTY runs, and the shell it reaches.
    pub terminal_roots: HashMap<String, Vec<u32>>,
    /// Project id → its terminal ids.
    pub project_terminals: HashMap<String, Vec<String>>,
}

/// Sum resident memory over the process table.
///
/// A terminal's tree is every root and all its descendants. Each process
/// counts once per figure, however many roots reach it, and the daemon's own
/// pid is never walked into, so a stray root at the daemon cannot swallow the
/// daemon and every other terminal with it.
pub(crate) fn measure(
    table: &HashMap<u32, ProcessEntry>,
    targets: &MemoryTargets,
    daemon_pid: u32,
) -> ApiProcessMemory {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&pid, entry) in table {
        if let Some(parent) = entry.parent {
            children.entry(parent).or_default().push(pid);
        }
    }

    let tree_of = |roots: &[u32]| -> HashSet<u32> {
        let mut seen = HashSet::new();
        let mut stack: Vec<u32> = roots.to_vec();
        while let Some(pid) = stack.pop() {
            if pid == daemon_pid || !table.contains_key(&pid) || !seen.insert(pid) {
                continue;
            }
            if let Some(kids) = children.get(&pid) {
                stack.extend(kids);
            }
        }
        seen
    };
    let bytes_of = |pids: &HashSet<u32>| -> u64 { pids.iter().map(|pid| table[pid].bytes).sum() };

    let trees: HashMap<&str, HashSet<u32>> = targets
        .terminal_roots
        .iter()
        .map(|(id, roots)| (id.as_str(), tree_of(roots)))
        .collect();

    let all: HashSet<u32> = trees.values().flatten().copied().collect();

    let projects = targets
        .project_terminals
        .iter()
        .filter_map(|(project, terminals)| {
            let pids: HashSet<u32> = terminals
                .iter()
                .filter_map(|t| trees.get(t.as_str()))
                .flatten()
                .copied()
                .collect();
            let bytes = bytes_of(&pids);
            (bytes > 0).then(|| (project.clone(), bytes))
        })
        .collect();

    ApiProcessMemory {
        daemon_bytes: table.get(&daemon_pid).map_or(0, |e| e.bytes),
        terminals_bytes: bytes_of(&all),
        projects,
    }
}

/// Reads the process table, keeping one [`System`] between passes.
pub(crate) struct ProcessTable {
    system: System,
}

impl ProcessTable {
    pub(crate) fn new() -> Self {
        Self {
            system: System::new(),
        }
    }

    /// Every process's parent and resident memory, as of now.
    pub(crate) fn read(&mut self) -> HashMap<u32, ProcessEntry> {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );
        self.system
            .processes()
            .iter()
            .map(|(pid, process)| {
                (
                    pid.as_u32(),
                    ProcessEntry {
                        parent: process.parent().map(Pid::as_u32),
                        bytes: process.memory(),
                    },
                )
            })
            .collect()
    }
}

/// The terminals to measure: every live PTY, whichever project it is in.
///
/// Reads the workspace under its lock and nothing else; resolving pids can
/// run subprocesses (tmux), so that happens in [`terminal_roots`], off it.
fn project_terminals(workspace: &Mutex<Workspace>) -> HashMap<String, Vec<String>> {
    let ws = workspace.lock();
    ws.projects()
        .iter()
        .map(|p| (p.id.clone(), ws.all_terminal_ids_for_project(&p.id)))
        .collect()
}

/// Where each live terminal's tree starts: the PTY's own child, and the
/// shell a session backend (dtach, tmux) keeps behind it.
fn terminal_roots(pty_manager: &PtyManager) -> HashMap<String, Vec<u32>> {
    let ids = pty_manager.terminal_ids();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut roots = pty_manager.get_batch_service_pids(&refs);
    for id in &ids {
        let entry = roots.entry(id.clone()).or_default();
        if let Some(pid) = pty_manager.get_shell_pid(id)
            && !entry.contains(&pid)
        {
            entry.push(pid);
        }
    }
    roots
}

/// Measure every [`POLL_INTERVAL`] and publish into `memory_tx`, until the
/// daemon shuts down. The remote server attaches the latest figures to each
/// `SystemStatsChanged` it sends.
pub async fn run_memory_poll(
    workspace: Arc<Mutex<Workspace>>,
    pty_manager: Arc<PtyManager>,
    memory_tx: Arc<watch::Sender<Option<ApiProcessMemory>>>,
) {
    let table = Arc::new(Mutex::new(ProcessTable::new()));
    let daemon_pid = std::process::id();
    loop {
        let project_terminals = project_terminals(&workspace);
        let pty_manager = pty_manager.clone();
        let table = table.clone();
        let measured = tokio::task::spawn_blocking(move || {
            let targets = MemoryTargets {
                terminal_roots: terminal_roots(&pty_manager),
                project_terminals,
            };
            let processes = table.lock().read();
            measure(&processes, &targets, daemon_pid)
        })
        .await;
        match measured {
            Ok(memory) => {
                memory_tx.send_replace(Some(memory));
            }
            Err(e) => log::warn!("memory poll failed: {e}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(rows: &[(u32, Option<u32>, u64)]) -> HashMap<u32, ProcessEntry> {
        rows.iter()
            .map(|&(pid, parent, bytes)| (pid, ProcessEntry { parent, bytes }))
            .collect()
    }

    fn targets(terminals: &[(&str, &[u32])], projects: &[(&str, &[&str])]) -> MemoryTargets {
        MemoryTargets {
            terminal_roots: terminals
                .iter()
                .map(|(id, roots)| (id.to_string(), roots.to_vec()))
                .collect(),
            project_terminals: projects
                .iter()
                .map(|(id, ts)| (id.to_string(), ts.iter().map(|t| t.to_string()).collect()))
                .collect(),
        }
    }

    const DAEMON: u32 = 10;

    #[test]
    fn a_project_sums_every_terminal_tree_it_owns() {
        // Daemon 10 → shells 20 and 30. Shell 20 runs agent 21, which runs 22.
        // Shell 30 is a split beside it running 31.
        let t = table(&[
            (1, None, 5),
            (DAEMON, Some(1), 1000),
            (20, Some(DAEMON), 1),
            (21, Some(20), 100),
            (22, Some(21), 50),
            (30, Some(DAEMON), 2),
            (31, Some(30), 400),
        ]);
        let m = measure(
            &t,
            &targets(
                &[("agent", &[20]), ("split", &[30])],
                &[("p", &["agent", "split"])],
            ),
            DAEMON,
        );
        assert_eq!(m.projects["p"], 1 + 100 + 50 + 2 + 400);
        assert_eq!(m.terminals_bytes, 553);
        assert_eq!(m.daemon_bytes, 1000, "the daemon alone, not its children");
    }

    #[test]
    fn projects_differ_as_their_processes_do() {
        let t = table(&[
            (DAEMON, None, 1000),
            (20, Some(DAEMON), 300),
            (30, Some(DAEMON), 700),
        ]);
        let m = measure(
            &t,
            &targets(
                &[("a", &[20]), ("b", &[30])],
                &[("pa", &["a"]), ("pb", &["b"])],
            ),
            DAEMON,
        );
        assert_eq!(m.projects["pa"], 300);
        assert_eq!(m.projects["pb"], 700);
        assert_eq!(m.terminals_bytes, 1000);
    }

    #[test]
    fn a_process_two_roots_reach_counts_once() {
        // A session backend: attach 20 under the daemon, and the shell 40 it
        // reaches under the backend's server 39, which is not the daemon's.
        let t = table(&[
            (DAEMON, None, 1000),
            (20, Some(DAEMON), 3),
            (39, None, 9),
            (40, Some(39), 200),
            (41, Some(40), 60),
        ]);
        let m = measure(
            &t,
            &targets(&[("t", &[20, 40, 41])], &[("p", &["t"])]),
            DAEMON,
        );
        assert_eq!(m.projects["p"], 3 + 200 + 60);
        assert_eq!(m.terminals_bytes, 263);
    }

    #[test]
    fn a_root_at_the_daemon_does_not_swallow_it() {
        let t = table(&[
            (DAEMON, None, 1000),
            (20, Some(DAEMON), 5),
            (30, Some(DAEMON), 7),
        ]);
        let m = measure(
            &t,
            &targets(&[("t", &[DAEMON, 20])], &[("p", &["t"])]),
            DAEMON,
        );
        assert_eq!(m.projects["p"], 5);
        assert_eq!(m.terminals_bytes, 5);
    }

    #[test]
    fn terminals_no_project_owns_still_count_in_the_total() {
        let t = table(&[
            (DAEMON, None, 1000),
            (20, Some(DAEMON), 5),
            (30, Some(DAEMON), 7),
        ]);
        let m = measure(
            &t,
            &targets(
                &[("owned", &[20]), ("service", &[30])],
                &[("p", &["owned"])],
            ),
            DAEMON,
        );
        assert_eq!(m.terminals_bytes, 12);
        assert_eq!(m.projects["p"], 5);
    }

    #[test]
    fn a_project_with_nothing_running_is_left_out() {
        let t = table(&[(DAEMON, None, 1000), (20, Some(DAEMON), 5)]);
        let m = measure(
            &t,
            // "gone" names a pid that has exited; "idle" has no terminal at all.
            &targets(
                &[("t", &[20]), ("gone", &[99])],
                &[("p", &["t"]), ("stopped", &["gone"]), ("idle", &[])],
            ),
            DAEMON,
        );
        assert_eq!(m.projects.len(), 1);
        assert!(m.projects.contains_key("p"));
    }

    /// Pins the sysinfo contract the poll depends on: a full refresh with only
    /// memory asked for still fills in parents, so a real child and grandchild
    /// are found under this process and their memory counted.
    #[cfg(unix)]
    #[test]
    fn a_real_process_tree_is_measured() {
        let mut child = std::process::Command::new("sh")
            .args(["-c", "sleep 30 & wait"])
            .spawn()
            .expect("spawn sh");
        let shell = child.id();
        let mut processes = ProcessTable::new();
        // Give sh a moment to fork its sleep.
        let mut grandchild = None;
        for _ in 0..50 {
            let t = processes.read();
            grandchild = t
                .iter()
                .find(|(_, e)| e.parent == Some(shell))
                .map(|(pid, _)| *pid);
            if grandchild.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let t = processes.read();
        if let Some(pid) = grandchild {
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .status();
        }
        let _ = child.kill();
        let _ = child.wait();
        let grandchild = grandchild.expect("sysinfo should see sh's sleep with its parent");

        let m = measure(
            &t,
            &targets(&[("t", &[shell])], &[("p", &["t"])]),
            std::process::id(),
        );
        assert!(t[&shell].bytes > 0 && t[&grandchild].bytes > 0);
        assert_eq!(m.projects["p"], t[&shell].bytes + t[&grandchild].bytes);
        assert!(
            m.daemon_bytes > 0,
            "this test process stands in for the daemon"
        );
    }
}
