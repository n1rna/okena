//! The dialog a Specs or Knowledge launcher picks projects and context in.
//!
//! Those launchers stand among the document or form they are about, and chip
//! searches beside them crowded it. Their gear opens this instead: the same
//! projects and context pickers the launcher sends, in a modal, kept on the
//! launcher's own state until it starts.
//!
//! A launcher's pickers are made the first time its dialog opens. Most panes
//! never open one, and each pickers component carries two live text inputs.

use super::HarnessPane;
use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_ms};
use crate::views::components::launch_pickers::{LaunchPickers, LaunchPickersEvent};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::context::ContextRef;

/// What the projects on these launchers do.
const PROJECTS_HINT: &str = "Optional. Their map entries, specs and knowledge rank first.";

/// Which launcher's picks the dialog is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextTarget {
    /// The New change form.
    SpecDraft,
    /// The Write with an agent form.
    KnowledgeDraft,
    /// The Refine with agent card under an open spec document.
    SpecRefine,
    /// The Refine with agent card under an open knowledge file.
    KnowledgeRefine,
}

impl ContextTarget {
    fn id(self) -> &'static str {
        match self {
            ContextTarget::SpecDraft => "spec-draft",
            ContextTarget::KnowledgeDraft => "knowledge-draft",
            ContextTarget::SpecRefine => "spec-refine",
            ContextTarget::KnowledgeRefine => "knowledge-refine",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            ContextTarget::SpecDraft => "For the agent drafting the new change",
            ContextTarget::KnowledgeDraft => "For the agent writing in knowledge",
            ContextTarget::SpecRefine | ContextTarget::KnowledgeRefine => {
                "For the agent refining this document"
            }
        }
    }
}

/// The context picked on a launcher, or none when its dialog never opened.
pub(super) fn picked_context(pickers: Option<&Entity<LaunchPickers>>, cx: &App) -> Vec<ContextRef> {
    pickers
        .map(|p| p.read(cx).context_refs(cx))
        .unwrap_or_default()
}

/// Forget a launcher's picked context after it started, if it had any.
pub(super) fn clear_picked_context(pickers: Option<Entity<LaunchPickers>>, cx: &mut App) {
    if let Some(pickers) = pickers {
        pickers.update(cx, |p, cx| p.clear_context(cx));
    }
}

impl HarnessPane {
    pub(super) fn open_context_dialog(&mut self, target: ContextTarget, cx: &mut Context<Self>) {
        if self.pickers_slot(target).is_none() {
            let (client, workspace) = (self.client.clone(), self.workspace.clone());
            let pickers =
                cx.new(|cx| LaunchPickers::new(target.id(), client, workspace, PROJECTS_HINT, cx));
            cx.subscribe(&pickers, |_: &mut Self, _, _: &LaunchPickersEvent, cx| {
                cx.notify()
            })
            .detach();
            *self.pickers_slot_mut(target) = Some(pickers);
        }
        self.context_dialog = Some(target);
        cx.notify();
    }

    fn close_context_dialog(&mut self, cx: &mut Context<Self>) {
        self.context_dialog = None;
        cx.notify();
    }

    fn pickers_slot(&self, target: ContextTarget) -> Option<&Entity<LaunchPickers>> {
        match target {
            ContextTarget::SpecDraft => self.specs.pickers.as_ref(),
            ContextTarget::KnowledgeDraft => self.knowledge_draft.pickers.as_ref(),
            ContextTarget::SpecRefine => self.spec_refine.pickers.as_ref(),
            ContextTarget::KnowledgeRefine => self.knowledge_refine.pickers.as_ref(),
        }
    }

    fn pickers_slot_mut(&mut self, target: ContextTarget) -> &mut Option<Entity<LaunchPickers>> {
        match target {
            ContextTarget::SpecDraft => &mut self.specs.pickers,
            ContextTarget::KnowledgeDraft => &mut self.knowledge_draft.pickers,
            ContextTarget::SpecRefine => &mut self.spec_refine.pickers,
            ContextTarget::KnowledgeRefine => &mut self.knowledge_refine.pickers,
        }
    }

    pub(super) fn render_context_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let target = self.context_dialog?;
        let pickers = self.pickers_slot(target)?.clone();
        let t = theme(cx);
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(with_alpha(0x000000, 0.45))
                // Occluding, so the view behind neither scrolls nor takes
                // hovers and clicks while the dialog is up.
                .occlude()
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    v_flex()
                        .w(px(760.0))
                        .max_h(px(620.0))
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(rgb(t.border))
                        .bg(rgb(t.bg_primary))
                        .child(
                            h_flex()
                                .items_start()
                                .gap(px(8.0))
                                .px(px(16.0))
                                .py(px(12.0))
                                .border_b_1()
                                .border_color(rgb(t.border))
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .gap(px(2.0))
                                        .child(
                                            div()
                                                .text_size(ui_text(14.0, cx))
                                                .text_color(rgb(t.text_primary))
                                                .child("Projects and context"),
                                        )
                                        .child(
                                            div()
                                                .text_size(ui_text_ms(cx))
                                                .text_color(rgb(t.text_muted))
                                                .child(target.subtitle()),
                                        ),
                                )
                                // The picks stay on the launcher; closing keeps
                                // them, and starting from it sends them.
                                .child(self.small_button(
                                    "context-dialog-done",
                                    "Done",
                                    cx.listener(|this, _, _window, cx| {
                                        this.close_context_dialog(cx)
                                    }),
                                    cx,
                                )),
                        )
                        .child(
                            div()
                                .id("context-dialog-body")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .p(px(16.0))
                                .child(pickers),
                        ),
                )
                .into_any_element(),
        )
    }
}
