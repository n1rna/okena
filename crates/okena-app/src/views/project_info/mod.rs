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

use self::model::{
    MenuEntry, MenuPick, RootSection, StoreChip, entry_matches, locate_file, menu_entries,
    store_chips,
};
use crate::workspace::requests::WorkbenchRequest;
use crate::views::agent_session::InfoPanelContext;
use crate::workspace::focus::FocusManager;
use crate::workspace::state::{WindowId, Workspace};
use gpui::*;
use okena_core::api::ActionRequest;
use okena_core::context::{ContextItem, ContextSearchResult};
use okena_core::knowledge::KnowledgeStores;
use okena_core::project_map::{ProjectLinks, ProjectMapReport};
use okena_core::specs::SpecStores;
use okena_terminal::TerminalsRegistry;
use okena_ui::chip_search::{ChipGroup, ChipItem, ChipSearch, ChipSearchEvent};
use std::time::Duration;

/// How often an open panel re-reads its project's map. A scan writes the
/// manifest from an agent's terminal, which okena is not told about, so the
/// panel looks again rather than waiting to be reopened.
const MAP_POLL: Duration = Duration::from_secs(3);

/// Most items the menu asks the daemon for. Everything one project owns fits
/// well inside the daemon's own ceiling.
const MENU_LIMIT: usize = 500;

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
    ///
    /// Nothing is said when one starts: its session is listed by the launcher
    /// that started it, like every other agent. A start that fails is a toast,
    /// where the rest of okena puts what went wrong.
    scan_starting: bool,
    /// Links across every scanned project, as the daemon last matched them.
    links: Option<ProjectLinks>,
    links_reading: bool,
    /// A links scan on its way.
    links_starting: bool,
    /// Everything the project owns, behind the MAP section's button.
    menu: Entity<ChipSearch>,
    /// What the menu lists, rebuilt when the context or the links change.
    entries: Vec<MenuEntry>,
    /// The project's context items, as the daemon last indexed them.
    context_items: Vec<ContextItem>,
    context_reading: bool,
    /// The knowledge roots on this machine: which stores the project follows,
    /// and where a picked doc, skill or agent opens.
    stores: Option<KnowledgeStores>,
    /// The spec roots, for opening a picked spec in the root holding it.
    spec_roots: Vec<(String, String)>,
    _subscriptions: Vec<Subscription>,
}

impl ProjectInfoPanel {
    pub fn new(project_id: String, ctx: InfoPanelContext, cx: &mut Context<Self>) -> Self {
        // The launcher's Context menu, over one project and opening what it
        // picks instead of collecting chips.
        let menu = cx.new(|cx| {
            ChipSearch::new(
                format!("project-info-menu-{project_id}"),
                "Search this project's map, specs, knowledge and links",
                cx,
            )
            .menu("Browse")
            .empty_text("Nothing matches.")
        });
        let subscriptions = vec![cx.subscribe(
            &menu,
            |this, _, event: &ChipSearchEvent, cx| match event {
                ChipSearchEvent::QueryChanged(query) => {
                    // An empty box is the menu opening, or a pick having
                    // cleared it: read what the project owns again, so a scan
                    // that has since finished shows.
                    if query.is_empty() {
                        this.refresh_context(cx);
                    }
                    this.show_menu_results(query, cx);
                }
                ChipSearchEvent::Picked(item) => this.open_entry(&item.id.clone(), cx),
                ChipSearchEvent::Added(_)
                | ChipSearchEvent::Removed(_)
                | ChipSearchEvent::Hint(_) => {}
            },
        )];
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
            links: None,
            links_reading: false,
            links_starting: false,
            menu,
            entries: Vec::new(),
            context_items: Vec::new(),
            context_reading: false,
            stores: None,
            spec_roots: Vec::new(),
            _subscriptions: subscriptions,
        };
        panel.refresh_map(cx);
        panel.refresh_links(cx);
        panel.refresh_default_agent(cx);
        panel.refresh_context(cx);
        panel.refresh_roots(cx);
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

    /// The project whose map this panel shows: its own, or for a worktree the
    /// repository it was created from — the map is committed there, and a
    /// worktree is a second checkout of it. `None` when that repository is not
    /// a project okena knows, which is when neither section shows.
    fn map_project_id(&self, cx: &App) -> Option<String> {
        let ws = self.workspace.read(cx);
        let project = ws.project(&self.project_id)?;
        match &project.worktree_info {
            None => Some(project.id.clone()),
            Some(info) => ws.project(&info.parent_project_id).map(|p| p.id.clone()),
        }
    }

    /// The same, as the daemon names it.
    fn daemon_map_id(&self, cx: &App) -> Option<String> {
        self.map_project_id(cx)
            .map(|id| okena_transport::client::strip_prefix(&id, self.client.connection_id()))
    }

    /// Read everything the project owns from the daemon's context index: its
    /// map entries, specs, knowledge docs, skills and agents.
    fn refresh_context(&mut self, cx: &mut Context<Self>) {
        if self.context_reading {
            return;
        }
        let Some(project_id) = self.daemon_map_id(cx) else {
            return;
        };
        self.context_reading = true;
        let client = self.client.clone();
        let project_ids = vec![project_id];
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::ContextSearch {
                        // Everything, ranked by the daemon; the box narrows it
                        // here, so typing costs no round trip.
                        query: String::new(),
                        project_ids,
                        terminal_id: None,
                        limit: Some(MENU_LIMIT),
                    })
                    .and_then(|v| {
                        serde_json::from_value::<ContextSearchResult>(v.unwrap_or_default())
                            .map_err(|e| e.to_string())
                    })
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.context_reading = false;
                    match result {
                        Ok(found) => {
                            this.context_items = found.items;
                            this.rebuild_menu(cx);
                        }
                        Err(e) => {
                            log::warn!("[project-info] could not read the project's context: {e}");
                        }
                    }
                });
            });
        })
        .detach();
    }

    /// Read the knowledge and spec roots: which stores the project follows,
    /// and which root a picked file opens in.
    fn refresh_roots(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let (knowledge, specs) = smol::unblock(move || {
                let knowledge = client
                    .post_action(ActionRequest::KnowledgeStores)
                    .and_then(|v| {
                        serde_json::from_value::<KnowledgeStores>(v.unwrap_or_default())
                            .map_err(|e| e.to_string())
                    });
                let specs = client.post_action(ActionRequest::SpecStores).and_then(|v| {
                    serde_json::from_value::<SpecStores>(v.unwrap_or_default())
                        .map_err(|e| e.to_string())
                });
                (knowledge, specs)
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    match knowledge {
                        Ok(stores) => this.stores = Some(stores),
                        Err(e) => {
                            log::warn!("[project-info] could not read the knowledge stores: {e}");
                        }
                    }
                    match specs {
                        Ok(stores) => {
                            this.spec_roots = stores
                                .roots
                                .iter()
                                .map(|root| (root.key.clone(), root.path.clone()))
                                .collect();
                        }
                        Err(e) => log::warn!("[project-info] could not read the spec roots: {e}"),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Rebuild what the menu lists, after the context or the links changed.
    fn rebuild_menu(&mut self, cx: &mut Context<Self>) {
        let Some(me) = self.daemon_map_id(cx) else {
            return;
        };
        self.entries = menu_entries(&self.context_items, &me, self.links.as_ref());
        let label = format!("Browse · {}", self.entries.len());
        let query = self.menu.read(cx).query(cx);
        self.menu
            .update(cx, |menu, cx| menu.set_menu_label(label, cx));
        self.show_menu_results(&query, cx);
        cx.notify();
    }

    /// Put the rows matching `query` in the menu.
    fn show_menu_results(&mut self, query: &str, cx: &mut Context<Self>) {
        let results: Vec<ChipItem> = self
            .entries
            .iter()
            .filter(|entry| entry_matches(entry, query))
            .map(menu_item)
            .collect();
        self.menu
            .update(cx, |menu, cx| menu.set_results(results, cx));
    }

    /// Open what a menu row stands for. Picking opens: it never adds context
    /// or starts an agent.
    fn open_entry(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(pick) = self
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.pick.clone())
        else {
            return;
        };
        match pick {
            MenuPick::File(path) => self.open_file(&path, cx),
            MenuPick::Project(project) => self.open_project_info(project, cx),
            // A link naming nothing okena has: the row says so, and opens
            // nothing.
            MenuPick::Nothing => {}
        }
    }

    /// Open one of the project's files, in the harness section owning the
    /// root it lies in.
    fn open_file(&mut self, path: &str, cx: &mut Context<Self>) {
        let knowledge: Vec<(String, String)> = self
            .stores
            .iter()
            .flat_map(|stores| stores.roots.iter())
            .map(|root| (root.key.clone(), root.path.clone()))
            .collect();
        let Some(target) = locate_file(path, &knowledge, &self.spec_roots) else {
            log::warn!("[project-info] no knowledge or spec root holds {path}");
            return;
        };
        let request = match target.section {
            RootSection::Knowledge => WorkbenchRequest::OpenKnowledgeDoc {
                root_key: target.root_key,
                path: target.path,
            },
            RootSection::Specs => WorkbenchRequest::OpenSpecDoc {
                root_key: target.root_key,
                path: target.path,
            },
        };
        self.push_workbench(request, cx);
    }

    /// Open a launcher's brief: its own file in Harness → Knowledge.
    pub(super) fn open_brief(
        &self,
    ) -> impl Fn(&SharedString, &SharedString, &mut Window, &mut App) + 'static + use<> {
        let broker = self.request_broker.clone();
        move |root, path, _window, cx| {
            crate::views::launch_briefs::open_brief(&broker, root, path, cx);
        }
    }

    /// Open the project's `project-map.yaml`, invalid or not.
    fn open_manifest(&mut self, cx: &mut Context<Self>) {
        let Some(root_key) = self.map.as_ref().and_then(|m| m.root_key.clone()) else {
            return;
        };
        self.push_workbench(
            WorkbenchRequest::OpenKnowledgeDoc {
                root_key,
                path: render::MANIFEST_FILE.to_string(),
            },
            cx,
        );
    }

    /// Show Knowledge on a store the project follows.
    fn open_knowledge_root(&mut self, root_key: String, cx: &mut Context<Self>) {
        self.push_workbench(WorkbenchRequest::OpenKnowledgeRoot { root_key }, cx);
    }

    /// Show another project's info panel, by the daemon's id for it.
    fn open_project_info(&mut self, daemon_id: String, cx: &mut Context<Self>) {
        let connection = self.client.connection_id();
        let Some(project_id) = self
            .workspace
            .read(cx)
            .projects()
            .iter()
            .find(|p| okena_transport::client::strip_prefix(&p.id, connection) == daemon_id)
            .map(|p| p.id.clone())
        else {
            return;
        };
        self.push_workbench(WorkbenchRequest::ShowProjectInfo { project_id }, cx);
    }

    /// The stores the project follows, as the panel's chips.
    fn store_chips(&self, cx: &App) -> Vec<StoreChip> {
        let Some(stores) = self.stores.as_ref() else {
            return Vec::new();
        };
        let Some(path) = self
            .map_project_id(cx)
            .and_then(|id| self.workspace.read(cx).project(&id).map(|p| p.path.clone()))
        else {
            return Vec::new();
        };
        store_chips(stores, &path)
    }

    fn push_workbench(&mut self, request: WorkbenchRequest, cx: &mut Context<Self>) {
        self.request_broker.update(cx, |broker, cx| {
            broker.push_workbench_request(request, cx);
        });
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
        model: Option<String>,
        project_ids: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if self.links_starting {
            return;
        }
        self.links_starting = true;
        cx.notify();

        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::ProjectsScan {
                        project_ids,
                        agent_command: Some(agent),
                        model,
                    })
                    .and_then(|v| v.ok_or_else(|| "Missing links scan result".to_string()))
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.links_starting = false;
                    if let Err(e) = result {
                        crate::views::panels::toast::ToastManager::error(
                            format!("Could not scan for links: {e}"),
                            cx,
                        );
                    }
                    cx.notify();
                });
            });
        })
        .detach();
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
    fn start_scan(&mut self, agent: String, model: Option<String>, cx: &mut Context<Self>) {
        if self.scan_starting {
            return;
        }
        self.scan_starting = true;
        cx.notify();

        let client = self.client.clone();
        let project_id = self.daemon_project_id();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::ProjectScan {
                        project_id,
                        agent_command: Some(agent),
                        model,
                    })
                    .and_then(|v| v.ok_or_else(|| "Missing scan result".to_string()))
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.scan_starting = false;
                    if let Err(e) = result {
                        crate::views::panels::toast::ToastManager::error(
                            format!("Could not scan the project: {e}"),
                            cx,
                        );
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
    fn open_diff(
        &mut self,
        project_id: &str,
        mode: Option<okena_core::types::DiffMode>,
        cx: &mut Context<Self>,
    ) {
        crate::views::components::project_nav::open_diff(
            &self.request_broker,
            project_id,
            mode,
            cx,
        );
    }
}

/// One row of the menu: its group's icon and header, with the description
/// after the title — as the launcher's Context menu shows its results.
fn menu_item(entry: &MenuEntry) -> ChipItem {
    let mut item = ChipItem::new(entry.id.clone(), entry.title.clone())
        .kind(entry.group.name())
        .icon(entry.group.icon())
        .description(entry.description.clone())
        .group(ChipGroup {
            id: entry.group.id().into(),
            name: entry.group.name().into(),
            icon: Some(entry.group.icon().into()),
        });
    for tag in &entry.tags {
        item = item.tag(tag.clone());
    }
    item
}
