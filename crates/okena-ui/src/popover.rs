//! Reusable popover components for anchored floating panels.
//!
//! Provides anchored positioning and styled panel container.
//! Uses GPUI's `deferred(anchored().position().snap_to_window())` pattern.

use crate::theme::ThemeColors;
use gpui::*;

/// Styled popover panel container with standard look: bg, border, rounded corners, shadow.
///
/// Stops mouse-down and scroll-wheel propagation to prevent interaction with elements underneath.
pub fn popover_panel(id: impl Into<SharedString>, t: &ThemeColors) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .occlude()
        .bg(rgb(t.bg_primary))
        .border_1()
        .border_color(rgb(t.border))
        .rounded(px(6.0))
        .shadow_lg()
        .p(px(8.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_scroll_wheel(|_, _, cx| {
            cx.stop_propagation();
        })
}

// ─── Status-bar panels ───────────────────────────────────────────────────────
//
// The panels that hang off the status bar — MEM's breakdown, the Claude and
// Codex usage — share one look: a titled header, a padded body of rows, and
// dividers between groups. Built here so they cannot drift apart.

/// A status-bar panel's frame. The caller adds `id`, `occlude` and handlers.
pub fn status_panel(t: &ThemeColors) -> Div {
    div()
        .min_w(px(300.0))
        .max_w(px(420.0))
        .bg(rgb(t.bg_primary))
        .border_1()
        .border_color(rgb(t.border))
        .rounded(px(8.0))
        .shadow_lg()
}

/// A status-bar panel's header: an uppercase title on the left and, on the
/// right, whatever the panel puts there.
pub fn status_panel_header(
    title: impl Into<SharedString>,
    trailing: Option<AnyElement>,
    t: &ThemeColors,
    cx: &App,
) -> Div {
    gpui_component::h_flex()
        .px(px(12.0))
        .py(px(7.0))
        .items_center()
        .justify_between()
        .border_b_1()
        .border_color(rgb(t.border))
        .child(
            div()
                .text_size(crate::tokens::ui_text_xs(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(t.text_secondary))
                .child(title.into()),
        )
        .children(trailing)
}

/// A status-bar panel's padded body; the caller adds the rows.
pub fn status_panel_body() -> Div {
    gpui_component::v_flex().px(px(12.0)).py(px(10.0)).gap(px(7.0))
}

/// A line between groups of rows.
pub fn status_panel_divider(t: &ThemeColors) -> Div {
    div().h(px(1.0)).w_full().bg(rgb(t.border))
}

/// A label on the left and its value, emphasised, on the right.
pub fn status_panel_row(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    value_color: u32,
    t: &ThemeColors,
    cx: &App,
) -> Div {
    gpui_component::h_flex()
        .items_baseline()
        .justify_between()
        .gap(px(16.0))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(crate::tokens::ui_text_ms(cx))
                .text_color(rgb(t.text_secondary))
                .child(label.into()),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(crate::tokens::ui_text_ms(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(value_color))
                .child(value.into()),
        )
}
