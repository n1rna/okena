//! Overriding one of okena's default briefs.
//!
//! `okena-defaults` is okena's own store: every file in it is rewritten to
//! match the build on each start, so it opens as a preview and nothing in the
//! view offers to change it. The one action it does offer is **Override** —
//! copy the file into a knowledge root of your own, where resolution will
//! prefer it (`okena_knowledge::prompts`).
//!
//! The daemon answers both questions this asks, because the layer order is
//! resolution's and nothing else should be reimplementing it: which roots
//! could hold a copy, and which one a launch would actually read.

use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text_ms, ui_text_xs};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use serde::Deserialize;

use super::HarnessPane;

/// What the daemon said about one file of okena's defaults.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub(crate) struct Overrides {
    /// The path this describes, so a late reply for a file you have since
    /// left is dropped rather than shown against the wrong one.
    pub(crate) path: String,
    /// False for a file nothing layers — a doc, an agent, the README — where
    /// "who overrides this?" has no answer.
    #[serde(default)]
    pub(crate) layered: bool,
    /// Key of the root a launch reads this file from, when one overrides it.
    #[serde(default)]
    pub(crate) winner: Option<String>,
    /// Every root a copy could go in, in resolution order.
    #[serde(default)]
    pub(crate) roots: Vec<OverrideRoot>,
}

impl Overrides {
    /// The root that currently wins, if any.
    pub(crate) fn winning(&self) -> Option<&OverrideRoot> {
        let key = self.winner.as_deref()?;
        self.roots.iter().find(|r| r.key == key)
    }

    /// Would a copy put in `root` actually be read?
    ///
    /// Only if nothing earlier in the order already supplies the file. Said in
    /// the picker rather than after the copy, so the answer arrives while it
    /// can still change what you pick.
    fn loses_to(&self, root: &OverrideRoot) -> Option<&OverrideRoot> {
        let winner = self.winning()?;
        let at = |key: &str| self.roots.iter().position(|r| r.key == key);
        (at(&winner.key)? < at(&root.key)?).then_some(winner)
    }
}

/// One root a copy could go in.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub(crate) struct OverrideRoot {
    pub(crate) key: String,
    pub(crate) name: String,
    /// `store` or `project`, for the line under the name.
    #[serde(default)]
    pub(crate) kind: String,
    /// This root already has the file.
    #[serde(default)]
    pub(crate) has: bool,
}

/// The Override picker's state, and the answer it is drawn from.
#[derive(Default)]
pub(crate) struct OverrideState {
    /// The daemon's answer for the open file. `None` while it is in flight,
    /// or when the open file is not one of okena's.
    pub(crate) overrides: Option<Overrides>,
    /// The picker is showing.
    pub(crate) picking: bool,
    pub(crate) busy: bool,
    pub(crate) error: Option<String>,
}

impl OverrideState {
    /// Forget everything about the file that was open. Called whenever the
    /// selection changes, so a stale answer is never shown against a new file.
    pub(crate) fn clear(&mut self) {
        self.overrides = None;
        self.picking = false;
        self.busy = false;
        self.error = None;
    }

    /// The answer, when it is about `path`.
    fn for_path(&self, path: &str) -> Option<&Overrides> {
        self.overrides.as_ref().filter(|o| o.path == path)
    }
}

impl HarnessPane {
    /// Ask the daemon who overrides `path`, for the file just opened from
    /// okena's defaults.
    pub(super) fn load_knowledge_overrides(&mut self, path: String, cx: &mut Context<Self>) {
        self.knowledge_override.clear();
        cx.notify();
        let client = self.client.clone();
        let wanted = path.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::KnowledgeOverrides { path })
                    .and_then(|v| v.ok_or_else(|| "Missing reply".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<Overrides>(v)
                            .map_err(|e| format!("Unexpected reply: {e}"))
                    })
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    // A slow reply must not describe a file opened after it.
                    if this.knowledge.selected.as_deref() != Some(wanted.as_str()) {
                        return;
                    }
                    match result {
                        Ok(o) => this.knowledge_override.overrides = Some(o),
                        Err(e) => this.knowledge_override.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Copy the open default into `root` and open the copy there, ready to
    /// edit. An existing file is opened rather than replaced — the daemon
    /// decides that, and says which happened.
    fn override_into(&mut self, root: String, cx: &mut Context<Self>) {
        let Some(path) = self.knowledge.selected.clone() else {
            return;
        };
        self.knowledge_override.busy = true;
        self.knowledge_override.error = None;
        cx.notify();

        let client = self.client.clone();
        let target = root.clone();
        cx.spawn(async move |this, cx| {
            let wanted = path.clone();
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::KnowledgeOverride { root, path })
                    .map(|_| ())
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.knowledge_override.busy = false;
                    match result {
                        Ok(()) => {
                            this.knowledge_override.picking = false;
                            // Land on the copy, in its root, ready to edit.
                            this.open_knowledge_doc(target, wanted, cx);
                        }
                        Err(e) => this.knowledge_override.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// The line above a default's text: what it is, and who is already
    /// overriding it.
    pub(super) fn render_default_notice(
        &self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let t = theme(cx);
        let overriding = self.knowledge_override.for_path(path)?.winning()?.clone();
        let key = overriding.key.clone();
        let target = path.to_string();
        Some(
            h_flex()
                .w_full()
                .items_center()
                .gap(px(8.0))
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(4.0))
                .bg(with_alpha(t.button_primary_bg, 0.10))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_secondary))
                        .child(format!(
                            "`{}` overrides this, so it is what agents are sent.",
                            overriding.name
                        )),
                )
                .child(self.small_button(
                    "knowledge-open-override",
                    "Open",
                    cx.listener(move |this, _, _window, cx| {
                        this.open_knowledge_doc(key.clone(), target.clone(), cx);
                    }),
                    cx,
                ))
                .into_any_element(),
        )
    }

    /// The header controls for a default: the badge, and the one action it
    /// offers.
    pub(super) fn render_default_controls(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx);
        vec![
            div()
                .flex_shrink_0()
                .px(px(6.0))
                .py(px(1.0))
                .rounded(px(3.0))
                .border_1()
                .border_color(rgb(t.border))
                .text_size(ui_text_xs(cx))
                .text_color(rgb(t.text_muted))
                .child("okena's default")
                .into_any_element(),
            self.small_button(
                "knowledge-override",
                if self.knowledge_override.picking {
                    "Cancel"
                } else {
                    "Override"
                },
                cx.listener(|this, _, _window, cx| {
                    this.knowledge_override.picking = !this.knowledge_override.picking;
                    this.knowledge_override.error = None;
                    cx.notify();
                }),
                cx,
            ),
        ]
    }

    /// The picker: where to put the copy.
    pub(super) fn render_override_picker(
        &self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.knowledge_override.picking {
            return None;
        }
        let t = theme(cx);
        let state = self.knowledge_override.for_path(path);
        let mut panel = v_flex()
            .w_full()
            .flex_shrink_0()
            .gap(px(6.0))
            .px(px(16.0))
            .py(px(10.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_secondary))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_primary))
                    .child("Put a copy in which root?"),
            );

        // Only templates, partials and skills are resolved across roots. A
        // copy of anything else is just a copy, and saying so here is cheaper
        // than letting someone discover it later.
        if state.is_some_and(|s| !s.layered) {
            panel = panel.child(
                div()
                    .text_size(ui_text_xs(cx))
                    .text_color(rgb(t.text_muted))
                    .child("okena reads this file only from its own store, so a copy is yours to keep rather than an override."),
            );
        }

        if let Some(err) = &self.knowledge_override.error {
            panel = panel.child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.error))
                    .child(err.clone()),
            );
        }

        let roots = state.map(|s| s.roots.as_slice()).unwrap_or_default();
        if roots.is_empty() {
            panel = panel.child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(match state {
                        // Answered, and there is genuinely nowhere to put it.
                        Some(_) => "No knowledge roots of your own yet.",
                        None => "Reading roots…",
                    }),
            );
        }
        for root in roots {
            panel = panel.child(self.render_override_root(state, root, cx));
        }

        Some(
            panel
                .child(
                    h_flex().pt(px(2.0)).child(self.small_button(
                        "knowledge-override-add-root",
                        "Add a new root…",
                        cx.listener(|this, _, _window, cx| {
                            this.open_settings_at("knowledge", Some("add"), cx);
                        }),
                        cx,
                    )),
                )
                .into_any_element(),
        )
    }

    /// One root in the picker: its name, what it would mean to pick it, and
    /// the button that does.
    fn render_override_root(
        &self,
        state: Option<&Overrides>,
        root: &OverrideRoot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        // Three things a root can be, in the order they matter to the choice.
        let note = if root.has {
            Some(("Already has this file — opens it".to_string(), t.text_muted))
        } else if let Some(winner) = state.and_then(|s| s.loses_to(root)) {
            Some((
                format!("`{}` comes first, so a copy here is not used", winner.name),
                t.warning,
            ))
        } else {
            None
        };
        let busy = self.knowledge_override.busy;
        let key = root.key.clone();

        h_flex()
            .w_full()
            .items_center()
            .gap(px(8.0))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap(px(6.0))
                            .items_center()
                            .child(
                                div()
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(t.text_primary))
                                    .child(root.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(ui_text_xs(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(root.kind.clone()),
                            ),
                    )
                    .children(note.map(|(text, color)| {
                        div()
                            .text_size(ui_text_xs(cx))
                            .text_color(rgb(color))
                            .child(text)
                    })),
            )
            .child(self.choice_chip(
                format!("knowledge-override-{}", root.key),
                if busy {
                    "Copying…".to_string()
                } else if root.has {
                    "Open".to_string()
                } else {
                    "Copy here".to_string()
                },
                false,
                move |this, cx| {
                    if !this.knowledge_override.busy {
                        this.override_into(key.clone(), cx);
                    }
                },
                cx,
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{OverrideRoot, Overrides};

    fn root(key: &str, has: bool) -> OverrideRoot {
        OverrideRoot {
            key: key.into(),
            name: key.into(),
            kind: "store".into(),
            has,
        }
    }

    fn overrides(roots: Vec<OverrideRoot>) -> Overrides {
        let winner = roots.iter().find(|r| r.has).map(|r| r.key.clone());
        Overrides {
            path: "templates/spec-draft.md".into(),
            layered: true,
            winner,
            roots,
        }
    }

    #[test]
    fn the_winner_is_the_first_root_that_has_the_file() {
        let o = overrides(vec![root("a", false), root("b", true), root("c", true)]);
        assert_eq!(o.winning().map(|r| r.key.as_str()), Some("b"));

        let none = overrides(vec![root("a", false)]);
        assert!(none.winning().is_none());
    }

    #[test]
    fn a_copy_is_only_wasted_in_a_root_that_comes_after_the_winner() {
        let o = overrides(vec![root("a", false), root("b", true), root("c", false)]);
        // Earlier than the winner: the copy would take over.
        assert!(o.loses_to(&root("a", false)).is_none());
        // Later: it would sit there unread, and the picker has to say so.
        assert_eq!(
            o.loses_to(&root("c", false)).map(|r| r.key.as_str()),
            Some("b")
        );
        // The winner does not lose to itself.
        assert!(o.loses_to(&root("b", true)).is_none());
        // With nothing overriding it, no root is wasted.
        let open = overrides(vec![root("a", false), root("b", false)]);
        assert!(open.loses_to(&root("b", false)).is_none());
    }

    #[test]
    fn an_answer_about_another_file_is_not_shown_against_this_one() {
        let state = super::OverrideState {
            overrides: Some(overrides(vec![root("a", true)])),
            ..Default::default()
        };
        assert!(state.for_path("templates/spec-draft.md").is_some());
        assert!(state.for_path("templates/task-start.md").is_none());
    }
}
