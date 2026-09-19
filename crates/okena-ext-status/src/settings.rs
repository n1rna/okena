//! Settings → Status: which services show on the bar.

use crate::selection::{EXTENSION_ID, selected, set_selected};
use crate::services::ServiceId;
use gpui::*;
use okena_extensions::ExtensionSettingsStore;
use okena_ui::settings::{section_container, section_header, section_note, settings_row_with_desc};
use okena_ui::toggle::toggle_switch;

pub struct StatusSettingsView;

impl StatusSettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.observe_global::<ExtensionSettingsStore>(|_, cx| cx.notify())
            .detach();
        Self
    }
}

impl Render for StatusSettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = okena_extensions::theme(cx);
        let chosen = selected(cx);
        let mut section = section_container(&t);
        let count = ServiceId::ALL.len();
        for (index, service) in ServiceId::ALL.into_iter().enumerate() {
            let on = chosen.contains(&service);
            section = section.child(
                settings_row_with_desc(
                    format!("{EXTENSION_ID}-{}", service.slug()),
                    service.label(),
                    service.description(),
                    &t,
                    cx,
                    index + 1 < count,
                )
                .child(
                    toggle_switch(format!("{EXTENSION_ID}-{}-toggle", service.slug()), on, &t)
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            set_selected(service, !on, cx);
                        }),
                ),
            );
        }
        div()
            .child(section_header("Shown on the status bar", &t, cx))
            .child(section_note(
                "Each is read from its public status page once a minute. Hover one with an open incident to read it; click to open the page.",
                &t,
                cx,
            ))
            .child(section)
    }
}
