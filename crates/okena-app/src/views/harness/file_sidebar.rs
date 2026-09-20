//! The file sidebar the Knowledge and Specs views share.
//!
//! Both views are the same shape: a column of roots and their files beside
//! whichever document you picked. They used to hold a `TREE_WIDTH` constant
//! and a container each, which is how two sidebars that must behave alike
//! drift apart — so the shape lives here once, and each view only fills the
//! column with its own rows.
//!
//! Open state and width are persisted (`AppSettings::harness_files`), shared
//! by both views, and mirrored into the pane so a drag costs a frame rather
//! than a settings write per mouse move.

use crate::theme::theme;
use gpui::prelude::*;
use gpui::*;
use gpui_component::v_flex;
use okena_ui::resizable_sidebar::{ResizableSidebarState, resizable_sidebar};

use super::HarnessPane;

/// A harness file sidebar: whether it is open, and how wide.
pub(crate) struct FileSidebar {
    pub(crate) open: bool,
    /// Width, and the drag in flight. Seeded from settings; written back when
    /// the drag ends.
    pub(crate) resize: ResizableSidebarState,
}

impl FileSidebar {
    /// Start from what the last session left behind.
    pub(crate) fn from_settings(cx: &App) -> Self {
        let saved = crate::settings::settings(cx).harness_files;
        Self {
            open: saved.is_open,
            resize: ResizableSidebarState::new(saved.width),
        }
    }

    /// Take a setting another pane changed.
    ///
    /// The width is refused while this pane is mid-drag: the saved value is
    /// the one from before the gesture started, and taking it back would fight
    /// the mouse for the rest of the drag.
    pub(crate) fn sync(&mut self, open: bool, width: f32) {
        self.open = open;
        if !self.resize.is_resizing() {
            self.resize.set_width(width);
        }
    }
}

impl HarnessPane {
    /// The scrolling column a view builds its rows into.
    ///
    /// No width and no border of its own: the sidebar around it owns both, so
    /// a resize moves one edge rather than two that must agree.
    pub(super) fn file_sidebar_column(&self, id: &'static str) -> Stateful<Div> {
        v_flex()
            .id(id)
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .px(px(6.0))
            .pb(px(10.0))
    }

    /// Wrap a built column as the sidebar proper: the saved width, the divider
    /// you drag it by, and the window-level mouse-up that ends the drag.
    pub(super) fn render_file_sidebar(
        &self,
        column: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let entity = cx.entity().downgrade();
        let entity_for_end = entity.clone();
        resizable_sidebar(
            self.files.resize.width(),
            t.bg_primary,
            t.border,
            t.border_active,
            vec![column],
            move |mouse_pos, cx| {
                if let Some(entity) = entity.upgrade() {
                    entity.update(cx, |this, _| {
                        this.files.resize.start_resize(f32::from(mouse_pos.x));
                    });
                }
            },
            // The width is only written back here, not on every move: a drag
            // is dozens of events, and each save is a round trip to the
            // daemon that owns the settings file.
            move |cx| {
                if let Some(entity) = entity_for_end.upgrade() {
                    entity.update(cx, |this, cx| this.end_file_sidebar_resize(cx));
                }
            },
        )
        .into_any_element()
    }

    /// Follow a drag in flight. Called from the pane's mouse-move handler,
    /// and does nothing unless the divider is being dragged.
    pub(crate) fn drag_file_sidebar(&mut self, mouse_x: f32, cx: &mut Context<Self>) {
        if self.files.resize.update_resize(mouse_x) {
            cx.notify();
        }
    }

    /// End a drag and remember the width it landed on.
    pub(crate) fn end_file_sidebar_resize(&mut self, cx: &mut Context<Self>) {
        if !self.files.resize.is_resizing() {
            return;
        }
        self.files.resize.end_resize();
        let width = self.files.resize.width();
        crate::settings::settings_entity(cx)
            .update(cx, |s, cx| s.set_harness_files_width(width, cx));
    }

    /// Open the sidebar, or close it and give the document the space.
    pub(crate) fn toggle_file_sidebar(&mut self, cx: &mut Context<Self>) {
        let open = !self.files.open;
        self.files.open = open;
        crate::settings::settings_entity(cx).update(cx, |s, cx| s.set_harness_files_open(open, cx));
        cx.notify();
    }

    /// Take the open state and width a settings change carries, so the Specs
    /// and Knowledge panes — which are separate entities over one setting —
    /// stay in step without either one reloading.
    pub(crate) fn sync_file_sidebar(&mut self, settings: &okena_workspace::settings::AppSettings) {
        self.files
            .sync(settings.harness_files.is_open, settings.harness_files.width);
    }

    /// The toolbar's sidebar toggle, at the far left — the same control the
    /// file and diff viewers carry, in the same place.
    ///
    /// `has_roots` is false where the view shows its empty state instead of a
    /// sidebar. The control stays, dimmed, rather than appearing once roots
    /// arrive: a toolbar that grows a button under you is harder to read than
    /// one with nothing to press yet.
    pub(super) fn file_sidebar_toggle(
        &self,
        has_roots: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let open = self.files.open && has_roots;
        div()
            .id("harness-files-toggle")
            .flex_shrink_0()
            .when(has_roots, |d| d.cursor_pointer())
            .size(px(24.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(rgb(if open { t.border_active } else { t.bg_primary }))
            .bg(rgb(if open { t.bg_secondary } else { t.bg_primary }))
            .when(has_roots, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path("icons/panel-left.svg")
                    .size(px(13.0))
                    .text_color(rgb(if !has_roots {
                        t.text_muted
                    } else if open {
                        t.text_primary
                    } else {
                        t.text_secondary
                    }))
                    .opacity(if has_roots { 1.0 } else { 0.35 }),
            )
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(if !has_roots {
                    "No roots"
                } else if open {
                    "Hide files"
                } else {
                    "Show files"
                })
                .build(window, cx)
            })
            .when(has_roots, |d| {
                d.on_click(cx.listener(|this, _, _window, cx| this.toggle_file_sidebar(cx)))
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::FileSidebar;
    use okena_ui::resizable_sidebar::ResizableSidebarState;

    fn sidebar(width: f32) -> FileSidebar {
        FileSidebar {
            open: true,
            resize: ResizableSidebarState::new(width),
        }
    }

    #[test]
    fn the_other_views_width_and_open_state_are_taken_when_idle() {
        // Specs and Knowledge are separate panes over one setting: what you
        // do in the open one must be what you find in the other.
        let mut files = sidebar(280.0);
        files.sync(false, 360.0);
        assert!(!files.open);
        assert_eq!(files.resize.width(), 360.0);
    }

    #[test]
    fn a_settings_change_mid_drag_does_not_yank_the_width_back() {
        // The saved width is the one from before this gesture began. Taking
        // it while the mouse is down would undo the drag every frame.
        let mut files = sidebar(280.0);
        files.resize.start_resize(500.0);
        files.resize.update_resize(560.0);
        assert_eq!(files.resize.width(), 340.0);

        files.sync(true, 280.0);
        assert_eq!(files.resize.width(), 340.0, "the drag lost its width");

        // Once the drag is over, the next change lands as usual.
        files.resize.end_resize();
        files.sync(true, 280.0);
        assert_eq!(files.resize.width(), 280.0);
    }
}
