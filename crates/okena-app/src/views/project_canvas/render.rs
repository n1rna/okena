//! Drawing the canvas: edges on a paint layer, cards and labels as elements
//! over it, and a toolbar that does not move with the canvas.

use super::ProjectCanvas;
use super::model::{self, CARD_PADDING, CARD_WIDTH, HEADER_HEIGHT, View};
use crate::theme::theme;
use crate::ui::tokens::ui_text_ms;
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::project_map::{LinkSource, ProjectMapState};
use okena_core::theme::ThemeColors;

/// Below this zoom, edge labels are left out: they would only be noise.
const LABEL_MIN_ZOOM: f32 = 0.6;

/// One edge, ready to paint in canvas-relative screen space.
struct Stroke {
    curve: [(f32, f32); 4],
    color: u32,
    dashed: bool,
}

impl Render for ProjectCanvas {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let (cards, edges) = self.layout(cx);

        // The first view: the one this window last had, else everything
        // fitted once the canvas knows its size.
        if self.view.is_none() {
            let saved = self.workspace.read(cx).canvas_viewport(self.window_id);
            let size = self.size();
            if let Some(v) = saved {
                self.view = Some(View {
                    x: v.x,
                    y: v.y,
                    zoom: v.zoom,
                });
            } else if size.0 > 0.0
                && let Some(content) = model::content_bounds(
                    cards
                        .iter()
                        .map(|(id, origin)| model::card_rect(*origin, self.area_count(id))),
                )
            {
                self.view = Some(View::fit(content, size));
            }
        }
        let view = self.view.unwrap_or_default();

        let origin_of = |id: &str| cards.iter().find(|(c, _)| c == id).map(|(_, o)| *o);
        let mut strokes = Vec::new();
        let mut labels = Vec::new();
        for edge in &edges {
            let (Some(from_origin), Some(to_origin)) =
                (origin_of(&edge.consumer), origin_of(&edge.provider))
            else {
                continue;
            };
            let from = match edge.consumer_area {
                Some(i) => model::area_rect(from_origin, i),
                None => model::card_rect(from_origin, self.area_count(&edge.consumer)),
            };
            let to = match edge.provider_area {
                Some(i) => model::area_rect(to_origin, i),
                None => model::card_rect(to_origin, self.area_count(&edge.provider)),
            };
            let curve = model::edge_curve(view.rect_to_screen(from), view.rect_to_screen(to));
            let color = source_color(edge.source, &t);
            strokes.push(Stroke {
                curve,
                color,
                dashed: edge.one_sided,
            });
            labels.push((model::curve_midpoint(curve), edge.label(), color));
        }

        let bounds = self.bounds.clone();
        let edge_layer = canvas(
            move |captured, _window, _cx| {
                *bounds.borrow_mut() = captured;
            },
            move |painted, _, window, _cx| {
                for stroke in &strokes {
                    paint_stroke(window, painted.origin, stroke);
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let mut layer = div()
            .id("project-canvas")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(t.bg_primary))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.background_down(event.position, event.modifiers.shift, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                    this.drag_to(event.position, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _window, cx| this.end_drag(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _window, cx| this.end_drag(cx)),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                this.scroll(event, cx);
            }))
            .child(edge_layer);

        for (id, origin) in &cards {
            layer = layer.child(self.render_card(id, *origin, view, &t, cx));
        }
        if view.zoom >= LABEL_MIN_ZOOM {
            for (i, ((x, y), label, color)) in labels.into_iter().enumerate() {
                layer = layer.child(
                    div()
                        .id(SharedString::from(format!("canvas-edge-label-{i}")))
                        .absolute()
                        .left(px(x + 6.0))
                        .top(px(y - 18.0))
                        .px(px(5.0))
                        .rounded(px(3.0))
                        .bg(rgb(t.bg_primary))
                        .border_1()
                        .border_color(rgb(color))
                        .text_size(px((10.0 * view.zoom).max(8.0)))
                        .text_color(rgb(color))
                        .child(label),
                );
            }
        }
        if self.projects.is_empty() {
            layer = layer.child(
                div()
                    .absolute()
                    .top(px(60.0))
                    .left(px(16.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child("No projects to show."),
            );
        }
        layer.child(self.render_toolbar(view, &t, cx))
    }
}

impl ProjectCanvas {
    /// The link scans started over the selected projects, so an agent looking
    /// for links is shown where it was asked for.
    fn link_sessions(&self, cx: &App) -> Vec<okena_ui::agent_launcher::LauncherSession> {
        let t = theme(cx);
        let workspace = self.workspace.read(cx);
        let names: Vec<String> = self
            .selected
            .iter()
            .filter_map(|id| workspace.project(id).map(|p| p.name.clone()))
            .collect();
        workspace
            .projects()
            .iter()
            .filter(|p| {
                p.custom_session
                    .as_deref()
                    .is_some_and(|label| label.starts_with("Link "))
                    && p.project_scan.as_deref().is_some_and(|scanned| {
                        scanned.split(", ").any(|n| names.iter().any(|name| name == n))
                    })
            })
            .filter_map(|p| {
                crate::views::agent_session::AgentSessionInfo::collect(
                    workspace,
                    &self.terminals,
                    &p.id,
                )
            })
            .map(|info| crate::views::agent_session::launcher_session(&info, &t))
            .collect()
    }

    /// One project's card: its name and map status, then its areas with the
    /// concepts in each — scaled with the canvas.
    fn render_card(
        &self,
        id: &str,
        origin: (f32, f32),
        view: View,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let z = view.zoom;
        let text = |size: f32| px((size * z).max(5.0));
        let report = self.maps.get(id);
        let state = report.map(|r| &r.state);
        let map = state.and_then(ProjectMapState::map);
        let rect = view.rect_to_screen(model::card_rect(origin, self.area_count(id)));
        let name = self
            .workspace
            .read(cx)
            .project(id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| id.to_string());
        let selected = self.selected.contains(id);
        let (status, status_color) = match state {
            None if self.links.is_none() => ("reading", t.text_muted),
            None => ("no map", t.text_muted),
            Some(ProjectMapState::NotScanned) => ("not scanned", t.text_muted),
            Some(ProjectMapState::Scanned { .. }) => ("scanned", t.success),
            Some(ProjectMapState::Invalid { .. }) => ("invalid", t.warning),
        };

        let mut card = div()
            .id(SharedString::from(format!("canvas-card-{id}")))
            .absolute()
            .left(px(rect.x))
            .top(px(rect.y))
            .w(px(rect.w))
            .h(px(rect.h))
            .rounded(px(8.0 * z))
            .border_1()
            .border_color(rgb(if selected { t.border_active } else { t.border }))
            .bg(rgb(t.bg_secondary))
            .cursor_pointer()
            .child(
                h_flex()
                    .absolute()
                    .left_0()
                    .top_0()
                    .w_full()
                    .h(px(HEADER_HEIGHT * z))
                    .px(px(CARD_PADDING * z))
                    .gap(px(6.0 * z))
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(text(13.0))
                            .text_color(rgb(t.text_primary))
                            .child(name),
                    )
                    .child(
                        div()
                            .px(px(5.0 * z))
                            .rounded(px(3.0 * z))
                            .border_1()
                            .border_color(rgb(status_color))
                            .text_size(text(10.0))
                            .text_color(rgb(status_color))
                            .child(status),
                    ),
            );

        if let Some(map) = map.filter(|m| !m.areas.is_empty()) {
            for (i, area) in map.areas.iter().enumerate() {
                // Relative to the card, which is the positioned parent.
                let r = model::area_rect((0.0, 0.0), i);
                let concepts: Vec<&str> = map
                    .concepts
                    .iter()
                    .filter(|c| c.areas.contains(&area.id))
                    .map(|c| c.label())
                    .collect();
                card = card.child(
                    v_flex()
                        .absolute()
                        .left(px(r.x * z))
                        .top(px(r.y * z))
                        .w(px(r.w * z))
                        .h(px(r.h * z))
                        .px(px(8.0 * z))
                        .justify_center()
                        .rounded(px(5.0 * z))
                        .bg(rgb(t.bg_primary))
                        .border_1()
                        .border_color(rgb(t.border))
                        .child(
                            div()
                                .truncate()
                                .text_size(text(11.5))
                                .text_color(rgb(t.text_primary))
                                .child(area.label().to_string()),
                        )
                        .children((!concepts.is_empty()).then(|| {
                            div()
                                .truncate()
                                .text_size(text(10.0))
                                .text_color(rgb(t.text_muted))
                                .child(concepts.join(" · "))
                        })),
                );
            }
        } else {
            let note = match state {
                Some(ProjectMapState::NotScanned) => {
                    "Scan it from its info panel to see its areas.".to_string()
                }
                Some(ProjectMapState::Invalid { problems }) => problems
                    .first()
                    .map(|p| p.message.clone())
                    .unwrap_or_default(),
                Some(ProjectMapState::Scanned { .. }) => "No areas in its map.".to_string(),
                None => String::new(),
            };
            card = card.child(
                div()
                    .absolute()
                    .left(px(CARD_PADDING * z))
                    .top(px(HEADER_HEIGHT * z))
                    .w(px((CARD_WIDTH - 2.0 * CARD_PADDING) * z))
                    .truncate()
                    .text_size(text(10.5))
                    .text_color(rgb(t.text_muted))
                    .child(note),
            );
        }

        let project = id.to_string();
        card.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                // The card, not the canvas behind it.
                cx.stop_propagation();
                if event.click_count >= 2 {
                    this.open_project(&project, cx);
                } else {
                    this.card_down(
                        project.clone(),
                        origin,
                        event.position,
                        event.modifiers.shift,
                    );
                }
            }),
        )
        .into_any_element()
    }

    /// Fit, reset, zoom level and hints — and, with two or more cards
    /// selected, the launcher that looks for links between them.
    fn render_toolbar(&self, view: View, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        let bar = h_flex()
            .gap(px(6.0))
            .items_center()
            .child(
                toolbar_button("canvas-fit", "Fit", t, cx)
                    .on_click(cx.listener(|this, _, _window, cx| this.fit(cx))),
            )
            .child(
                toolbar_button("canvas-auto-layout", "Auto layout", t, cx)
                    .on_click(cx.listener(|this, _, _window, cx| this.auto_layout(cx))),
            )
            .child(muted(format!("{:.0}%", view.zoom * 100.0), t, cx))
            .child(muted(
                "Drag to move · scroll to pan · ⌘/Ctrl-scroll to zoom · shift-click to select · double-click to open",
                t,
                cx,
            ));

        let mut panel = v_flex()
            .absolute()
            .top(px(10.0))
            .left(px(10.0))
            .gap(px(6.0))
            // Clicks here are for the toolbar, not a pan of the canvas.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(bar);

        if self.selected.len() >= 2 {
            let launcher = okena_ui::agent_launcher::AgentLauncher::new(
                "canvas-links-launcher",
                format!("Scan links across {} projects", self.selected.len()),
            )
            .options(crate::views::agent_session::launch_options(
                self.default_agent.as_deref(),
                t,
            ))
            .preferred(self.default_agent.clone())
            .sessions(self.link_sessions(cx))
            // Nothing on disk to collide over, so a finished scan must not
            // hide the way to run another.
            .launch_alongside_sessions()
            .on_open(cx.listener(|this, id: &SharedString, _window, cx| {
                this.open_project(id.as_ref(), cx);
            }))
            .busy(self.links_starting.then_some("Starting…"))
            .brief(crate::views::launch_briefs::brief_for(&self.client, "projects-scan", cx))
            .on_launch(cx.listener(
                |this, launch: &okena_ui::agent_launcher::Launch, _window, cx| {
                    this.start_links_scan(launch.command.to_string(), launch.model.clone(), cx);
                },
            ));
            panel = panel.child(div().w(px(360.0)).child(launcher));
        } else if self.selected.len() == 1 {
            panel = panel.child(muted(
                "Shift-click another project to look for links between them.",
                t,
                cx,
            ));
        }
        panel.into_any_element()
    }
}

fn toolbar_button(
    id: &'static str,
    label: &'static str,
    t: &ThemeColors,
    cx: &App,
) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(8.0))
        .py(px(3.0))
        .rounded(px(4.0))
        .bg(rgb(t.bg_secondary))
        .border_1()
        .border_color(rgb(t.border))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .text_size(ui_text_ms(cx))
        .text_color(rgb(t.text_primary))
        .child(label)
}

fn muted(text: impl Into<SharedString>, t: &ThemeColors, cx: &App) -> Div {
    div()
        .text_size(ui_text_ms(cx))
        .text_color(rgb(t.text_muted))
        .child(text.into())
}

/// Matched links are quiet, confirmed ones green, and ones only a scan found
/// blue.
fn source_color(source: LinkSource, t: &ThemeColors) -> u32 {
    match source {
        LinkSource::Matched => t.text_muted,
        LinkSource::Confirmed => t.success,
        LinkSource::FoundByScan => t.term_blue,
    }
}

/// Paint one edge and its arrowhead at the provider's end.
fn paint_stroke(window: &mut Window, origin: Point<Pixels>, stroke: &Stroke) {
    const STEPS: usize = 24;
    const HEAD: f32 = 7.0;
    let at = |p: (f32, f32)| point(origin.x + px(p.0), origin.y + px(p.1));
    let color = Hsla::from(rgb(stroke.color));

    let mut line = PathBuilder::stroke(px(1.5));
    if stroke.dashed {
        line = line.dash_array(&[px(6.0), px(4.0)]);
    }
    line.move_to(at(stroke.curve[0]));
    for step in 1..=STEPS {
        line.line_to(at(model::curve_point(
            stroke.curve,
            step as f32 / STEPS as f32,
        )));
    }
    if let Ok(path) = line.build() {
        window.paint_path(path, color);
    }

    let end = stroke.curve[3];
    let before = model::curve_point(stroke.curve, 0.92);
    let (dx, dy) = (end.0 - before.0, end.1 - before.1);
    let length = (dx * dx + dy * dy).sqrt().max(0.001);
    let (ux, uy) = (dx / length, dy / length);
    let base = (end.0 - ux * HEAD, end.1 - uy * HEAD);
    let mut head = PathBuilder::fill();
    head.move_to(at(end));
    head.line_to(at((base.0 - uy * HEAD * 0.5, base.1 + ux * HEAD * 0.5)));
    head.line_to(at((base.0 + uy * HEAD * 0.5, base.1 - ux * HEAD * 0.5)));
    head.close();
    if let Ok(path) = head.build() {
        window.paint_path(path, color);
    }
}
