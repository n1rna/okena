use crate::theme::ThemeColors;
use crate::ui::tokens::ui_text_md;
use crate::views::components::simple_input::{SimpleInput, SimpleInputState};
use gpui::prelude::FluentBuilder as _;
use gpui::*;

// Re-export from okena-ui
pub use okena_ui::settings::{
    input_box, section_container, section_header, section_note, settings_input_row, settings_row,
    settings_row_with_desc, stepper,
};
pub use okena_ui::toggle::{Segment, segmented_control, toggle_switch};

pub(super) const MONOSPACE_FONT_FAMILIES: &[&str] = &[
    "JetBrains Mono",
    "Menlo",
    "SF Mono",
    "Monaco",
    "Fira Code",
    "Source Code Pro",
    "Consolas",
    "DejaVu Sans Mono",
    "Ubuntu Mono",
    "Hack",
];

pub(super) fn font_family_label(family: &str) -> &str {
    if family == ".SystemUIFont" {
        "System UI"
    } else {
        family
    }
}

/// Render a stacked row with label, description and a full-width text input.
pub(super) fn hook_input_row(
    id: impl Into<SharedString>,
    label: &str,
    desc: &str,
    input: &Entity<SimpleInputState>,
    t: &ThemeColors,
    has_border: bool,
    cx: &App,
) -> Stateful<Div> {
    settings_input_row(id, label, desc, t, cx, has_border)
        .child(input_box(t).child(SimpleInput::new(input).text_size(ui_text_md(cx))))
}

/// The three ways to add a store. Knowledge and Specs offer the same choices
/// under the same labels, so the words live here rather than in each page
/// (QBL-415).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum AddMode {
    Clone,
    Register,
    Create,
}

impl AddMode {
    /// In the order they are offered: cloning is what most teams do.
    pub(super) const ALL: [AddMode; 3] = [AddMode::Clone, AddMode::Register, AddMode::Create];

    pub(super) fn label(self) -> &'static str {
        match self {
            AddMode::Clone => "Clone a repository",
            AddMode::Register => "Add an existing folder",
            AddMode::Create => "Create a new store",
        }
    }
}

/// One pill in a row of choices — the "add a store" modes, and the git toggle
/// that looks like them.
///
/// `id` is a `String` because callers build it per mode.
pub(super) fn mode_chip(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    selected: bool,
    t: &ThemeColors,
    cx: &App,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id.into())
        .cursor_pointer()
        .px(px(10.0))
        .py(px(3.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(rgb(if selected { t.border_active } else { t.border }))
        .when(selected, |d| {
            d.bg(crate::theme::with_alpha(t.button_primary_bg, 0.15))
        })
        .text_size(crate::ui::tokens::ui_text_ms(cx))
        .text_color(rgb(if selected {
            t.text_primary
        } else {
            t.text_secondary
        }))
        .child(label.into())
        .on_mouse_down(MouseButton::Left, on_click)
        .into_any_element()
}

/// The "initialize Git" pill a new store is created with.
///
/// Always reads as selected-coloured text: it is a checkbox drawn as a pill,
/// not one of several choices.
pub(super) fn init_git_toggle(
    id: impl Into<SharedString>,
    on: bool,
    t: &ThemeColors,
    cx: &App,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id.into())
        .cursor_pointer()
        .px(px(10.0))
        .py(px(3.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(rgb(if on { t.border_active } else { t.border }))
        .when(on, |d| {
            d.bg(crate::theme::with_alpha(t.button_primary_bg, 0.15))
        })
        .text_size(crate::ui::tokens::ui_text_ms(cx))
        .text_color(rgb(t.text_primary))
        .child(if on {
            "✓ Initialize Git with an initial commit"
        } else {
            "Initialize Git with an initial commit"
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .into_any_element()
}

/// Convert empty string to None, non-empty to Some
pub(super) fn opt_string(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}
