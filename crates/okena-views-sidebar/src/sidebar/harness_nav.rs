//! Harness view nav, rendered above the project list.
//!
//! Clicking an entry pushes a `WorkbenchRequest` through the shared
//! `RequestBroker`; the window drains it and opens the view as a tab in the
//! main area. The sidebar never holds the window's entity, so the broker is the
//! only channel between them.

use gpui::prelude::*;
use gpui::*;
use gpui_component::v_flex;
use okena_core::harness::HarnessSection;
use okena_ui::theme::theme;
use okena_ui::tokens::{ui_text, ui_text_ms};
use okena_workspace::harness_state::{EXTENSIONS_PAGE, active_harness};
use okena_workspace::requests::WorkbenchRequest;

use super::{Sidebar, SidebarList};

/// The harness views the nav lists, in order, with each entry's element id.
///
/// Public so the app can check that the nav, the views and the saved window
/// layout all cover the same sections — the three read `HarnessSection::all()`
/// separately, and a section one of them missed would fail silently.
pub fn harness_nav_entries() -> Vec<(HarnessSection, SharedString)> {
    HarnessSection::all()
        .into_iter()
        .map(|section| {
            let id = SharedString::from(format!("harness-nav-{}", section.slug()));
            (section, id)
        })
        .collect()
}

impl Sidebar {
    pub(super) fn render_harness_nav(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);

        let active = active_harness(self.window_id, cx);

        // The two overviews first: they are where the work is watched, and the
        // harness views below are where it is planned. Links rather than a
        // button in the list header, so every place the main area can show is
        // named in one list.
        let overviews: Vec<AnyElement> = [
            ("agents", "Agents", SidebarList::Agents),
            ("projects", "Projects", SidebarList::Projects),
        ]
        .into_iter()
        .map(|(slug, label, list)| {
            let is_active = active.is_none()
                && okena_workspace::harness_state::active_extension(self.window_id, cx).is_none()
                && self.overview_is_active(list, cx);
            self.nav_item(
                SharedString::from(format!("harness-nav-{slug}")),
                label,
                is_active,
                cx,
            )
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.open_overview(list, cx);
            }))
            .into_any_element()
        })
        .collect();

        let items: Vec<AnyElement> = harness_nav_entries()
            .into_iter()
            .map(|(section, id)| {
                let broker = self.request_broker.clone();
                let is_active = active == Some(section);
                self.nav_item(id, section.label(), is_active, cx)
                    .on_click(move |_, _window, cx| {
                        broker.update(cx, |b, cx| {
                            b.push_workbench_request(
                                WorkbenchRequest::OpenHarnessView(section),
                                cx,
                            );
                        });
                    })
                    .into_any_element()
            })
            .collect();

        // The Extensions page closes the harness list: managing what extends
        // okena is okena's own view, like the ones above it.
        let active_extension = okena_workspace::harness_state::active_extension(self.window_id, cx);
        let manage = {
            let broker = self.request_broker.clone();
            let is_active = active.is_none() && active_extension.as_deref() == Some(EXTENSIONS_PAGE);
            self.nav_item("harness-nav-extensions", "Extensions", is_active, cx)
                .on_click(move |_, _window, cx| {
                    broker.update(cx, |b, cx| {
                        b.push_workbench_request(WorkbenchRequest::OpenExtensionsPage { open: None }, cx);
                    });
                })
                .into_any_element()
        };

        // Extensions with a view of their own, in a section of their own below
        // okena's: these are pages an extension draws, not harness views. A
        // remote daemon's are named after it, since two daemons may run the
        // same extension.
        let extension_entries: Vec<(String, String)> =
            okena_workspace::extensions_state::extensions_entity(cx)
                .map(|entity| {
                    entity
                        .read(cx)
                        .with_views()
                        .map(|e| {
                            let title = e.ext.view_title.clone().unwrap_or_else(|| e.ext.name.clone());
                            let label = if e.local {
                                title
                            } else {
                                format!("{title} ({})", e.connection_name)
                            };
                            (e.key(), label)
                        })
                        .collect()
                })
                .unwrap_or_default();
        let extensions: Vec<AnyElement> = extension_entries
            .into_iter()
            .map(|(key, label)| {
                let broker = self.request_broker.clone();
                let is_active = active.is_none() && active_extension.as_deref() == Some(key.as_str());
                // Marked as an extension's, so it does not read as one more
                // of okena's views.
                self.nav_item_with_icon(
                    SharedString::from(format!("harness-nav-ext-{key}")),
                    Some("icons/puzzle.svg"),
                    SharedString::from(label),
                    is_active,
                    cx,
                )
                .on_click(move |_, _window, cx| {
                    let key = key.clone();
                    broker.update(cx, |b, cx| {
                        b.push_workbench_request(WorkbenchRequest::OpenExtensionView { key }, cx);
                    });
                })
                .into_any_element()
            })
            .collect();
        let has_extensions = !extensions.is_empty();

        let divider = || div().h(px(1.0)).mx(px(8.0)).my(px(4.0)).bg(rgb(t.border));

        v_flex()
            .child(self.nav_heading("HARNESS", cx))
            .children(overviews)
            .children(items)
            .child(manage)
            .child(divider())
            .when(has_extensions, |d| {
                d.child(self.nav_heading("EXTENSIONS", cx))
                    .children(extensions)
                    .child(divider())
            })
    }
}

impl Sidebar {
    /// The label above a group of nav entries.
    fn nav_heading(&self, label: &'static str, cx: &App) -> Div {
        let t = theme(cx);
        div()
            .h(px(28.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(label)
    }

    /// One entry in the nav, the same for an overview and a harness view.
    fn nav_item(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        is_active: bool,
        cx: &App,
    ) -> Stateful<Div> {
        self.nav_item_with_icon(id, None, label, is_active, cx)
    }

    /// A nav entry with `icon` before its label.
    fn nav_item_with_icon(
        &self,
        id: impl Into<SharedString>,
        icon: Option<&'static str>,
        label: impl Into<SharedString>,
        is_active: bool,
        cx: &App,
    ) -> Stateful<Div> {
        let t = theme(cx);
        div()
            .id(ElementId::Name(id.into()))
            .cursor_pointer()
            .h(px(24.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .when(is_active, |d| d.bg(rgb(t.bg_selection)))
            .when(!is_active, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
            .text_size(ui_text(13.0, cx))
            .text_color(if is_active {
                rgb(t.text_primary)
            } else {
                rgb(t.text_secondary)
            })
            .when_some(icon, |d, icon| {
                d.gap(px(6.0)).child(
                    svg()
                        .path(icon)
                        .size(px(12.0))
                        .flex_shrink_0()
                        .text_color(rgb(t.text_muted)),
                )
            })
            .child(label.into())
    }

    /// Leave the harness view so the projects grid shows again.
    ///
    /// Called from every path that selects a project — click and keyboard
    /// alike. Without it, selecting a project while a harness view is up would
    /// change the focus but leave the view covering the whole main area, so
    /// nothing would appear to happen.
    /// Focus a project from the sidebar, leaving any harness view.
    ///
    /// The two belong together: selecting a project means "show me that
    /// project", and a harness view left covering the main area makes the click
    /// look like it did nothing. Every sidebar row that focuses a project goes
    /// through here so a new row cannot forget the pairing — which is exactly
    /// how the project and worktree rows came to be missing it.
    ///
    /// `individual` picks the narrow focus a leaf row wants; a group header
    /// passes `false` so its worktrees stay visible alongside it.
    pub(crate) fn focus_project_from_sidebar(
        &mut self,
        project_id: String,
        individual: bool,
        cx: &mut Context<Self>,
    ) {
        self.leave_harness_view(cx);
        self.cursor_index = None;
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| {
                if individual {
                    ws.set_focused_project_individual(fm, Some(project_id.clone()), cx);
                } else {
                    ws.set_focused_project(fm, Some(project_id.clone()), cx);
                }
            });
            cx.notify();
        });
    }

    pub(crate) fn leave_harness_view(&self, cx: &mut App) {
        okena_workspace::harness_state::set_active_harness(self.window_id, None, cx);
    }
}
