//! The search island: how it is drawn, wherever it is shown.
//!
//! One floating bar — a search box, a Filters button, "N of M" and Clear
//! while anything narrows the page, and a close button — that closes to a
//! pill naming the shortcut that opens it again. The page's filters are never
//! laid out along the bar: the button opens a menu above it that holds them
//! in headed groups, so the bar stays the same size however many there are. The Projects and Agents
//! overviews, Tasks, Specs and Knowledge each own what it narrows and what its
//! chips mean; this is only the look, so the five cannot drift apart.
//!
//! Every piece comes back without a handler: the page that owns the state
//! attaches its own click and `Cancel` listeners.

use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text_ms, ui_text_sm};
use crate::views::components::{SimpleInput, SimpleInputState};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};

/// How far the island floats above the bottom of what it narrows.
const ISLAND_BOTTOM: f32 = 16.0;

/// Room a scrolling list leaves under its last row, so the island floating
/// over it never hides one for good.
pub(crate) const ISLAND_CLEARANCE: f32 = 56.0;

/// The row the island floats in, centred at the bottom of its page.
///
/// Full-width with nothing to hit, so only the island itself catches the mouse
/// and the page under the rest stays usable.
pub(crate) fn island_anchor(content: impl IntoElement) -> Div {
    h_flex()
        .absolute()
        .left_0()
        .right_0()
        .bottom(px(ISLAND_BOTTOM))
        .justify_center()
        .px(px(12.0))
        .child(content)
}

/// The open island's bar, with its search icon. The box, chips and buttons
/// are added by the page.
pub(crate) fn island_bar(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    let t = theme(cx);
    h_flex()
        .id(id)
        .occlude()
        .max_w_full()
        .min_w_0()
        .items_center()
        .gap(px(6.0))
        .pl(px(10.0))
        .pr(px(4.0))
        .py(px(4.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(rgb(t.border))
        .bg(rgb(t.bg_secondary))
        .shadow_lg()
        .child(
            svg()
                .path("icons/search.svg")
                .flex_shrink_0()
                .size(px(13.0))
                .text_color(rgb(t.text_muted)),
        )
}

/// The island's search box. The page adds the `Cancel` handler that clears it.
pub(crate) fn island_search_box(
    id: impl Into<ElementId>,
    input: &Entity<SimpleInputState>,
    cx: &App,
) -> Stateful<Div> {
    let t = theme(cx);
    div()
        .id(id)
        .w(px(240.0))
        .flex_shrink(1.0)
        .min_w(px(120.0))
        .overflow_hidden()
        .rounded(px(4.0))
        .bg(rgb(t.bg_primary))
        .child(SimpleInput::new(input).text_size(ui_text_ms(cx)))
}

/// One filter value: lit when it is part of the filter.
pub(crate) fn island_chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
    cx: &App,
) -> Stateful<Div> {
    let t = theme(cx);
    div()
        .id(id)
        .cursor_pointer()
        .flex_shrink_0()
        .px(px(7.0))
        .py(px(1.0))
        .rounded(px(3.0))
        .border_1()
        .text_size(ui_text_ms(cx))
        .map(|el| {
            if on {
                el.bg(with_alpha(t.button_primary_bg, 0.22))
                    .border_color(rgb(t.border_active))
                    .text_color(rgb(t.text_primary))
            } else {
                el.border_color(rgb(t.border))
                    .text_color(rgb(t.text_secondary))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
            }
        })
        .child(label.into())
}

/// The button that opens the filter menu, saying how many values are picked
/// so a narrowed page reads as one with the menu shut. Lit while anything is
/// picked or the menu is open.
pub(crate) fn island_filters_button(
    id: impl Into<ElementId>,
    selected: usize,
    open: bool,
    cx: &App,
) -> Stateful<Div> {
    let label = if selected > 0 {
        format!("Filters · {selected}")
    } else {
        "Filters".to_string()
    };
    island_chip(id, label, selected > 0 || open, cx)
}

/// The filter menu: a panel above the bar holding the page's filter groups.
/// Scrolls when a page has more values than fit.
pub(crate) fn island_menu(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    let t = theme(cx);
    v_flex()
        .id(id)
        .occlude()
        .min_w(px(260.0))
        .max_w(px(560.0))
        .max_h(px(280.0))
        .overflow_y_scroll()
        .gap(px(8.0))
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(rgb(t.border))
        .bg(rgb(t.bg_secondary))
        .shadow_lg()
}

/// One group in the filter menu: its heading over its values, which wrap.
pub(crate) fn island_menu_group(
    heading: &str,
    chips: impl IntoIterator<Item = impl IntoElement>,
    cx: &App,
) -> Div {
    let t = theme(cx);
    v_flex()
        .w_full()
        .gap(px(4.0))
        .child(
            div()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(heading.to_uppercase()),
        )
        .child(h_flex().gap(px(4.0)).flex_wrap().children(chips))
}

/// The bar with its filter menu, when open, stacked above it.
pub(crate) fn island_stack(menu: Option<impl IntoElement>, bar: impl IntoElement) -> Div {
    v_flex()
        .max_w_full()
        .min_w_0()
        .items_center()
        .gap(px(6.0))
        .children(menu)
        .child(bar)
}

/// A plain text button in the bar: Clear, or a page's own control.
pub(crate) fn island_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<Div> {
    let t = theme(cx);
    div()
        .id(id)
        .cursor_pointer()
        .flex_shrink_0()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(px(3.0))
        .text_size(ui_text_ms(cx))
        .text_color(rgb(t.text_secondary))
        .hover(|s| s.bg(rgb(t.bg_hover)).text_color(rgb(t.text_primary)))
        .child(label.into())
}

/// "N of M": how much of the page the island is letting through.
pub(crate) fn island_count(shown: usize, total: usize, cx: &App) -> Div {
    let t = theme(cx);
    div()
        .flex_shrink_0()
        .text_size(ui_text_ms(cx))
        .text_color(rgb(t.text_muted))
        .child(format!("{shown} of {total}"))
}

/// The button that closes the island to its pill. `shortcut` is the key that
/// does the same, named in the tooltip when there is one.
pub(crate) fn island_close(
    id: impl Into<ElementId>,
    shortcut: Option<String>,
    cx: &App,
) -> Stateful<Div> {
    let t = theme(cx);
    let tip: SharedString = match shortcut {
        Some(keys) => format!("Close search ({keys})").into(),
        None => "Close search".into(),
    };
    div()
        .id(id)
        .flex_shrink_0()
        .cursor_pointer()
        .size(px(22.0))
        .rounded(px(6.0))
        .flex()
        .items_center()
        .justify_center()
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .child(
            svg()
                .path("icons/close.svg")
                .size(px(11.0))
                .text_color(rgb(t.text_secondary)),
        )
        .tooltip(move |window, cx| {
            gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
        })
}

/// The closed island: a pill saying `label` and the shortcut that opens it,
/// edged when the page is narrowed so that never passes for the whole of it.
pub(crate) fn island_pill(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    shortcut: Option<String>,
    cx: &App,
) -> Stateful<Div> {
    let t = theme(cx);
    h_flex()
        .id(id)
        .occlude()
        .cursor_pointer()
        .items_center()
        .gap(px(6.0))
        .px(px(10.0))
        .py(px(3.0))
        .rounded_full()
        .border_1()
        .border_color(rgb(if active { t.border_active } else { t.border }))
        .bg(rgb(t.bg_secondary))
        .shadow_md()
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .text_size(ui_text_ms(cx))
        .child(
            svg()
                .path("icons/search.svg")
                .size(px(11.0))
                .text_color(rgb(t.text_muted)),
        )
        .child(div().text_color(rgb(t.text_secondary)).child(label.into()))
        .children(shortcut.map(|keys| {
            div()
                .px(px(4.0))
                .rounded(px(3.0))
                .bg(with_alpha(t.border, 0.5))
                .text_color(rgb(t.text_muted))
                .child(keys)
        }))
}

/// What the pill says: the count while the page is narrowed, else "Search".
pub(crate) fn pill_label(active: bool, shown: usize, total: usize) -> String {
    if active {
        format!("{shown} of {total}")
    } else {
        "Search".to_string()
    }
}
