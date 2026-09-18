//! The projects overview as an open canvas.
//!
//! Every project the overview shows is a card with its map on it: areas as
//! boxes, the concepts that live in each. Links between projects are drawn
//! from the area that uses an interface to the area that serves it, and from
//! the card itself when a map does not say which area. Cards are placed
//! automatically so linked projects sit together, and can be dragged; the
//! canvas pans and zooms. Card positions and the view are kept per window.
//!
//! The maps and links come from the local daemon, read every few seconds, so
//! a scan's result shows up without reopening anything.

mod model;
mod render;

use crate::workspace::focus::FocusManager;
use crate::workspace::state::{CanvasPoint, CanvasViewport, WindowId, Workspace};
use gpui::*;
use model::{CanvasEdge, View};
use okena_core::api::ActionRequest;
use okena_core::project_map::{ProjectLinks, ProjectMap, ProjectMapReport};
use okena_transport::remote_action::RemoteActionClient;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::time::Duration;

/// How often the canvas reads the maps and links again.
const POLL: Duration = Duration::from_secs(3);
/// How long a pan or zoom has to settle before the view is saved.
const SAVE_VIEW_AFTER: Duration = Duration::from_millis(600);
/// How far the pointer moves before a press on a card is a drag, not a click.
const DRAG_THRESHOLD: f32 = 3.0;

/// Each card's client id and origin, in canvas units.
type CardOrigins = Vec<(String, (f32, f32))>;

/// What the pointer is doing with the canvas.
enum Drag {
    None,
    /// Moving the whole canvas.
    Pan {
        from: (f32, f32),
        start: View,
    },
    /// Moving one card. `at` is where it is now, in canvas units.
    Card {
        project: String,
        from: (f32, f32),
        start: (f32, f32),
        at: (f32, f32),
        moved: bool,
        shift: bool,
    },
}

pub struct ProjectCanvas {
    client: RemoteActionClient,
    workspace: Entity<Workspace>,
    focus_manager: Entity<FocusManager>,
    window_id: WindowId,
    /// The projects on the canvas: client ids, in the overview's order.
    projects: Vec<String>,
    /// Each project's map as last read, by client id. A project the daemon
    /// could not read has no entry.
    maps: HashMap<String, ProjectMapReport>,
    links: Option<ProjectLinks>,
    reading: bool,
    /// The pan and zoom. `None` until restored or fitted.
    view: Option<View>,
    /// Where the canvas is on screen, captured as it paints.
    bounds: Rc<RefCell<Bounds<Pixels>>>,
    drag: Drag,
    /// Selected cards, by client id.
    selected: BTreeSet<String>,
    default_agent: Option<String>,
    links_starting: bool,
    notice: Option<String>,
    error: Option<String>,
    save_view: Option<Task<()>>,
}

impl ProjectCanvas {
    pub fn new(
        client: RemoteActionClient,
        workspace: Entity<Workspace>,
        focus_manager: Entity<FocusManager>,
        window_id: WindowId,
        cx: &mut Context<Self>,
    ) -> Self {
        let canvas = Self {
            client,
            workspace,
            focus_manager,
            window_id,
            projects: Vec::new(),
            maps: HashMap::new(),
            links: None,
            reading: false,
            view: None,
            bounds: Rc::new(RefCell::new(Bounds::default())),
            drag: Drag::None,
            selected: BTreeSet::new(),
            default_agent: None,
            links_starting: false,
            notice: None,
            error: None,
            save_view: None,
        };
        canvas.refresh_default_agent(cx);
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                smol::Timer::after(POLL).await;
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break; // The canvas was dropped.
                }
            }
        })
        .detach();
        canvas
    }

    /// The projects the overview shows, client ids in its order. The window
    /// calls this as it renders; the maps are read again when the set changes.
    pub fn set_projects(&mut self, projects: Vec<String>, cx: &mut Context<Self>) {
        if self.projects == projects {
            return;
        }
        self.projects = projects;
        let projects = &self.projects;
        self.selected.retain(|id| projects.contains(id));
        self.refresh(cx);
    }

    fn daemon_id(&self, client_id: &str) -> String {
        okena_transport::client::strip_prefix(client_id, self.client.connection_id())
    }

    /// Read the links and every project's map from the daemon.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.reading || self.projects.is_empty() {
            return;
        }
        self.reading = true;
        let client = self.client.clone();
        let ids: Vec<(String, String)> = self
            .projects
            .iter()
            .map(|c| (c.clone(), self.daemon_id(c)))
            .collect();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                let links = client
                    .post_action(ActionRequest::ProjectLinks)
                    .and_then(|v| v.ok_or_else(|| "Missing project links".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<ProjectLinks>(v).map_err(|e| e.to_string())
                    })?;
                let mut maps = HashMap::new();
                for (client_id, daemon_id) in ids {
                    // A project this daemon cannot read — one on another
                    // connection — simply has no map on the canvas.
                    if let Ok(Some(v)) = client.post_action(ActionRequest::ProjectMapRead {
                        project_id: daemon_id,
                    }) && let Ok(report) = serde_json::from_value::<ProjectMapReport>(v)
                    {
                        maps.insert(client_id, report);
                    }
                }
                Ok::<_, String>((links, maps))
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.reading = false;
                    match result {
                        Ok((links, maps)) => {
                            if this.links.as_ref() != Some(&links) || this.maps != maps {
                                this.links = Some(links);
                                this.maps = maps;
                                cx.notify();
                            }
                        }
                        Err(e) => log::warn!("[canvas] could not read project maps: {e}"),
                    }
                });
            });
        })
        .detach();
    }

    /// Read the daemon's configured agent so the links launcher can mark it.
    fn refresh_default_agent(&self, cx: &mut Context<Self>) {
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
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    // ─── Layout ─────────────────────────────────────────────────────────────

    fn map_of(&self, client_id: &str) -> Option<&ProjectMap> {
        self.maps.get(client_id).and_then(|r| r.state.map())
    }

    fn area_count(&self, client_id: &str) -> usize {
        self.map_of(client_id).map_or(0, |m| m.areas.len())
    }

    /// Every card's origin in canvas units, and the edges between cards, by
    /// client id: a card being dragged where it is dragged, a card placed by
    /// hand where it was put, the rest where the automatic layout puts them.
    fn layout(&self, cx: &App) -> (CardOrigins, Vec<CanvasEdge>) {
        let by_daemon: HashMap<String, &String> = self
            .projects
            .iter()
            .map(|c| (self.daemon_id(c), c))
            .collect();
        let maps: HashMap<String, ProjectMap> = self
            .projects
            .iter()
            .filter_map(|c| self.map_of(c).map(|m| (self.daemon_id(c), m.clone())))
            .collect();
        let edges: Vec<CanvasEdge> = self
            .links
            .as_ref()
            .map(|links| model::canvas_edges(links, &maps))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|mut edge| {
                edge.consumer = (*by_daemon.get(&edge.consumer)?).clone();
                edge.provider = (*by_daemon.get(&edge.provider)?).clone();
                Some(edge)
            })
            .collect();

        let heights: Vec<(String, f32)> = self
            .projects
            .iter()
            .map(|c| (c.clone(), model::card_height(self.area_count(c))))
            .collect();
        let auto = model::auto_layout(&heights, &edges);
        let saved = self.workspace.read(cx).canvas_positions(self.window_id);
        let dragged = match &self.drag {
            Drag::Card {
                project,
                at,
                moved: true,
                ..
            } => Some((project.as_str(), *at)),
            _ => None,
        };
        let cards = self
            .projects
            .iter()
            .map(|c| {
                let origin = dragged
                    .filter(|(p, _)| *p == c.as_str())
                    .map(|(_, at)| at)
                    .or_else(|| saved.get(c).map(|p| (p.x, p.y)))
                    .or_else(|| auto.get(c).copied())
                    .unwrap_or((0.0, 0.0));
                (c.clone(), origin)
            })
            .collect();
        (cards, edges)
    }

    // ─── Viewport ───────────────────────────────────────────────────────────

    /// A window position, relative to the canvas's top-left corner.
    fn local(&self, position: Point<Pixels>) -> (f32, f32) {
        let bounds = self.bounds.borrow();
        (
            f32::from(position.x - bounds.origin.x),
            f32::from(position.y - bounds.origin.y),
        )
    }

    fn size(&self) -> (f32, f32) {
        let bounds = self.bounds.borrow();
        (f32::from(bounds.size.width), f32::from(bounds.size.height))
    }

    /// Save the view once panning or zooming has settled.
    fn schedule_save_view(&mut self, cx: &mut Context<Self>) {
        self.save_view = Some(cx.spawn(async move |this, cx| {
            smol::Timer::after(SAVE_VIEW_AFTER).await;
            let _ = this.update(cx, |this, cx| {
                let window_id = this.window_id;
                let viewport = this.view.map(|v| CanvasViewport {
                    x: v.x,
                    y: v.y,
                    zoom: v.zoom,
                });
                this.workspace
                    .update(cx, |ws, cx| ws.set_canvas_viewport(window_id, viewport, cx));
            });
        }));
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(17.0));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        let view = self.view.unwrap_or_default();
        let next = if event.modifiers.platform || event.modifiers.control {
            let factor = (1.0 + dy * 0.01).clamp(0.5, 2.0);
            view.zoomed_at(factor, self.local(event.position))
        } else {
            View {
                x: view.x + dx,
                y: view.y + dy,
                ..view
            }
        };
        self.view = Some(next);
        self.schedule_save_view(cx);
        cx.notify();
    }

    /// Fit every card on screen.
    fn fit(&mut self, cx: &mut Context<Self>) {
        let (cards, _) = self.layout(cx);
        let content = model::content_bounds(
            cards
                .iter()
                .map(|(id, origin)| model::card_rect(*origin, self.area_count(id))),
        );
        if let Some(content) = content {
            self.view = Some(View::fit(content, self.size()));
            self.schedule_save_view(cx);
            cx.notify();
        }
    }

    /// Hand every card back to the automatic layout, and fit it.
    fn auto_layout(&mut self, cx: &mut Context<Self>) {
        let window_id = self.window_id;
        self.workspace
            .update(cx, |ws, cx| ws.clear_canvas_positions(window_id, cx));
        self.fit(cx);
    }

    // ─── Pointer ────────────────────────────────────────────────────────────

    fn background_down(&mut self, position: Point<Pixels>, shift: bool, cx: &mut Context<Self>) {
        if !shift && !self.selected.is_empty() {
            self.selected.clear();
            cx.notify();
        }
        self.drag = Drag::Pan {
            from: self.local(position),
            start: self.view.unwrap_or_default(),
        };
    }

    fn card_down(
        &mut self,
        project: String,
        origin: (f32, f32),
        position: Point<Pixels>,
        shift: bool,
    ) {
        self.drag = Drag::Card {
            project,
            from: self.local(position),
            start: origin,
            at: origin,
            moved: false,
            shift,
        };
    }

    fn drag_to(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let now = self.local(position);
        let zoom = self.view.map_or(1.0, |v| v.zoom);
        match &mut self.drag {
            Drag::None => {}
            Drag::Pan { from, start } => {
                self.view = Some(View {
                    x: start.x + now.0 - from.0,
                    y: start.y + now.1 - from.1,
                    zoom: start.zoom,
                });
                cx.notify();
            }
            Drag::Card {
                from,
                start,
                at,
                moved,
                ..
            } => {
                let (dx, dy) = (now.0 - from.0, now.1 - from.1);
                if !*moved && dx.abs().max(dy.abs()) < DRAG_THRESHOLD {
                    return;
                }
                *moved = true;
                *at = (start.0 + dx / zoom, start.1 + dy / zoom);
                cx.notify();
            }
        }
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::None => return,
            Drag::Pan { .. } => self.schedule_save_view(cx),
            Drag::Card {
                project,
                at,
                moved: true,
                ..
            } => {
                let window_id = self.window_id;
                self.workspace.update(cx, |ws, cx| {
                    ws.set_canvas_position(
                        window_id,
                        &project,
                        CanvasPoint { x: at.0, y: at.1 },
                        cx,
                    )
                });
            }
            // A press that did not move is a click: select the card, or add
            // it to the selection with shift.
            Drag::Card { project, shift, .. } => {
                if shift {
                    if !self.selected.remove(&project) {
                        self.selected.insert(project);
                    }
                } else {
                    self.selected = BTreeSet::from([project]);
                }
            }
        }
        cx.notify();
    }

    /// Open a project the usual way: focused, in its column.
    fn open_project(&mut self, client_id: &str, cx: &mut Context<Self>) {
        crate::views::components::project_nav::focus_project(
            &self.workspace,
            &self.focus_manager,
            self.window_id,
            client_id,
            cx,
        );
    }

    /// Start one agent looking for links between the selected projects.
    fn start_links_scan(&mut self, agent: String, model: Option<String>, cx: &mut Context<Self>) {
        let project_ids: Vec<String> = self.selected.iter().map(|c| self.daemon_id(c)).collect();
        if project_ids.len() < 2 || self.links_starting {
            return;
        }
        self.links_starting = true;
        self.notice = None;
        self.error = None;
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
                    match result {
                        Ok(v) => {
                            let name = v
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("the session");
                            this.notice = Some(format!("Started {name} — it is in the sidebar."));
                        }
                        Err(e) => this.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
}
