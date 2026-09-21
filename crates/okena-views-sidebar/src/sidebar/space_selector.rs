//! The space selector: one row of dots at the very top of the sidebar.
//!
//! A space is a separate set of projects, agents, tasks and roots (QBL-430).
//! The selector is how you see which one you are in and move between them: one
//! dot per space, the active one highlighted, a **+** to add one, and — when
//! the sidebar is too narrow for them all — a **+N** chip holding the rest.
//!
//! The arithmetic of which dots fit is `okena_core::spaces::fit_selector`, so
//! the rule that the active space is always drawn is tested without a window.
//! What is left here is drawing and clicking.

use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use gpui_component::tooltip::Tooltip;
use okena_core::api::ActionRequest;
use okena_core::spaces::{SpaceData, fit_selector};
use okena_ui::theme::theme;
use okena_ui::tokens::ui_text_ms;

use super::Sidebar;

/// Width one dot takes, including the gap after it.
const DOT_SLOT: f32 = 18.0;
/// The row's own padding, the **+** button at the end, and the room the **+N**
/// chip takes when there is one. Counted always rather than conditionally: a
/// selector whose dot count changed the width it measured against would
/// oscillate between two layouts.
const ROW_CHROME: f32 = 24.0 + 22.0 + 30.0;

/// How many dots fit in `width` pixels of selector row.
///
/// At least one, always: a selector that shows nothing says nothing about
/// where you are, and the width can be zero for the frame before the row has
/// been laid out.
pub(super) fn dots_that_fit(width: f32) -> usize {
    let room = width - ROW_CHROME;
    if room <= 0.0 {
        return 1;
    }
    ((room / DOT_SLOT).floor() as usize).max(1)
}

/// What a space's own menu offers, in order.
///
/// Default is the space that is always there: Rename and Delete are not shown
/// on it at all, rather than shown and refused. Its task backend is editable
/// like any other space's.
pub(super) fn space_menu_entries(space_id: &str) -> Vec<&'static str> {
    let mut entries = vec!["tasks"];
    if space_id != okena_core::spaces::DEFAULT_SPACE_ID {
        entries.push("rename");
        entries.push("delete");
    }
    entries
}

/// Whether one of `space`'s agents is waiting on the user.
///
/// Read off the sidebar's own projects rather than taken from the daemon, so
/// the dot and the rows below it cannot disagree. A closed session is history
/// and is not waiting on anyone.
fn space_wants_attention(ws: &okena_workspace::state::Workspace, space_id: &str) -> bool {
    ws.data()
        .projects
        .iter()
        .filter(|p| p.space_id == space_id && !p.is_closed())
        .any(|p| {
            p.agent
                .as_ref()
                .and_then(|a| a.state)
                .is_some_and(|s| s.wants_attention())
        })
}

impl Sidebar {
    /// Ask the window to open the add-a-space form. The sidebar holds no
    /// handle to the window, so the broker is the channel — as it is for every
    /// other overlay the sidebar opens.
    pub(crate) fn request_add_space(&mut self, cx: &mut Context<Self>) {
        self.request_broker.update(cx, |broker, cx| {
            broker.push_workbench_request(
                okena_workspace::requests::WorkbenchRequest::AddSpace,
                cx,
            );
        });
    }

    /// Ask the window to open one of the per-space forms.
    pub(crate) fn request_space_form(
        &mut self,
        request: okena_workspace::requests::WorkbenchRequest,
        cx: &mut Context<Self>,
    ) {
        self.request_broker.update(cx, |broker, cx| {
            broker.push_workbench_request(request, cx);
        });
    }

    /// Switch to `space_id`. The daemon owns which space is showing, so this
    /// asks rather than sets: the change comes back on the next snapshot and
    /// in the settings the selector reads.
    pub(crate) fn switch_space(&mut self, space_id: String, cx: &mut Context<Self>) {
        self.dispatch_daemon_action(ActionRequest::SpaceActivate { space_id }, cx);
    }

    pub(super) fn render_space_selector(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let spaces = okena_workspace::spaces_state::spaces(cx);
        let active = okena_workspace::spaces_state::active_space(cx);
        // One space is no choice: the row would be a single dot that does
        // nothing, so it is not drawn until there is somewhere to go. The
        // **+** lives in the list header's menu for that case.
        if spaces.len() < 2 && !self.space_selector_forced {
            return div().into_any_element();
        }

        let active_at = spaces.iter().position(|s| s.id == active).unwrap_or(0);
        let fit = fit_selector(
            spaces.len(),
            active_at,
            dots_that_fit(f32::from(self.space_row_bounds.size.width)),
        );

        let waiting: Vec<bool> = {
            let ws = self.workspace.read(cx);
            spaces
                .iter()
                .map(|s| space_wants_attention(ws, &s.id))
                .collect()
        };

        let dots: Vec<AnyElement> = fit
            .shown
            .iter()
            .filter_map(|i| spaces.get(*i).map(|s| (*i, s)))
            .map(|(i, space)| self.render_dot(space, space.id == active, waiting[i], cx))
            .collect();

        let entity = cx.entity();
        h_flex()
            .relative()
            .w_full()
            .items_center()
            .gap(px(4.0))
            .px(px(12.0))
            .py(px(6.0))
            .child(
                canvas(
                    move |bounds, _window, app| {
                        entity.update(app, |this, _cx| {
                            this.space_row_bounds = bounds;
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .children(dots)
            .children(
                (!fit.overflow.is_empty())
                    .then(|| self.render_overflow_chip(&spaces, &fit.overflow, &waiting, cx)),
            )
            .child(self.render_add_space(cx))
            .border_b_1()
            .border_color(rgb(t.border))
            .into_any_element()
    }

    /// One space's dot. Hovering it names the space.
    fn render_dot(
        &self,
        space: &SpaceData,
        is_active: bool,
        waiting: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let id = space.id.clone();
        let menu_id = space.id.clone();
        let name = SharedString::from(space.name.clone());
        let hover_name = name.clone();
        // An agent waiting on you colours the dot even from a space you have
        // left, which is the whole point of showing it here.
        let color = if waiting {
            t.warning
        } else if is_active {
            t.text_primary
        } else {
            t.text_muted
        };
        div()
            .id(SharedString::from(format!("space-dot-{id}")))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .size(px(14.0))
            .child(
                div()
                    .size(px(if is_active { 9.0 } else { 7.0 }))
                    .rounded_full()
                    .bg(rgb(color))
                    // The active space also gets a ring, so the highlight
                    // survives a colour-blind eye and a waiting agent at once.
                    .when(is_active, |d| {
                        d.border_2().border_color(rgb(t.border_active))
                    }),
            )
            .tooltip(move |window, cx| {
                Tooltip::new(hover_name.clone()).build(window, cx)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.switch_space(id.clone(), cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.space_menu = Some((menu_id.clone(), event.position));
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// What a space's own menu offers.
    ///
    /// Rename and Delete are simply absent on Default — it is the space that
    /// is always there, and offering an action that would be refused reads as
    /// a bug. Its connection and filters are editable like any other's.
    pub(super) fn render_space_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        use okena_workspace::requests::WorkbenchRequest;
        let t = theme(cx);
        let Some((space_id, at)) = self.space_menu.clone() else {
            return div().into_any_element();
        };
        let offers = space_menu_entries(&space_id);
        let mut panel = okena_ui::menu::context_menu_panel("space-menu", &t)
            .min_w(px(220.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.space_menu = None;
                cx.notify();
            }));

        let for_tasks = space_id.clone();
        panel = panel.child(
            okena_ui::menu::menu_item(
                "space-menu-tasks",
                "icons/settings.svg",
                "Task connection and filters…",
                &t,
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.space_menu = None;
                    this.request_space_form(
                        WorkbenchRequest::EditSpaceTasks {
                            space_id: for_tasks.clone(),
                        },
                        cx,
                    );
                }),
            ),
        );

        if offers.contains(&"rename") {
            let for_rename = space_id.clone();
            let for_delete = space_id.clone();
            panel = panel
                .child(
                    okena_ui::menu::menu_item("space-menu-rename", "icons/edit.svg", "Rename…", &t)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                this.space_menu = None;
                                this.request_space_form(
                                    WorkbenchRequest::RenameSpace {
                                        space_id: for_rename.clone(),
                                    },
                                    cx,
                                );
                            }),
                        ),
                )
                .child(
                    okena_ui::menu::menu_item_with_color(
                        "space-menu-delete",
                        "icons/trash.svg",
                        "Delete space…",
                        t.error,
                        t.error,
                        &t,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.space_menu = None;
                            this.request_space_form(
                                WorkbenchRequest::DeleteSpace {
                                    space_id: for_delete.clone(),
                                },
                                cx,
                            );
                        }),
                    ),
                );
        }

        div()
            .absolute()
            .left(at.x)
            .top(at.y)
            .child(panel)
            .into_any_element()
    }

    /// The **+N** chip: the spaces that did not fit, by name.
    fn render_overflow_chip(
        &self,
        spaces: &[SpaceData],
        overflow: &[usize],
        waiting: &[bool],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let count = overflow.len();
        let hidden: Vec<(String, String, bool)> = overflow
            .iter()
            .filter_map(|i| spaces.get(*i).map(|s| (*i, s)))
            .map(|(i, s)| (s.id.clone(), s.name.clone(), waiting[i]))
            .collect();
        let entity = cx.entity();
        div()
            .id("space-overflow")
            .cursor_pointer()
            .px(px(5.0))
            .py(px(1.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(rgb(t.border))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(if hidden.iter().any(|(_, _, w)| *w) {
                t.warning
            } else {
                t.text_muted
            }))
            .child(format!("+{count}"))
            .tooltip(move |window, cx| Tooltip::new("More spaces").build(window, cx))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.space_overflow_menu = Some(hidden.clone());
                    let _ = entity;
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// The **+N** chip's menu: the spaces that did not fit, by name. Picking
    /// one switches to it, so the selector is complete however narrow the
    /// sidebar is.
    pub(super) fn render_space_overflow_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let Some(hidden) = self.space_overflow_menu.clone() else {
            return div().into_any_element();
        };
        let mut panel = okena_ui::menu::context_menu_panel("space-overflow-menu", &t)
            .min_w(px(180.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.space_overflow_menu = None;
                cx.notify();
            }))
            .child(okena_ui::menu::menu_section("Spaces", &t));
        for (id, name, waiting) in hidden {
            let target = id.clone();
            panel = panel.child(
                okena_ui::menu::menu_item_with_color(
                    SharedString::from(format!("space-menu-{id}")),
                    "icons/folder.svg",
                    // The mark travels with the name, so a space that wants
                    // you is findable without opening each one.
                    SharedString::from(if waiting {
                        format!("{name} ·")
                    } else {
                        name.clone()
                    }),
                    if waiting { t.warning } else { t.text_primary },
                    t.text_muted,
                    &t,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        this.space_overflow_menu = None;
                        this.switch_space(target.clone(), cx);
                    }),
                ),
            );
        }
        div()
            .absolute()
            .top(px(30.0))
            .left(px(12.0))
            .child(panel)
            .into_any_element()
    }

    /// The **+** that adds a space.
    fn render_add_space(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .id("space-add")
            .cursor_pointer()
            .ml_auto()
            .flex()
            .items_center()
            .justify_center()
            .size(px(18.0))
            .rounded(px(4.0))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .child(
                svg()
                    .path("icons/plus.svg")
                    .size(px(12.0))
                    .text_color(rgb(t.text_secondary)),
            )
            .tooltip(move |window, cx| Tooltip::new("Add a space").build(window, cx))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.request_add_space(cx);
                }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{dots_that_fit, space_menu_entries};

    #[test]
    fn default_is_not_offered_rename_or_delete() {
        assert_eq!(space_menu_entries("default"), ["tasks"]);
    }

    #[test]
    fn every_other_space_can_be_renamed_and_deleted() {
        assert_eq!(
            space_menu_entries("client-a"),
            ["tasks", "rename", "delete"]
        );
    }

    #[test]
    fn a_default_width_sidebar_fits_a_useful_number_of_dots() {
        // 260px is okena's default sidebar width.
        assert!(dots_that_fit(260.0) >= 10, "{}", dots_that_fit(260.0));
    }

    #[test]
    fn a_narrow_sidebar_still_shows_one_dot() {
        assert_eq!(dots_that_fit(0.0), 1);
        assert_eq!(dots_that_fit(40.0), 1);
        assert_eq!(dots_that_fit(-10.0), 1);
    }

    #[test]
    fn widening_the_sidebar_never_fits_fewer() {
        let mut last = 0;
        for width in (0..600).step_by(7) {
            let fits = dots_that_fit(width as f32);
            assert!(fits >= last, "{width}px fitted {fits} after {last}");
            last = fits;
        }
    }
}
