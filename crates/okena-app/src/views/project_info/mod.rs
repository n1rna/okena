//! The project info panel — what okena knows about a repo or worktree, shown
//! beside its terminal.
//!
//! The project counterpart to the agent-session panel, hosted the same way: a
//! project column swaps it in behind the header's info toggle, or when the
//! overview-wide switch is on. It replaces the harness Projects view, which
//! showed these facts in lanes of their own — away from the terminals they
//! were about, and in a second place a project had to be looked for.

mod model;
mod render;

pub use model::{GitFacts, ProjectInfo, ProjectInfoKind};

use crate::views::agent_session::InfoPanelContext;
use crate::workspace::focus::FocusManager;
use crate::workspace::state::{WindowId, Workspace};
use gpui::*;
use okena_core::api::ActionRequest;
use okena_core::project_map::{ProjectLinks, ProjectMapReport};
use okena_terminal::TerminalsRegistry;
use std::time::Duration;

/// How often an open panel re-reads its project's map. A scan writes the
/// manifest from an agent's terminal, which okena is not told about, so the
/// panel looks again rather than waiting to be reopened.
const MAP_POLL: Duration = Duration::from_secs(3);

pub struct ProjectInfoPanel {
    client: okena_transport::remote_action::RemoteActionClient,
    request_broker: Entity<okena_workspace::request_broker::RequestBroker>,
    workspace: Entity<Workspace>,
    focus_manager: Entity<FocusManager>,
    window_id: WindowId,
    /// Read for whether the listed sessions are running, which the workspace
    /// mirror knows the layout of but not the life inside.
    terminals: TerminalsRegistry,
    /// The project this panel describes.
    project_id: String,
    /// The project's map, as the daemon last read it. `None` until the first
    /// read answers.
    map: Option<ProjectMapReport>,
    /// A map read on its way, so a slow daemon does not stack polls.
    map_reading: bool,
    /// The daemon's configured agent, drawn as the launcher's default.
    default_agent: Option<String>,
    /// A scan on its way, so a double click does not start two.
    scan_starting: bool,
    scan_error: Option<String>,
    /// What the last scan start did.
    scan_notice: Option<String>,
    /// Links across every scanned project, as the daemon last matched them.
    links: Option<ProjectLinks>,
    links_reading: bool,
    /// A links scan on its way.
    links_starting: bool,
    links_error: Option<String>,
    links_notice: Option<String>,
}

impl ProjectInfoPanel {
    pub fn new(project_id: String, ctx: InfoPanelContext, cx: &mut Context<Self>) -> Self {
        let mut panel = Self {
            client: ctx.client,
            request_broker: ctx.request_broker,
            workspace: ctx.workspace,
            focus_manager: ctx.focus_manager,
            window_id: ctx.window_id,
            terminals: ctx.terminals,
            project_id,
            map: None,
            map_reading: false,
            default_agent: None,
            scan_starting: false,
            scan_error: None,
            scan_notice: None,
            links: None,
            links_reading: false,
            links_starting: false,
            links_error: None,
            links_notice: None,
        };
        panel.refresh_map(cx);
        panel.refresh_links(cx);
        panel.refresh_default_agent(cx);
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                smol::Timer::after(MAP_POLL).await;
                let polled = this.update(cx, |this, cx| {
                    this.refresh_map(cx);
                    this.refresh_links(cx);
                });
                if polled.is_err() {
                    break; // The panel was dropped.
                }
            }
        })
        .detach();
        panel
    }

    /// The project's id as the daemon knows it.
    fn daemon_project_id(&self) -> String {
        okena_transport::client::strip_prefix(&self.project_id, self.client.connection_id())
    }

    /// Read the project's map from the daemon, which reads the checkout — so a
    /// remote project's map reads the same as a local one's.
    fn refresh_map(&mut self, cx: &mut Context<Self>) {
        if self.map_reading {
            return;
        }
        self.map_reading = true;
        let client = self.client.clone();
        let project_id = self.daemon_project_id();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::ProjectMapRead { project_id })
                    .and_then(|v| v.ok_or_else(|| "Missing project map".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<ProjectMapReport>(v).map_err(|e| e.to_string())
                    })
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.map_reading = false;
                    match result {
                        // Polled every few seconds: only a change re-renders.
                        Ok(report) if this.map.as_ref() != Some(&report) => {
                            this.map = Some(report);
                            cx.notify();
                        }
                        Ok(_) => {}
                        Err(e) => log::warn!("[project-info] could not read the project map: {e}"),
                    }
                });
            });
        })
        .detach();
    }

    /// Read the links across every scanned project. One project's panel shows
    /// its own side, but a link needs both maps to be matched.
    fn refresh_links(&mut self, cx: &mut Context<Self>) {
        if self.links_reading {
            return;
        }
        self.links_reading = true;
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::ProjectLinks)
                    .and_then(|v| v.ok_or_else(|| "Missing project links".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<ProjectLinks>(v).map_err(|e| e.to_string())
                    })
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.links_reading = false;
                    match result {
                        Ok(links) if this.links.as_ref() != Some(&links) => {
                            this.links = Some(links);
                            cx.notify();
                        }
                        Ok(_) => {}
                        Err(e) => log::warn!("[project-info] could not read project links: {e}"),
                    }
                });
            });
        })
        .detach();
    }

    /// Start one agent looking for links between `project_ids`, which are the
    /// daemon's own ids.
    fn start_links_scan(
        &mut self,
        agent: String,
        project_ids: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if self.links_starting {
            return;
        }
        self.links_starting = true;
        self.links_error = None;
        self.links_notice = None;
        cx.notify();

        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::ProjectsScan {
                        project_ids,
                        agent_command: Some(agent),
                    })
                    .and_then(|v| v.ok_or_else(|| "Missing links scan result".to_string()))
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.links_starting = false;
                    match result {
                        Ok(v) => {
                            let name = v
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("the session");
                            this.links_notice =
                                Some(format!("Started {name} — it is in the sidebar."));
                        }
                        Err(e) => this.links_error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Open one of the map's docs in Harness → Knowledge, in the root the map
    /// was read from.
    fn open_map_doc(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(root_key) = self.map.as_ref().and_then(|m| m.root_key.clone()) else {
            return;
        };
        self.request_broker.update(cx, |broker, cx| {
            broker.push_workbench_request(
                crate::workspace::requests::WorkbenchRequest::OpenKnowledgeDoc { root_key, path },
                cx,
            );
        });
    }

    /// Read the daemon's configured agent so the launcher can mark it.
    fn refresh_default_agent(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::GetSettings)
                    .and_then(|v| v.ok_or_else(|| "Missing settings".to_string()))
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if let Ok(v) = result {
                        this.default_agent = v
                            .get("harness")
                            .and_then(|h| h.get("agent_command"))
                            .and_then(|c| c.as_str())
                            .filter(|c| !c.trim().is_empty())
                            .map(str::to_string);
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Start an agent mapping this repository.
    fn start_scan(&mut self, agent: String, cx: &mut Context<Self>) {
        if self.scan_starting {
            return;
        }
        self.scan_starting = true;
        self.scan_error = None;
        self.scan_notice = None;
        cx.notify();

        let client = self.client.clone();
        let project_id = self.daemon_project_id();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::ProjectScan {
                        project_id,
                        agent_command: Some(agent),
                    })
                    .and_then(|v| v.ok_or_else(|| "Missing scan result".to_string()))
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.scan_starting = false;
                    match result {
                        Ok(v) => {
                            let name = v
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("the session");
                            this.scan_notice =
                                Some(format!("Started {name} — it is in the sidebar."));
                        }
                        Err(e) => this.scan_error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Everything shown, read fresh from the workspace mirror each frame.
    fn info(&self, cx: &App) -> Option<ProjectInfo> {
        ProjectInfo::collect(self.workspace.read(cx), &self.project_id)
    }

    /// Focus a worktree or session, leaving any harness view.
    fn open_project(&mut self, project_id: String, cx: &mut Context<Self>) {
        crate::views::components::project_nav::focus_project(
            &self.workspace,
            &self.focus_manager,
            self.window_id,
            &project_id,
            cx,
        );
        cx.notify();
    }

    /// Show what changed in a checkout.
    fn open_diff(&mut self, project_id: &str, cx: &mut Context<Self>) {
        crate::views::components::project_nav::open_diff(&self.request_broker, project_id, cx);
    }
}
