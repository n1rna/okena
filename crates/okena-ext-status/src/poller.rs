//! One poll per service for the whole app.
//!
//! Every window's bar holds a strong handle to the pollers of the services it
//! shows; the global keeps only weak ones. A poller — and its request every
//! minute — lives while some window shows its service, and stops once none do.

use crate::services::{ServiceId, ServiceStatus, parse};
use gpui::*;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// How often a status page is read.
const INTERVAL: Duration = Duration::from_secs(60);

#[derive(Default)]
struct GlobalPollers(HashMap<ServiceId, WeakEntity<ServicePoller>>);
impl Global for GlobalPollers {}

pub struct ServicePoller {
    status: Arc<Mutex<Option<ServiceStatus>>>,
    _task: Task<()>,
}

impl ServicePoller {
    /// The app's poller for `service`, started on first use.
    pub fn shared(service: ServiceId, cx: &mut App) -> Entity<Self> {
        if let Some(existing) = cx
            .try_global::<GlobalPollers>()
            .and_then(|g| g.0.get(&service))
            .and_then(WeakEntity::upgrade)
        {
            return existing;
        }
        let entity = cx.new(|cx| Self::new(service, cx));
        cx.default_global::<GlobalPollers>()
            .0
            .insert(service, entity.downgrade());
        entity
    }

    fn new(service: ServiceId, cx: &mut Context<Self>) -> Self {
        let status: Arc<Mutex<Option<ServiceStatus>>> = Arc::new(Mutex::new(None));
        let shared = status.clone();
        let task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                let fetched = smol::unblock(move || {
                    let resp: serde_json::Value = okena_transport::http::send(
                        okena_transport::http::HttpRequest::get(service.api_url())
                            .timeout(Duration::from_secs(10))
                            .label(service.http_label())
                            // Safety floor: the real cadence is a minute; this
                            // only ever catches a runaway re-spawn.
                            .min_interval(Duration::from_secs(5)),
                    )
                    .ok()?
                    .json()
                    .ok()?;
                    let parsed = parse(service, &resp);
                    if parsed.is_none() {
                        log::warn!("[status] {} page did not parse", service.slug());
                    }
                    parsed
                })
                .await;
                if let Some(fetched) = fetched {
                    *shared.lock() = Some(fetched);
                }
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
                smol::Timer::after(INTERVAL).await;
            }
        });
        Self {
            status,
            _task: task,
        }
    }

    /// What the page last said; `None` until it has been read.
    pub fn status(&self) -> Option<ServiceStatus> {
        self.status.lock().clone()
    }
}
