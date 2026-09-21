//! The Extensions page: every extension in one place, full-width in the main
//! area, opened from the sidebar.
//!
//! Built-in extensions and extensions from git are listed the same way — a
//! row each, with its switch, that opens to its settings. The git ones are the
//! [`ExtensionsManager`]'s, which also installs, updates and removes them; it
//! is the same list Settings → Extensions shows.

use crate::settings::settings_entity;
use crate::theme::theme;
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::harness::SessionBrief;
use okena_extensions::ExtensionRegistry;
use okena_ui::button::button_primary;
use okena_ui::theme::with_alpha;
use okena_ui::toggle::toggle_switch;
use okena_ui::tokens::{ui_text, ui_text_ms, ui_text_sm};
use okena_views_extensions::{ExtensionsManager, card, heading, icon_tile};
use okena_workspace::extensions_state::extensions_entity;
use okena_workspace::request_broker::RequestBroker;
use okena_workspace::requests::{NewAgentPrefill, OverlayRequest};
use std::collections::HashMap;

/// How wide the page's column grows; wider than this, lines get long to read.
const COLUMN_WIDTH: f32 = 860.0;

pub struct ExtensionsPage {
    manager: Entity<ExtensionsManager>,
    /// Where "Build an extension" sends its launcher request.
    broker: Entity<RequestBroker>,
    /// The built-in extension whose settings are open, by id.
    expanded: Option<&'static str>,
    /// Built-in extensions' settings views, made on first open and kept, so an
    /// edit in progress survives closing the row.
    settings_views: HashMap<&'static str, AnyView>,
}

impl ExtensionsPage {
    /// `open` shows that extension's details, by key.
    pub fn new(
        open: Option<String>,
        broker: Entity<RequestBroker>,
        cx: &mut Context<Self>,
    ) -> Self {
        let manager = cx.new(|cx| {
            let mut manager = ExtensionsManager::new(open, cx);
            // The page pads its own column.
            manager.set_inset(px(0.0), cx);
            manager
        });
        // The built-in switches read settings, the counts the installed list.
        cx.observe(&settings_entity(cx), |_, _, cx| cx.notify())
            .detach();
        if let Some(extensions) = extensions_entity(cx) {
            cx.observe(&extensions, |_, _, cx| cx.notify()).detach();
        }
        Self {
            manager,
            broker,
            expanded: None,
            settings_views: HashMap::new(),
        }
    }

    /// Open the agent launcher on the extension-build brief.
    ///
    /// Writing an extension is a doc-reading job — the reference, the
    /// template, the `wasm32-wasip2` target — so the page hands it to an
    /// agent rather than to the person. Everything but the summary is the
    /// launcher's own: which agent, which model, where it works.
    fn build_an_extension(&mut self, cx: &mut Context<Self>) {
        let prefill = NewAgentPrefill {
            heading: Some("Build an extension".to_string()),
            brief: Some(SessionBrief::ExtensionBuild),
            ..Default::default()
        };
        self.broker.update(cx, |broker, cx| {
            broker.push_overlay_request(OverlayRequest::NewAgentDialog(Box::new(prefill)), cx);
        });
    }

    /// Show the extension `key` with its details open.
    pub fn open(&mut self, key: String, cx: &mut Context<Self>) {
        self.manager.update(cx, |m, cx| m.open(key, cx));
    }

    /// Open or close a built-in extension's settings.
    fn toggle_expanded(&mut self, id: &'static str, cx: &mut Context<Self>) {
        if self.expanded == Some(id) {
            self.expanded = None;
        } else {
            if !self.settings_views.contains_key(id) {
                let factory = cx
                    .try_global::<ExtensionRegistry>()
                    .and_then(|r| r.extensions().iter().find(|e| e.manifest.id == id))
                    .and_then(|e| e.settings_view.clone());
                if let Some(factory) = factory {
                    let view = factory(cx);
                    self.settings_views.insert(id, view);
                }
            }
            self.expanded = Some(id);
        }
        cx.notify();
    }

    fn render_header(&self, built_in_on: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let (installed, running) = extensions_entity(cx)
            .map(|e| {
                let list = e.read(cx).list();
                (list.len(), list.iter().filter(|e| e.ext.enabled).count())
            })
            .unwrap_or_default();
        let stat = |value: usize, label: &'static str| {
            v_flex()
                .items_end()
                .child(
                    div()
                        .text_size(ui_text(18.0, cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(t.text_primary))
                        .child(value.to_string()),
                )
                .child(
                    div()
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_muted))
                        .child(label),
                )
        };
        h_flex()
            .w_full()
            .items_center()
            .gap(px(16.0))
            .child(
                div()
                    .flex_shrink_0()
                    .size(px(44.0))
                    .rounded(px(12.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(okena_ui::theme::with_alpha(t.border_active, 0.14))
                    .child(
                        svg()
                            .path("icons/puzzle.svg")
                            .size(px(22.0))
                            .text_color(rgb(t.border_active)),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(ui_text(20.0, cx))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(t.text_primary))
                            .child("Extensions"),
                    )
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child("Install, turn on and off, configure and remove what extends okena."),
                    ),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap(px(20.0))
                    .child(stat(built_in_on + running, "on"))
                    .child(stat(installed, "from git")),
            )
            .child(
                button_primary("ext-build-one", "Build an extension", &t)
                    .debug_selector(|| "ext-build".into())
                    .flex_shrink_0()
                    .on_click(cx.listener(|this, _, _, cx| this.build_an_extension(cx))),
            )
    }

    fn render_built_in(&self, built_in: &[BuiltIn], cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let mut rows = v_flex().gap(px(8.0));
        for &BuiltIn {
            id,
            name,
            on,
            has_settings,
        } in built_in
        {
            let (icon, blurb) = built_in_look(id);
            let expanded = has_settings && self.expanded == Some(id);
            let header = h_flex()
                .id(SharedString::from(format!("ext-builtin-{id}")))
                .px(px(14.0))
                .py(px(12.0))
                .gap(px(12.0))
                .items_center()
                .when(has_settings, |d| {
                    d.cursor_pointer()
                        .hover(|s| s.bg(rgb(t.bg_hover)))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_expanded(id, cx)))
                })
                .child(icon_tile(
                    icon,
                    if on { t.border_active } else { t.text_muted },
                ))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(3.0))
                        .child(
                            h_flex()
                                .gap(px(8.0))
                                .items_center()
                                .child(
                                    div()
                                        .text_size(ui_text(13.0, cx))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(t.text_primary))
                                        .child(name),
                                )
                                .child(
                                    div()
                                        .text_size(ui_text_sm(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child("built in"),
                                ),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(ui_text_sm(cx))
                                .text_color(rgb(t.text_muted))
                                .child(blurb),
                        ),
                )
                .child(
                    h_flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            toggle_switch(format!("ext-page-{id}-toggle"), on, &t).on_click(
                                move |_, _, cx| {
                                    cx.stop_propagation();
                                    settings_entity(cx).update(cx, |state, cx| {
                                        state.set_extension_enabled(id, !on, cx);
                                    });
                                },
                            ),
                        )
                        // A chevron only where there are settings to open, so a
                        // row that does nothing on click does not look like it
                        // would. The slot is kept, so the switches line up.
                        .child(div().w(px(12.0)).when(has_settings, |d| {
                            d.child(
                                svg()
                                    .path(if expanded {
                                        "icons/chevron-down.svg"
                                    } else {
                                        "icons/chevron-right.svg"
                                    })
                                    .size(px(12.0))
                                    .text_color(rgb(t.text_muted)),
                            )
                        })),
                );
            let mut row = card(&t)
                .when(expanded, |d| {
                    d.border_color(with_alpha(t.border_active, 0.5))
                })
                .child(header);
            if expanded && let Some(view) = self.settings_views.get(id) {
                row = row.child(
                    div()
                        .border_t_1()
                        .border_color(rgb(t.border))
                        .pb(px(12.0))
                        .child(view.clone()),
                );
            }
            rows = rows.child(row);
        }
        let on = built_in.iter().filter(|b| b.on).count();
        v_flex()
            .debug_selector(|| "ext-builtin".into())
            .gap(px(10.0))
            .child(heading(
                "Built in",
                format!("Ship with okena · {on} of {} on", built_in.len()),
                None,
                &t,
                cx,
            ))
            .child(rows)
    }
}

/// A built-in extension as its card shows it.
struct BuiltIn {
    id: &'static str,
    name: &'static str,
    on: bool,
    has_settings: bool,
}

/// The icon and one line for a built-in extension, by id.
fn built_in_look(id: &str) -> (&'static str, &'static str) {
    match id {
        "usage" => (
            "icons/sparkles.svg",
            "Coding agents' limits on the status bar",
        ),
        "status" => ("icons/bell.svg", "Services' status pages on the status bar"),
        "updater" => (
            "icons/refresh.svg",
            "Checks for new okena releases and installs them",
        ),
        _ => ("icons/puzzle.svg", "Built into okena"),
    }
}

impl Render for ExtensionsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let enabled = settings_entity(cx)
            .read(cx)
            .settings
            .enabled_extensions
            .clone();
        let built_in: Vec<BuiltIn> = cx
            .try_global::<ExtensionRegistry>()
            .map(|registry| {
                registry
                    .extensions()
                    .iter()
                    .map(|ext| BuiltIn {
                        id: ext.manifest.id,
                        name: ext.manifest.name,
                        on: enabled.contains(ext.manifest.id),
                        has_settings: ext.settings_view.is_some(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let built_in_on = built_in.iter().filter(|b| b.on).count();
        div()
            .id("extensions-page")
            .size_full()
            .overflow_y_scroll()
            .bg(rgb(t.bg_primary))
            .child(
                // Centred by auto margins at a width it is given, not inside a
                // centring row: a row sizes the column from its content, and
                // GPUI keeps a text's size from a narrow measuring pass — the
                // page came out with a screen-high gap after each section.
                v_flex()
                    .w_full()
                    .max_w(px(COLUMN_WIDTH))
                    .mx_auto()
                    .px(px(32.0))
                    .pt(px(28.0))
                    .pb(px(40.0))
                    .gap(px(28.0))
                    .child(self.render_header(built_in_on, cx))
                    .child(self.render_built_in(&built_in, cx))
                    .child(
                        v_flex()
                            .debug_selector(|| "ext-manager".into())
                            .child(self.manager.clone()),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::ExtensionsPage;
    use crate::settings::{GlobalSettings, SettingsState};
    use crate::theme::{AppTheme, GlobalTheme, ThemeMode};
    use gpui::AppContext as _;
    use gpui::{Entity, TestAppContext, VisualTestContext, px, size};
    use okena_workspace::request_broker::RequestBroker;
    use std::sync::Arc;

    fn page(
        cx: &mut TestAppContext,
    ) -> (
        Entity<ExtensionsPage>,
        Entity<RequestBroker>,
        &mut VisualTestContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            let theme = cx.new(|_| AppTheme::new(ThemeMode::Dark, true));
            cx.set_global(GlobalTheme(theme));
            cx.set_global(okena_extensions::GlobalThemeProvider(crate::theme::theme));
            cx.set_global(okena_ui::theme::GlobalThemeProvider(crate::theme::theme));
            let mut settings: okena_workspace::settings::AppSettings = Default::default();
            settings.enabled_extensions.insert("usage".into());
            let settings = cx.new(|_| SettingsState::new(settings));
            cx.set_global(GlobalSettings(settings));
            let mut registry = okena_extensions::ExtensionRegistry::new();
            for (id, name, has_settings) in [
                ("usage", "Usage", true),
                ("status", "Status", true),
                ("updater", "Auto Update", false),
            ] {
                registry.register(okena_extensions::ExtensionRegistration {
                    manifest: okena_extensions::ExtensionManifest {
                        id,
                        name,
                        default_enabled: false,
                    },
                    activate: Arc::new(|_| okena_extensions::ExtensionInstance {
                        status_bar_widgets: vec![],
                        status_bar_right_widgets: vec![],
                    }),
                    settings_view: has_settings.then(|| -> okena_extensions::SettingsViewFactory {
                        Arc::new(|app| {
                            gpui::AnyView::from(app.new(|_| Tall))
                        })
                    }),
                });
            }
            cx.set_global(registry);
            let extensions = cx.new(|_| okena_workspace::extensions_state::ExtensionsState::default());
            cx.set_global(okena_workspace::extensions_state::GlobalExtensions(extensions.clone()));
            let list = ["cli-table", "git-tree"]
                .into_iter()
                .map(|id| okena_workspace::extensions_state::ClientExtension {
                    connection_id: "local".into(),
                    connection_name: "local".into(),
                    local: true,
                    ext: serde_json::from_value(serde_json::json!({
                        "id": id, "name": id, "version": "0.1.0", "enabled": false,
                        "source": { "kind": "git", "url": "https://example.com/x.git", "commit": "c3da524985" },
                    }))
                    .expect("extension"),
                })
                .collect();
            extensions.update(cx, |state, cx| state.replace(list, cx));
        });
        let broker = cx.update(|cx| cx.new(|_| RequestBroker::new()));
        let for_page = broker.clone();
        let (page, vcx) = cx.add_window_view(|_, cx| ExtensionsPage::new(None, for_page, cx));
        vcx.simulate_resize(size(px(760.0), px(650.0)));
        vcx.run_until_parked();
        (page, broker, vcx)
    }

    /// A settings view of a known height, to measure an open row by.
    struct Tall;
    impl gpui::Render for Tall {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            use gpui::Styled as _;
            gpui::div().h(px(120.0))
        }
    }

    /// The header's one action: it opens the agent launcher on the
    /// extension-build brief, with nothing else filled in — the person picks
    /// the agent, the root and what it should do.
    #[gpui::test]
    fn build_an_extension_opens_the_launcher_for_that_brief(cx: &mut TestAppContext) {
        use okena_workspace::requests::OverlayRequest;
        let (_page, broker, vcx) = page(cx);
        let button = vcx.debug_bounds("ext-build").expect("the header's button");
        vcx.simulate_click(button.center(), gpui::Modifiers::default());
        vcx.run_until_parked();

        let requests = vcx.update(|_, cx| broker.update(cx, |b, _| b.drain_overlay_requests()));
        let prefill = match requests.as_slice() {
            [OverlayRequest::NewAgentDialog(prefill)] => prefill.clone(),
            other => panic!("expected one new-agent request, got {other:?}"),
        };
        assert_eq!(
            prefill.brief,
            Some(okena_core::harness::SessionBrief::ExtensionBuild)
        );
        assert_eq!(prefill.heading.as_deref(), Some("Build an extension"));
        // Nothing prefilled: the summary, the root and the projects are the
        // person's to fill in.
        assert!(prefill.goal.is_empty());
        assert!(prefill.root.is_empty());
        assert!(prefill.project_ids.is_empty());
    }

    /// The installed list follows the built-in one by the column's gap, not a
    /// screen further down — closed, and with a row's settings open.
    #[gpui::test]
    fn the_sections_follow_each_other(cx: &mut TestAppContext) {
        let (page, _broker, vcx) = page(cx);
        let gap = |vcx: &mut VisualTestContext| {
            let built_in = vcx.debug_bounds("ext-builtin").expect("built-in section");
            let manager = vcx.debug_bounds("ext-manager").expect("installed section");
            (manager.top() - built_in.bottom(), built_in.size.height)
        };
        let (between, closed) = gap(vcx);
        assert_eq!(between, px(28.0));

        vcx.update(|_, cx| page.update(cx, |page, cx| page.toggle_expanded("usage", cx)));
        vcx.run_until_parked();
        let (between, open) = gap(vcx);
        assert_eq!(between, px(28.0));
        assert!(
            open > closed + px(120.0),
            "the settings open inside the row"
        );

        // A row with nothing to configure does not open.
        vcx.update(|_, cx| page.update(cx, |page, cx| page.toggle_expanded("usage", cx)));
        vcx.update(|_, cx| page.update(cx, |page, cx| page.toggle_expanded("updater", cx)));
        vcx.run_until_parked();
        assert_eq!(gap(vcx).1, closed);
    }
}
