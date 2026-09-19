//! GitHub Copilot usage: the month's premium requests.
//!
//! Read from `api.github.com/copilot_internal/user` — the endpoint Copilot's
//! own editor plugins use to show the same figure. It is not a documented
//! API: if GitHub changes it the widget shows nothing rather than a wrong
//! number. The token is okena's usual GitHub login (`GH_TOKEN` /
//! `GITHUB_TOKEN`, else `gh auth token`).

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::h_flex;
use okena_extensions::ThemeColors;
use okena_usage::{
    UsageRow, read_working_days, render_usage_row, usage_body_container, usage_kv_row,
    usage_popover_container, usage_popover_header, usage_trigger_items,
};
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How often the figure is read. It moves with each premium request, but a
/// month's allowance does not need minute-by-minute watching.
const INTERVAL: Duration = Duration::from_secs(300);
const HOVER_DELAY_MS: u64 = 300;
/// A month, for the pace line: the allowance resets monthly, and the endpoint
/// says only when the next reset is.
const MONTH_SECS: f64 = 30.44 * 24.0 * 3600.0;

/// The month's premium-request allowance.
#[derive(Clone, Debug, PartialEq)]
pub struct CopilotData {
    /// "individual", "business"…
    pub plan: Option<String>,
    pub unlimited: bool,
    /// Requests the month allows, and how many are left.
    pub entitlement: f64,
    pub remaining: f64,
    /// Used, 0–100.
    pub used_pct: f64,
    /// When the allowance resets, Unix seconds.
    pub reset_epoch: Option<f64>,
}

impl CopilotData {
    /// How far through the month it is, 0–100, for the pace line.
    fn time_pct(&self, now: f64) -> Option<f64> {
        let reset = self.reset_epoch?;
        let left = (reset - now).clamp(0.0, MONTH_SECS);
        Some(100.0 * (1.0 - left / MONTH_SECS))
    }
}

/// Read the allowance out of the endpoint's answer. `None` when it has no
/// premium-request quota — no Copilot, or a shape this build does not know.
pub fn parse(resp: &serde_json::Value) -> Option<CopilotData> {
    let quota = &resp["quota_snapshots"]["premium_interactions"];
    if !quota.is_object() {
        return None;
    }
    let unlimited = quota["unlimited"].as_bool().unwrap_or(false);
    let entitlement = quota["entitlement"].as_f64().unwrap_or(0.0);
    let remaining = quota["remaining"]
        .as_f64()
        .or_else(|| quota["quota_remaining"].as_f64())
        .unwrap_or(0.0);
    let used_pct = match quota["percent_remaining"].as_f64() {
        Some(left) => 100.0 - left,
        None if entitlement > 0.0 => 100.0 * (1.0 - remaining / entitlement),
        None => 0.0,
    }
    .clamp(0.0, 100.0);
    let reset_epoch = resp["quota_reset_date_utc"]
        .as_str()
        .and_then(|s| s.parse::<jiff::Timestamp>().ok())
        .or_else(|| {
            let date = resp["quota_reset_date"].as_str()?.parse::<jiff::civil::Date>().ok()?;
            date.to_zoned(jiff::tz::TimeZone::UTC).ok().map(|z| z.timestamp())
        })
        .map(|ts| ts.as_second() as f64);
    Some(CopilotData {
        plan: resp["copilot_plan"].as_str().map(str::to_string),
        unlimited,
        entitlement,
        remaining,
        used_pct: if unlimited { 0.0 } else { used_pct },
        reset_epoch,
    })
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default()
}

fn fetch() -> Option<CopilotData> {
    let Some(token) = okena_git::github_dotcom_token() else {
        log::debug!("[copilot-usage] no GitHub login");
        return None;
    };
    let resp: serde_json::Value = okena_transport::http::send(
        okena_transport::http::HttpRequest::get("https://api.github.com/copilot_internal/user")
            .header("Authorization", format!("token {token}"))
            .header("Accept", "application/json")
            .user_agent("okena")
            .timeout(Duration::from_secs(10))
            .label("copilot.usage")
            .min_interval(Duration::from_secs(30)),
    )
    .ok()?
    .error_for_status()
    .ok()?
    .json()
    .ok()?;
    let parsed = parse(&resp);
    if parsed.is_none() {
        log::info!("[copilot-usage] no premium-request quota in the answer");
    }
    parsed
}

struct GlobalCopilotData(WeakEntity<CopilotUsageData>);
impl Global for GlobalCopilotData {}

/// The app's one poll, shared by every window's widget.
struct CopilotUsageData {
    data: Arc<Mutex<Option<CopilotData>>>,
    _task: Task<()>,
}

impl CopilotUsageData {
    fn shared(cx: &mut App) -> Entity<Self> {
        if let Some(existing) = cx.try_global::<GlobalCopilotData>().and_then(|g| g.0.upgrade()) {
            return existing;
        }
        let entity = cx.new(Self::new);
        cx.set_global(GlobalCopilotData(entity.downgrade()));
        entity
    }

    fn new(cx: &mut Context<Self>) -> Self {
        let data: Arc<Mutex<Option<CopilotData>>> = Arc::new(Mutex::new(None));
        let shared = data.clone();
        let task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                let fetched = smol::unblock(fetch).await;
                *shared.lock() = fetched;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
                smol::Timer::after(INTERVAL).await;
            }
        });
        Self { data, _task: task }
    }
}

/// Copilot's figure on the status bar, with the month in a panel on hover.
pub struct CopilotUsage {
    data: Entity<CopilotUsageData>,
    popover_visible: bool,
    trigger_bounds: Bounds<Pixels>,
    hover_token: Arc<AtomicU64>,
}

impl CopilotUsage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let data = CopilotUsageData::shared(cx);
        cx.observe(&data, |_, _, cx| cx.notify()).detach();
        Self {
            data,
            popover_visible: false,
            trigger_bounds: Bounds::default(),
            hover_token: Arc::new(AtomicU64::new(0)),
        }
    }

    fn hover(&mut self, hovered: bool, cx: &mut Context<Self>) {
        let token = self.hover_token.fetch_add(1, Ordering::SeqCst) + 1;
        if hovered == self.popover_visible {
            return;
        }
        let delay = if hovered { HOVER_DELAY_MS } else { 100 };
        let hover_token = self.hover_token.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            smol::Timer::after(Duration::from_millis(delay)).await;
            if hover_token.load(Ordering::SeqCst) != token {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                this.popover_visible = hovered;
                cx.notify();
            });
        })
        .detach();
    }

    fn render_popover(&self, data: &CopilotData, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        let working = read_working_days(cx);
        let plan = data.plan.as_deref().map(crate::util::capitalize_first);
        let position = point(self.trigger_bounds.origin.x, self.trigger_bounds.origin.y - px(4.0));
        let mut body = usage_body_container();
        if data.unlimited {
            body = body.child(usage_kv_row(t, cx, "Premium requests", "Unlimited".into(), t.text_primary));
        } else {
            body = body
                .child(render_usage_row(
                    t,
                    cx,
                    &UsageRow {
                        label: "Premium requests".into(),
                        period: "mo".into(),
                        pct: data.used_pct,
                        time_pct: data.time_pct(now_secs()),
                        reset_epoch: data.reset_epoch,
                        period_secs: MONTH_SECS,
                        unit: None,
                        marker_id: "copilot-marker-month".into(),
                    },
                    working,
                ))
                .child(usage_kv_row(
                    t,
                    cx,
                    "Left this month",
                    format!("{:.0} of {:.0}", data.remaining, data.entitlement),
                    t.text_primary,
                ));
        }
        deferred(
            anchored()
                .position(position)
                .anchor(Anchor::BottomLeft)
                .snap_to_window()
                .child(
                    usage_popover_container(t)
                        .id("copilot-usage-popover")
                        .occlude()
                        .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                            this.hover(*hovered, cx);
                        }))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(usage_popover_header(
                            "COPILOT USAGE",
                            plan.as_deref(),
                            "https://github.com/settings/copilot",
                            "Open Copilot settings on github.com",
                            t,
                            cx,
                        ))
                        .child(body),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }
}

impl Render for CopilotUsage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = okena_extensions::theme(cx);
        let Some(data) = self.data.read(cx).data.lock().clone() else {
            return div().size_0().into_any_element();
        };
        let items = if data.unlimited {
            Vec::new()
        } else {
            vec![("mo".into(), data.used_pct, data.time_pct(now_secs()))]
        };
        let entity = cx.entity().downgrade();
        let popover = self
            .popover_visible
            .then(|| self.render_popover(&data, &t, cx));
        div()
            .child(
                h_flex()
                    .id("copilot-usage-trigger")
                    .relative()
                    .cursor_pointer()
                    .gap(px(8.0))
                    .px(px(4.0))
                    .py(px(1.0))
                    .rounded(px(3.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .child(crate::bar::agent_icon(crate::selection::Agent::Copilot, t.text_muted))
                    .children(usage_trigger_items(&t, cx, &items))
                    .when(data.unlimited, |d| {
                        d.child(div().text_color(rgb(t.text_muted)).child("unlimited"))
                    })
                    .child(
                        canvas(
                            move |bounds, _window, app| {
                                if let Some(entity) = entity.upgrade() {
                                    entity.update(app, |this, _| this.trigger_bounds = bounds);
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
                    .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                        this.hover(*hovered, cx);
                    })),
            )
            .children(popover)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::rust_2024::test;
    use serde_json::json;

    #[test]
    fn reads_the_premium_request_allowance() {
        let resp = json!({
            "copilot_plan": "individual",
            "quota_reset_date": "2026-10-01",
            "quota_snapshots": {
                "premium_interactions": {
                    "entitlement": 300, "remaining": 225, "percent_remaining": 75.0, "unlimited": false
                }
            }
        });
        let data = parse(&resp).expect("parsed");
        assert_eq!(data.plan.as_deref(), Some("individual"));
        assert_eq!(data.used_pct, 25.0);
        assert_eq!(data.remaining, 225.0);
        assert!(data.reset_epoch.is_some());
    }

    #[test]
    fn no_premium_quota_is_nothing_to_show() {
        assert_eq!(parse(&json!({ "quota_snapshots": {} })), None);
        assert_eq!(parse(&json!({ "message": "Not Found" })), None);
    }

    #[test]
    fn an_unlimited_plan_uses_nothing() {
        let resp = json!({ "quota_snapshots": { "premium_interactions": { "unlimited": true, "percent_remaining": 0.0 } } });
        let data = parse(&resp).expect("parsed");
        assert!(data.unlimited);
        assert_eq!(data.used_pct, 0.0);
    }

    #[test]
    fn the_month_elapses_toward_the_reset() {
        let data = CopilotData {
            plan: None,
            unlimited: false,
            entitlement: 300.0,
            remaining: 300.0,
            used_pct: 0.0,
            reset_epoch: Some(1_000_000.0 + MONTH_SECS / 2.0),
        };
        let pct = data.time_pct(1_000_000.0).expect("pct");
        assert!((pct - 50.0).abs() < 0.01);
    }
}
