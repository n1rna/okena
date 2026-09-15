//! gpui-component's code editor, set up for prose.
//!
//! The Specs and Knowledge document editor, and every box a goal or brief is
//! typed in for an agent. Rope-backed text, undo, line-wise arrow keys, IME,
//! find, and a scroll that follows the caret wherever it is — none of which
//! `SimpleInput` has.

use crate::theme::theme;
use crate::ui::tokens::ui_text;
use gpui::prelude::*;
use gpui::*;
use gpui_component::input::{Input, InputState};
use std::sync::Arc;

/// The editor over source in `language`, laid out as prose: soft-wrapped, and
/// no gutter to fold or number lines in.
pub fn source_editor(
    language: &'static str,
    window: &mut Window,
    cx: &mut Context<InputState>,
) -> InputState {
    InputState::new(window, cx)
        .code_editor(language)
        .line_number(false)
        .folding(false)
        .soft_wrap(true)
}

/// A goal or brief typed for an agent: Markdown source, no preview.
///
/// An input needs a window to be built, and the state holding this usually has
/// none, so it is made the first frame the box is shown. Until then, and after
/// `clear`, its text is what it was given to open with.
pub struct BriefInput {
    placeholder: &'static str,
    initial: String,
    input: Option<Entity<InputState>>,
}

impl BriefInput {
    pub fn new(placeholder: &'static str) -> Self {
        Self {
            placeholder,
            initial: String::new(),
            input: None,
        }
    }

    /// Open with `value` already typed.
    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.initial = value.into();
        self
    }

    /// Build the input, if this is the first frame it is shown.
    pub fn ensure(&mut self, window: &mut Window, cx: &mut App) {
        sync_editor_colors(cx);
        if self.input.is_some() {
            return;
        }
        let placeholder = self.placeholder;
        let initial = self.initial.clone();
        self.input = Some(cx.new(|cx| {
            source_editor("markdown", window, cx)
                .placeholder(placeholder)
                .default_value(initial)
        }));
    }

    pub fn value(&self, cx: &App) -> String {
        match &self.input {
            Some(input) => input.read(cx).value().to_string(),
            None => self.initial.clone(),
        }
    }

    /// Empty the box. The input goes, undo history with it; the next frame
    /// that shows the box makes a fresh one.
    pub fn clear(&mut self) {
        self.input = None;
        self.initial.clear();
    }

    /// The box, `height` tall, scrolling inside rather than growing.
    pub fn render(&self, height: f32, cx: &App) -> AnyElement {
        let t = theme(cx);
        okena_ui::input::input_container(&t, None)
            .w_full()
            .h(px(height))
            .py(px(6.0))
            .children(self.input.as_ref().map(|input| {
                Input::new(input)
                    .appearance(false)
                    .h_full()
                    .px(px(8.0))
                    .text_size(ui_text(13.0, cx))
                    .text_color(rgb(t.text_primary))
            }))
            .into_any_element()
    }
}

/// Paint the editor in okena's theme rather than gpui-component's.
///
/// Its caret, selection and background come from gpui-component's global
/// theme, which okena only ever switches between light and dark. This editor is
/// the only gpui-component input okena draws, so taking those colours over is
/// safe; only what differs is written, so a frame that changes nothing does not
/// mark the global changed.
pub fn sync_editor_colors(cx: &mut App) {
    let t = theme(cx);
    let caret: Hsla = rgb(t.cursor).into();
    let selection: Hsla = rgb(t.bg_selection).into();
    let background: Hsla = rgb(t.bg_primary).into();
    let foreground: Hsla = rgb(t.text_primary).into();

    let current = gpui_component::Theme::global(cx);
    let style = &current.highlight_theme.style;
    let up_to_date = current.caret == caret
        && current.selection == selection
        && style.editor_background == Some(background)
        && style.editor_foreground == Some(foreground)
        && style.editor_active_line.is_none();
    if up_to_date {
        return;
    }
    let global = gpui_component::Theme::global_mut(cx);
    global.caret = caret;
    global.selection = selection;
    let mut highlight = (*global.highlight_theme).clone();
    highlight.style.editor_background = Some(background);
    highlight.style.editor_foreground = Some(foreground);
    highlight.style.editor_active_line = None;
    global.highlight_theme = Arc::new(highlight);
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::BriefInput;
    use crate::theme::{AppTheme, GlobalTheme, ThemeMode};
    use gpui::AppContext as _;
    use gpui::{TestAppContext, VisualTestContext};

    #[gpui::test]
    fn a_brief_opens_on_its_prefill_reads_what_is_typed_and_clears(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            let theme = cx.new(|_| AppTheme::new(ThemeMode::Dark, true));
            cx.set_global(GlobalTheme(theme));
        });
        let vcx: &mut VisualTestContext = cx.add_empty_window();

        let mut brief = BriefInput::new("What should the agent do?").with_value("Prefilled goal");
        // Before the first frame there is no input, and the prefill is the text.
        assert_eq!(vcx.update(|_, cx| brief.value(cx)), "Prefilled goal");

        vcx.update(|window, cx| brief.ensure(window, cx));
        let input = brief.input.clone().expect("built on its first frame");
        assert_eq!(vcx.update(|_, cx| brief.value(cx)), "Prefilled goal");

        let typed = "  Line one\nLine two\n\nA paragraph.  ";
        vcx.update(|window, cx| input.update(cx, |i, cx| i.set_value(typed, window, cx)));
        // Exactly what was typed; trimming is the caller's, on submit.
        assert_eq!(vcx.update(|_, cx| brief.value(cx)), typed);

        brief.clear();
        assert_eq!(vcx.update(|_, cx| brief.value(cx)), "");
        vcx.update(|window, cx| brief.ensure(window, cx));
        assert_ne!(brief.input.as_ref(), Some(&input), "a fresh input after clear");
        assert_eq!(vcx.update(|_, cx| brief.value(cx)), "");
    }
}
