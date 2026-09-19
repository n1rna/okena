//! Memory the daemons report for their terminals, shared across views.
//!
//! Each connected daemon measures its own terminals and sends the figures with
//! its system stats. The remote manager files them here, per connection; the
//! status bar, the agent panel and the sidebar read them. Like the harness
//! state it lives in `okena-workspace`, which all three can see.

use gpui::*;
use okena_core::api::ApiProcessMemory;
use std::collections::HashMap;

/// The latest figures from every connected daemon, by connection id.
#[derive(Default)]
pub struct ProcessMemory {
    by_connection: HashMap<String, ApiProcessMemory>,
}

impl ProcessMemory {
    /// What `connection_id`'s daemon last reported, if anything.
    pub fn connection(&self, connection_id: &str) -> Option<&ApiProcessMemory> {
        self.by_connection.get(connection_id)
    }

    /// Memory held by one project's terminals, by the prefixed id the client
    /// knows it by (`remote:{connection}:{project}`). `None` when nothing in
    /// it is running, or its daemon does not measure memory.
    pub fn project(&self, project_id: &str) -> Option<u64> {
        let rest = project_id.strip_prefix("remote:")?;
        self.by_connection.iter().find_map(|(connection, memory)| {
            let id = rest.strip_prefix(connection.as_str())?.strip_prefix(':')?;
            memory.projects.get(id).copied()
        })
    }

    /// Replace `connection_id`'s figures; `None` forgets them.
    ///
    /// Observers are notified only when a project's figure changes as it is
    /// shown, so a byte-level wobble every few seconds does not re-render the
    /// sidebar and every agent panel.
    pub fn set(
        &mut self,
        connection_id: &str,
        memory: Option<ApiProcessMemory>,
        cx: &mut Context<Self>,
    ) {
        if self.replace(connection_id, memory) {
            cx.notify();
        }
    }

    /// [`set`](Self::set) without the context: whether a shown figure changed.
    fn replace(&mut self, connection_id: &str, memory: Option<ApiProcessMemory>) -> bool {
        let shown = |m: Option<&ApiProcessMemory>| -> HashMap<String, String> {
            m.map(|m| {
                m.projects
                    .iter()
                    .map(|(id, bytes)| (id.clone(), format_memory(*bytes)))
                    .collect()
            })
            .unwrap_or_default()
        };
        let before = shown(self.by_connection.get(connection_id));
        let after = shown(memory.as_ref());
        match memory {
            Some(memory) => {
                self.by_connection.insert(connection_id.to_string(), memory);
            }
            None => {
                self.by_connection.remove(connection_id);
            }
        }
        before != after
    }
}

/// Memory as the UI shows it: whole megabytes up to a gigabyte, then gigabytes
/// to one decimal.
pub fn format_memory(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    let mb = (bytes + MIB / 2) / MIB;
    if mb < 1024 {
        format!("{mb} MB")
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * MIB as f64))
    }
}

/// Global handle to the shared [`ProcessMemory`].
pub struct GlobalProcessMemory(pub Entity<ProcessMemory>);

impl Global for GlobalProcessMemory {}

/// Handle to the shared state, when it has been registered.
pub fn process_memory_entity(cx: &App) -> Option<Entity<ProcessMemory>> {
    cx.try_global::<GlobalProcessMemory>().map(|g| g.0.clone())
}

/// Memory held by `project_id`'s terminals, as [`ProcessMemory::project`].
pub fn project_memory(project_id: &str, cx: &App) -> Option<u64> {
    process_memory_entity(cx).and_then(|m| m.read(cx).project(project_id))
}

#[cfg(test)]
mod tests {
    use super::{ProcessMemory, format_memory};
    use okena_core::api::ApiProcessMemory;

    fn memory(projects: &[(&str, u64)]) -> ApiProcessMemory {
        ApiProcessMemory {
            daemon_bytes: 1,
            terminals_bytes: 2,
            projects: projects
                .iter()
                .map(|(id, b)| (id.to_string(), *b))
                .collect(),
        }
    }

    const MB: u64 = 1024 * 1024;

    #[test]
    fn formats_megabytes_then_gigabytes() {
        assert_eq!(format_memory(0), "0 MB");
        assert_eq!(format_memory(412 * MB + 100), "412 MB");
        assert_eq!(format_memory(1023 * MB), "1023 MB");
        assert_eq!(format_memory(1024 * MB), "1.0 GB");
        assert_eq!(format_memory(1536 * MB), "1.5 GB");
    }

    #[test]
    fn a_project_is_found_under_its_own_connection() {
        let mut m = ProcessMemory::default();
        m.replace("local", Some(memory(&[("p1", 5 * MB)])));
        m.replace("box:2", Some(memory(&[("p1", 7 * MB)])));
        assert_eq!(m.project("remote:local:p1"), Some(5 * MB));
        assert_eq!(m.project("remote:box:2:p1"), Some(7 * MB));
        assert_eq!(m.project("remote:local:p2"), None, "nothing running in it");
        assert_eq!(m.project("p1"), None, "not a client id");
        assert_eq!(
            m.project("remote:loc:p1"),
            None,
            "a connection id prefix is not a match"
        );
    }

    #[test]
    fn only_a_shown_change_counts() {
        let mut m = ProcessMemory::default();
        assert!(m.replace("local", Some(memory(&[("p1", 5 * MB)]))));
        assert!(
            !m.replace("local", Some(memory(&[("p1", 5 * MB + 1000)]))),
            "same MB"
        );
        assert!(m.replace("local", Some(memory(&[("p1", 9 * MB)]))));
        assert!(m.replace("local", Some(memory(&[]))), "the agent stopped");
        assert!(!m.replace("local", None), "nothing shown before or after");
    }
}
