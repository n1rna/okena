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
use crate::ui::tokens::{ui_text_md, ui_text_ms, ui_text_xs};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use serde::Deserialize;
use std::collections::HashMap;

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

/// Which roots hold a copy of each layered file, and which copy is applied.
///
/// One answer for every root at once (`ActionRequest::KnowledgeLayering`),
/// because both things drawn from it are lists: a template's detail page names
/// every root holding a copy, and the sidebar marks each template that has an
/// override (QBL-426).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub(crate) struct Layering {
    /// Every healthy root in layering order, okena's own last.
    #[serde(default)]
    pub(crate) roots: Vec<LayerRoot>,
    /// Per path relative to a root, the copies of it and the applied one.
    #[serde(default)]
    pub(crate) paths: HashMap<String, Copies>,
}

/// One root in the layer order.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub(crate) struct LayerRoot {
    pub(crate) key: String,
    pub(crate) name: String,
    /// `store` or `project`, for the line beside the name.
    #[serde(default)]
    pub(crate) kind: String,
    /// okena's own defaults store: not a layer, the fallback made readable.
    #[serde(default)]
    pub(crate) builtin: bool,
}

/// What the roots hold of one file.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub(crate) struct Copies {
    /// Roots where the file exists, in layering order.
    #[serde(default)]
    pub(crate) copies: Vec<String>,
    /// The one a launch reads. Not always the first copy: an empty file is a
    /// placeholder, not an answer, and falls through to the layer below.
    #[serde(default)]
    pub(crate) applied: Option<String>,
}

/// What the sidebar puts beside a template that has an override.
///
/// Two readings, because the answer to "is my override doing anything?" is
/// what someone is looking for: [`Badge::Override`] when the copy a launch
/// reads is one of yours, [`Badge::Default`] when a copy exists but okena's
/// own is still what is sent — an empty placeholder, or a copy that only sits
/// below the one that wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Badge {
    Override,
    Default,
}

impl Badge {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Badge::Override => "override",
            Badge::Default => "default",
        }
    }
}

impl Layering {
    fn root(&self, key: &str) -> Option<&LayerRoot> {
        self.roots.iter().find(|r| r.key == key)
    }

    /// Every root holding a copy of `path`, in layering order, paired with
    /// whether it is the copy a launch reads.
    ///
    /// Empty for a path nothing layers — a doc, an agent, the README — where
    /// "which root supplies this?" has no answer.
    pub(crate) fn copies_of(&self, path: &str) -> Vec<(&LayerRoot, bool)> {
        let Some(copies) = self.paths.get(path) else {
            return Vec::new();
        };
        copies
            .copies
            .iter()
            .filter_map(|key| {
                let root = self.root(key)?;
                Some((root, copies.applied.as_deref() == Some(key.as_str())))
            })
            .collect()
    }

    /// What to mark `path` with in the entry list, if anything.
    ///
    /// Nothing unless a root of your own holds a copy: a template only okena
    /// has is the ordinary case, and a mark on every row would say nothing.
    pub(crate) fn badge(&self, path: &str) -> Option<Badge> {
        let copies = self.copies_of(path);
        if !copies.iter().any(|(root, _)| !root.builtin) {
            return None;
        }
        Some(match copies.iter().find(|(_, applied)| *applied) {
            Some((root, _)) if !root.builtin => Badge::Override,
            _ => Badge::Default,
        })
    }
}

/// The Override picker's state, and the answer it is drawn from.
#[derive(Default)]
pub(crate) struct OverrideState {
    /// Which roots hold each layered file. Loaded with the tree, and kept
    /// across selections: it describes every entry, not the open one.
    pub(crate) layering: Layering,
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
    /// Load which roots hold a copy of each layered file.
    ///
    /// Once per knowledge refresh rather than per row: the whole entry list is
    /// marked from it, and the open file's root list too. That is also what
    /// makes it follow the world — a copy deleted, a file saved or the root
    /// order changed all refresh the view, and the answer is rebuilt with it.
    pub(super) fn load_knowledge_layering(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::KnowledgeLayering)
                    .and_then(|v| v.ok_or_else(|| "Missing reply".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<Layering>(v)
                            .map_err(|e| format!("Unexpected reply: {e}"))
                    })
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    // A failure leaves the last answer standing: a stale mark
                    // is better than every template losing its list at once,
                    // and the next refresh tries again.
                    if let Ok(layering) = result {
                        this.knowledge_override.layering = layering;
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    /// The mark beside a template in the entry list: it has an override, and
    /// whether that override is what a launch reads.
    pub(super) fn render_layering_badge(
        &self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let badge = self.knowledge_override.layering.badge(path)?;
        let t = theme(cx);
        Some(
            div()
                .flex_shrink_0()
                .text_size(ui_text_xs(cx))
                .text_color(rgb(match badge {
                    Badge::Override => t.button_primary_bg,
                    Badge::Default => t.text_muted,
                }))
                .child(badge.label())
                .into_any_element(),
        )
    }

    /// Under an open template: every root holding a copy of it, in layering
    /// order, with the one a launch reads marked.
    ///
    /// The list includes okena's own store, which is where the answer "nothing
    /// overrides this, so the default is what is sent" comes from. Nothing is
    /// shown for a file the layers do not resolve.
    pub(super) fn render_layering_list(
        &self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let copies = self.knowledge_override.layering.copies_of(path);
        if copies.is_empty() {
            return None;
        }
        let t = theme(cx);
        let mut rows = v_flex().gap(px(1.0));
        for (root, applied) in copies {
            let key = root.key.clone();
            let target = path.to_string();
            rows = rows.child(
                h_flex()
                    .id(SharedString::from(format!("knowledge-copy-{}", root.key)))
                    .cursor_pointer()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(4.0))
                    .py(px(1.0))
                    .rounded(px(3.0))
                    .when(applied, |d| {
                        d.bg(with_alpha(t.button_primary_bg, 0.14))
                    })
                    .when(!applied, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
                    .child(
                        div()
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(if applied {
                                t.text_primary
                            } else {
                                t.text_secondary
                            }))
                            .child(root.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(ui_text_xs(cx))
                            .text_color(rgb(t.text_muted))
                            .child(root.kind.clone()),
                    )
                    .when(applied, |d| {
                        d.child(
                            div()
                                .text_size(ui_text_xs(cx))
                                .text_color(rgb(t.button_primary_bg))
                                .child("applied"),
                        )
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.open_knowledge_doc(key.clone(), target.clone(), cx);
                        }),
                    ),
            );
        }
        Some(
            h_flex()
                .gap(px(10.0))
                .items_start()
                .child(
                    div()
                        .w(px(72.0))
                        .flex_shrink_0()
                        .pt(px(1.0))
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child("Copies"),
                )
                .child(rows)
                .into_any_element(),
        )
    }

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
        } else {
            state.and_then(|s| s.loses_to(root)).map(|winner| {
                (
                    format!("`{}` comes first, so a copy here is not used", winner.name),
                    t.warning,
                )
            })
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
    use super::{Badge, Copies, LayerRoot, Layering, OverrideRoot, Overrides};

    /// The layer order okena always ends in: your roots, then okena's own.
    fn layering(paths: &[(&str, &[&str], Option<&str>)]) -> Layering {
        let root = |key: &str, builtin: bool| LayerRoot {
            key: key.into(),
            name: key.into(),
            kind: if builtin { "store" } else { "project" }.into(),
            builtin,
        };
        Layering {
            roots: vec![
                root("acme", false),
                root("zeta", false),
                root("store:okena-defaults", true),
            ],
            paths: paths
                .iter()
                .map(|(path, copies, applied)| {
                    (
                        (*path).to_string(),
                        Copies {
                            copies: copies.iter().map(|c| (*c).to_string()).collect(),
                            applied: applied.map(str::to_string),
                        },
                    )
                })
                .collect(),
        }
    }

    const BRIEF: &str = "templates/briefs/spec-draft.md";

    #[test]
    fn the_copies_of_a_template_come_in_layer_order_with_the_applied_one_marked() {
        let l = layering(&[(
            BRIEF,
            &["acme", "zeta", "store:okena-defaults"],
            Some("acme"),
        )]);
        assert_eq!(
            l.copies_of(BRIEF)
                .iter()
                .map(|(r, applied)| (r.key.as_str(), *applied))
                .collect::<Vec<_>>(),
            [
                ("acme", true),
                ("zeta", false),
                ("store:okena-defaults", false)
            ]
        );

        // A path nothing layers has no list at all — a doc is read from the
        // root you opened it in, so "who supplies it" is a category error.
        assert!(l.copies_of("docs/ci.md").is_empty());

        // A root the answer no longer lists is dropped rather than drawn as a
        // nameless row: the two halves come from one reply, but a later
        // refresh can still race a root being unregistered.
        let mut stale = l.clone();
        stale.roots.retain(|r| r.key != "zeta");
        assert_eq!(
            stale
                .copies_of(BRIEF)
                .iter()
                .map(|(r, _)| r.key.as_str())
                .collect::<Vec<_>>(),
            ["acme", "store:okena-defaults"]
        );
    }

    #[test]
    fn the_list_marks_a_template_by_whether_your_copy_is_the_one_sent() {
        // Only okena has it: the ordinary case, and no mark.
        let none = layering(&[(
            BRIEF,
            &["store:okena-defaults"],
            Some("store:okena-defaults"),
        )]);
        assert_eq!(none.badge(BRIEF), None);
        assert_eq!(none.badge("docs/ci.md"), None);

        // A root of yours overrides it and wins: that is what agents are sent.
        let won = layering(&[(BRIEF, &["acme", "store:okena-defaults"], Some("acme"))]);
        assert_eq!(won.badge(BRIEF), Some(Badge::Override));

        // A copy that is not what is sent — an empty placeholder — reads
        // differently, because "my override does nothing" is the thing worth
        // saying.
        let unused = layering(&[(
            BRIEF,
            &["acme", "store:okena-defaults"],
            Some("store:okena-defaults"),
        )]);
        assert_eq!(unused.badge(BRIEF), Some(Badge::Default));

        // And so does one where every copy is empty and nothing applies.
        let nothing = layering(&[(BRIEF, &["acme"], None)]);
        assert_eq!(nothing.badge(BRIEF), Some(Badge::Default));

        assert_eq!(Badge::Override.label(), "override");
        assert_eq!(Badge::Default.label(), "default");
    }

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
