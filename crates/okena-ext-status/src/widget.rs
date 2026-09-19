//! The chosen services, side by side on the status bar.
//!
//! Each shows a dot in its health's colour and its name, and says what is
//! wrong when something is. Hovering one with open incidents shows them; a
//! click opens its status page.

use crate::poller::ServicePoller;
use crate::selection::selected;
use crate::services::{Health, ServiceId, ServiceStatus};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_extensions::{ExtensionSettingsStore, ThemeColors};
use okena_ui::popover::{status_panel, status_panel_body, status_panel_divider, status_panel_header};
use okena_ui::tokens::{ui_text_ms, ui_text_sm};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// How long a hover settles before the panel opens.
const OPEN_DELAY_MS: u64 = 300;
/// How long the panel survives the pointer leaving, so it can cross the gap.
const CLOSE_DELAY_MS: u64 = 120;

pub struct StatusGroup {
    pollers: HashMap<ServiceId, Entity<ServicePoller>>,
    _observations: HashMap<ServiceId, Subscription>,
    /// The service whose panel is showing.
    open: Option<ServiceId>,
    /// Where each item ended up, so its panel can hang above it.
    bounds: HashMap<ServiceId, Bounds<Pixels>>,
    hover_token: Arc<AtomicU64>,
}

impl StatusGroup {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            pollers: HashMap::new(),
            _observations: HashMap::new(),
            open: None,
            bounds: HashMap::new(),
            hover_token: Arc::new(AtomicU64::new(0)),
        };
        this.sync(cx);
        // Choosing services in settings adds and drops them here.
        cx.observe_global::<ExtensionSettingsStore>(|this, cx| this.sync(cx))
            .detach();
        this
    }

    /// Hold a poller for every chosen service and none for the rest.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let chosen = selected(cx);
        self.pollers.retain(|service, _| chosen.contains(service));
        self._observations.retain(|service, _| chosen.contains(service));
        for service in chosen {
            if self.pollers.contains_key(&service) {
                continue;
            }
            let poller = ServicePoller::shared(service, cx);
            let observation = cx.observe(&poller, |_, _, cx| cx.notify());
            self.pollers.insert(service, poller);
            self._observations.insert(service, observation);
        }
        if self.open.is_some_and(|s| !self.pollers.contains_key(&s)) {
            self.open = None;
        }
        cx.notify();
    }

    /// Pointer over an item or its panel. `None` means it left.
    fn hover(&mut self, service: Option<ServiceId>, cx: &mut Context<Self>) {
        let token = self.hover_token.fetch_add(1, Ordering::SeqCst) + 1;
        if service == self.open {
            return;
        }
        let delay = if service.is_some() { OPEN_DELAY_MS } else { CLOSE_DELAY_MS };
        let hover_token = self.hover_token.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            smol::Timer::after(Duration::from_millis(delay)).await;
            if hover_token.load(Ordering::SeqCst) != token {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                this.open = service;
                cx.notify();
            });
        })
        .detach();
    }

    fn render_item(&self, service: ServiceId, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        let status = self.pollers.get(&service).and_then(|p| p.read(cx).status());
        let health = status.as_ref().map(|s| s.health);
        let color = health_color(health, t);
        let has_incidents = status.as_ref().is_some_and(|s| !s.incidents.is_empty());
        let entity = cx.entity().downgrade();

        h_flex()
            .id(SharedString::from(format!("status-{}", service.slug())))
            .relative()
            .cursor_pointer()
            .gap(px(5.0))
            .px(px(4.0))
            .py(px(1.0))
            .rounded(px(3.0))
            .items_center()
            .text_size(ui_text_sm(cx))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .child(div().flex_shrink_0().size(px(6.0)).rounded_full().bg(rgb(color)))
            .child(div().text_color(rgb(t.text_muted)).child(service.label()))
            .when(health.is_some_and(|h| h != Health::Operational), |d| {
                d.child(div().text_color(rgb(color)).child(health.map(Health::label).unwrap_or("")))
            })
            .child(
                canvas(
                    move |bounds, _window, app| {
                        if let Some(entity) = entity.upgrade() {
                            entity.update(app, |this, _| {
                                this.bounds.insert(service, bounds);
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .when(has_incidents, |d| {
                d.on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                    this.hover(hovered.then_some(service), cx);
                }))
            })
            .on_click(move |_, _, _| okena_core::process::open_url(service.page_url()))
            .into_any_element()
    }

    fn render_panel(&self, t: &ThemeColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let service = self.open?;
        let status = self.pollers.get(&service)?.read(cx).status()?;
        if status.incidents.is_empty() {
            return None;
        }
        let bounds = self.bounds.get(&service).copied().unwrap_or_default();
        let position = point(bounds.origin.x, bounds.origin.y - px(4.0));
        Some(
            deferred(
                anchored()
                    .position(position)
                    .anchor(Anchor::BottomLeft)
                    .snap_to_window()
                    .child(
                        incidents_panel(service, &status, t, cx)
                            .id("status-incidents")
                            .occlude()
                            .max_h(px(420.0))
                            .overflow_y_scroll()
                            .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                                this.hover(hovered.then_some(service), cx);
                            }))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_scroll_wheel(|_, _, cx| cx.stop_propagation()),
                    ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

fn health_color(health: Option<Health>, t: &ThemeColors) -> u32 {
    match health {
        Some(Health::Operational) => t.metric_normal,
        Some(Health::Degraded | Health::PartialOutage) => t.metric_warning,
        Some(Health::MajorOutage) => t.metric_critical,
        Some(Health::Maintenance | Health::Unknown) | None => t.text_muted,
    }
}

/// A service's open incidents, in the status bar's panel frame.
fn incidents_panel(service: ServiceId, status: &ServiceStatus, t: &ThemeColors, cx: &App) -> Div {
    let color = health_color(Some(status.health), t);
    let health = div()
        .text_size(ui_text_sm(cx))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(color))
        .child(status.health.label())
        .into_any_element();
    let mut body = status_panel_body();
    for (index, incident) in status.incidents.iter().enumerate() {
        let impact = match incident.impact.as_str() {
            "critical" | "major" => t.metric_critical,
            _ => t.metric_warning,
        };
        if index > 0 {
            body = body.child(status_panel_divider(t));
        }
        body = body.child(
            v_flex()
                .gap(px(6.0))
                .child(
                    h_flex()
                        .gap(px(6.0))
                        .items_center()
                        .child(div().flex_shrink_0().size(px(6.0)).rounded_full().bg(rgb(impact)))
                        .child(
                            div()
                                .text_size(ui_text_ms(cx))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(t.text_primary))
                                .child(incident.name.clone()),
                        ),
                )
                .children(incident.updates.iter().map(|update| {
                    v_flex()
                        .pl(px(12.0))
                        .gap(px(1.0))
                        .child(
                            div()
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_secondary))
                                .child(if update.status.is_empty() {
                                    update.body.clone()
                                } else {
                                    format!("{} — {}", capitalize(&update.status), update.body)
                                }),
                        )
                        .child(
                            div()
                                .text_size(ui_text_sm(cx))
                                .text_color(rgb(t.text_muted))
                                .child(update.created_at.clone()),
                        )
                })),
        );
    }
    status_panel(t)
        .child(status_panel_header(
            format!("{} STATUS", service.label().to_uppercase()),
            Some(health),
            t,
            cx,
        ))
        .child(body)
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

impl Render for StatusGroup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = okena_extensions::theme(cx);
        let items: Vec<AnyElement> = ServiceId::ALL
            .into_iter()
            .filter(|s| self.pollers.contains_key(s))
            .map(|service| self.render_item(service, &t, cx))
            .collect();
        let panel = self.render_panel(&t, cx);
        h_flex()
            .gap(px(6.0))
            .items_center()
            .children(items)
            .children(panel)
    }
}
