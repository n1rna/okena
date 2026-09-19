//! Harness → Testing: a placeholder for now.
//!
//! Verification runs moved to the agent that ran them, in the Testing tab of
//! its panel. The section keeps its place in the sidebar, and a window saved
//! with it open still reopens here, because it will hold something else.

use crate::theme::theme;
use crate::ui::tokens::ui_text;
use gpui::*;
use gpui_component::v_flex;

use super::HarnessPane;

impl HarnessPane {
    pub(super) fn render_testing_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let toolbar = self.render_toolbar(Vec::new(), cx);
        let t = theme(cx);
        v_flex()
            .size_full()
            .child(toolbar)
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_size(ui_text(14.0, cx))
                            .text_color(rgb(t.text_muted))
                            .child("Coming soon"),
                    ),
            )
            .into_any_element()
    }
}
