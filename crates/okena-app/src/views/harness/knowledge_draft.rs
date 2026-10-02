//! "New" in a knowledge or a freeform origin: say what to write, pick where
//! and with which agent, and okena opens an agent session there — briefed on
//! the store layout in a knowledge origin, and to follow what the folder
//! already does in a freeform one, which has no layout.
//!
//! It stands in the entry panel rather than taking the whole view, the way a
//! new spec change and a new task do. Taking the view hid the entries you are
//! meant to read before adding to them.

use super::HarnessPane;
use crate::theme::theme;
use crate::ui::tokens::ui_text;
use crate::views::components::source_editor::BriefInput;
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::harness::AgentPurpose;
use okena_core::library::{LibraryOrigins, LibraryTree, OriginKind, OriginType};
use okena_ui::agent_launcher::Launch;
use std::collections::{HashMap, HashSet};

/// State of the "Write with an agent" form.
pub(crate) struct DraftForm {
    pub(crate) open: bool,
    /// Which origins it writes in: knowledge or freeform, as the origin it
    /// was opened from.
    pub(crate) origin_type: OriginType,
    pub(crate) request: BriefInput,
    /// Root to write in. Follows the open root until picked in the form.
    pub(crate) root: Option<String>,
    pub(crate) starting: bool,
    pub(crate) error: Option<String>,
    /// What the last start did, shown above the view once the form closes.
    pub(crate) notice: Option<String>,
    /// The files each draft's root had when it started, by daemon project
    /// id — how its Drafting row knows the files it wrote have shown up.
    pub(crate) baselines: HashMap<String, HashSet<String>>,
    /// Projects and context for the agent, once its dialog has been opened.
    pub(crate) pickers: Option<Entity<crate::views::components::launch_pickers::LaunchPickers>>,
}

impl DraftForm {
    pub(crate) fn new() -> Self {
        let request = BriefInput::new(
            "e.g. document how CI caches dependencies, and when to bust the cache",
        );
        Self {
            open: false,
            origin_type: OriginType::Knowledge,
            request,
            root: None,
            starting: false,
            error: None,
            notice: None,
            baselines: HashMap::new(),
            pickers: None,
        }
    }
}

impl HarnessPane {
    /// Open the form for origins of `origin_type`: knowledge or freeform.
    pub(super) fn open_draft(&mut self, origin_type: OriginType, cx: &mut Context<Self>) {
        // The form takes the entry panel, so nothing is selected while it is
        // open: a highlighted entry whose text you cannot see reads as a bug.
        // Unsaved edits to it are kept, and come back when it is reopened.
        self.library.leave_selection();
        self.roots.open = false;
        self.library.composing = false;
        self.knowledge_draft.open = true;
        self.knowledge_draft.origin_type = origin_type;
        self.knowledge_draft.root = None;
        self.knowledge_draft.error = None;
        self.knowledge_draft.notice = None;
        cx.notify();
    }

    /// Shut the form, leaving the panel on the root overview.
    pub(super) fn close_knowledge_draft(&mut self, cx: &mut Context<Self>) {
        self.knowledge_draft.open = false;
        self.knowledge_draft.root = None;
        self.knowledge_draft.error = None;
        cx.notify();
    }

    /// Where the draft goes: the root picked in the form, else the open one.
    fn knowledge_draft_target(&self) -> Option<String> {
        self.knowledge_draft.root.clone().or_else(|| {
            // The open origin, when it is of the form's type and not okena's
            // own — falling back to one nothing can be drafted in would
            // preselect a chip that is not even offered.
            let origin_type = self.knowledge_draft.origin_type;
            self.library_open_origin()
                .filter(|o| o.origin_type == origin_type && o.writable())
                .map(|o| o.key.clone())
        })
    }

    fn start_knowledge_draft(
        &mut self,
        agent: String,
        model: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.knowledge_draft.starting {
            return;
        }
        let request = self.knowledge_draft.request.value(cx).trim().to_string();
        if request.is_empty() {
            self.knowledge_draft.error = Some("Say what to write first.".into());
            cx.notify();
            return;
        }
        self.knowledge_draft.starting = true;
        self.knowledge_draft.error = None;
        cx.notify();

        let client = self.client.clone();
        let root = self.knowledge_draft_target();
        let context =
            super::context_dialog::picked_context(self.knowledge_draft.pickers.as_ref(), cx);
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::LibraryDraft {
                        context,
                        root,
                        request,
                        name: None,
                        agent_command: Some(agent),
                        model,
                    })
                    .and_then(|v| v.ok_or_else(|| "Missing draft result".to_string()))
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.knowledge_draft.starting = false;
                    match result {
                        Ok(v) => {
                            this.knowledge_draft.open = false;
                            this.knowledge_draft.root = None;
                            this.knowledge_draft.request.clear();
                            super::context_dialog::clear_picked_context(this.knowledge_draft.pickers.clone(), cx);
                            let name = v
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("the session");
                            // What the root holds now, so its Drafting row can
                            // tell when the files it writes have shown up.
                            let before: Option<HashSet<String>> = match this.library.tree.as_ref() {
                                Some(LibraryTree::Knowledge(tree)) => {
                                    Some(tree.entries.iter().map(|e| e.path.clone()).collect())
                                }
                                Some(LibraryTree::Freeform(tree)) => {
                                    Some(tree.documents.iter().map(|d| d.path.clone()).collect())
                                }
                                _ => None,
                            };
                            if let (Some(id), Some(before)) =
                                (v.get("project_id").and_then(|p| p.as_str()), before)
                                && this.library.root_key.as_deref()
                                    == v.get("root").and_then(|r| r.as_str())
                            {
                                this.knowledge_draft.baselines.insert(id.to_string(), before);
                            }
                            // The session runs in its own terminal; its row in
                            // the tree opens it, and what it writes shows up
                            // here on the next refresh.
                            this.knowledge_draft.notice = Some(format!(
                                "Started {name} — it shows as Drafting in the tree. Refresh to see what it writes."
                            ));
                        }
                        Err(e) => this.knowledge_draft.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(super) fn render_knowledge_draft_form(
        &self,
        origins: &LibraryOrigins,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);

        let origin_type = self.knowledge_draft.origin_type;
        let freeform = origin_type == OriginType::Freeform;
        let target = self.knowledge_draft_target();
        let mut roots = h_flex().gap(px(6.0)).flex_wrap();
        // Origins of the form's own type only: the brief is that type's. And
        // never okena's own store: it is rewritten on every start, so an
        // agent briefed to write there would lose its work.
        for root in origins.of_type(origin_type).filter(|o| o.writable()) {
            let key = root.key.clone();
            let kind = root.kind.label();
            roots = roots.child(self.choice_chip(
                format!("knowledge-draft-root-{}", root.key),
                format!("{} · {kind}", root.name),
                target.as_deref() == Some(root.key.as_str()),
                move |this, _cx| this.knowledge_draft.root = Some(key.clone()),
                cx,
            ));
        }
        let target_hint = match target.as_deref().and_then(|k| origins.origin(k)) {
            Some(root) if !freeform && root.kind == OriginKind::Store => format!(
                "The agent works in {} on a new knowledge/… branch, commits when done, and does not push unless you ask.",
                root.path
            ),
            Some(root) => format!(
                "The agent works in {}. Committing is left to you.",
                root.path
            ),
            None if freeform => "Pick the folder to write in.".to_string(),
            None => "Pick where the knowledge should live.".to_string(),
        };

        let starting = self.knowledge_draft.starting;
        let mut body = v_flex()
            .id("knowledge-draft-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .gap(px(14.0))
            .px(px(16.0))
            .py(px(14.0))
            .child(
                v_flex()
                    .gap(px(4.0))
                    .child(
                        // Cancel belongs with the heading, not beside the
                        // launcher: leaving the form and starting an agent are
                        // opposite intents, and putting them side by side made
                        // the destructive one a neighbour of the one you came
                        // to press.
                        h_flex()
                            .w_full()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(ui_text(15.0, cx))
                                    .text_color(rgb(t.text_primary))
                                    .child("Write with an agent"),
                            )
                            .child(self.small_button(
                                "knowledge-draft-cancel",
                                "Cancel",
                                cx.listener(|this, _, _window, cx| {
                                    this.close_knowledge_draft(cx);
                                }),
                                cx,
                            )),
                    )
                    .child(self.field_hint(
                        if freeform {
                            "okena opens an agent session in the folder, briefed to read what \
                             is there and follow its folders, names and conventions — a \
                             freeform origin has no layout of its own."
                        } else {
                            "okena opens an agent session in the knowledge origin, briefed on its \
                             layout — docs/, skills/, agents/ and templates/ — and on the \
                             frontmatter people and agents pick entries by."
                        },
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .gap(px(6.0))
                    .child(self.field_label("Where", cx))
                    .child(roots)
                    .child(self.field_hint(&target_hint, cx)),
            )
            .child(
                v_flex()
                    .gap(px(5.0))
                    .child(self.field_label("What to write", cx))
                    .child(self.knowledge_draft.request.render(110.0, cx))
                    .child(self.field_hint(
                        if freeform {
                            "A new document, or what to change across the ones there. This is \
                             the agent's brief, so context beats brevity."
                        } else {
                            "A new doc, a skill, a subagent or a template — or what to change in \
                             one. This is the agent's brief, so context beats brevity."
                        },
                        cx,
                    )),
            );

        if let Some(err) = self.knowledge_draft.error.clone() {
            body = body.child(self.error_banner(err, cx));
        }

        // No "without an agent" here: writing is the agent's whole job, and
        // an empty entry is something you can make without okena.
        let launcher = okena_ui::agent_launcher::AgentLauncher::new(
            "knowledge-draft-launcher",
            match target.as_deref().and_then(|k| origins.origin(k)) {
                Some(root) => format!("Write in {}", root.name),
                None => "Write with an agent".to_string(),
            },
        )
        .options(crate::views::agent_session::launch_options(
            self.tasks.default_agent.as_deref(),
            &t,
        ))
        .preferred(self.tasks.default_agent.clone())
        // The drafts already writing into this root.
        .sessions(self.launcher_sessions(
            self.sessions_for(cx, |purpose| {
                matches!(
                    purpose,
                    AgentPurpose::KnowledgeDraft { .. } | AgentPurpose::FreeformDraft { .. }
                ) && purpose.origin() == target
            }),
            cx,
        ))
        .launch_alongside_sessions()
        .busy(starting.then_some("Starting…"))
        // Projects and context are picked in a dialog, not on the form.
        .on_configure(
            "Choose projects and context…",
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.open_context_dialog(super::context_dialog::ContextTarget::KnowledgeDraft, cx);
            }),
        )
        .brief(crate::views::launch_briefs::brief_for(
            &self.client,
            if freeform { "freeform-draft" } else { "knowledge-draft" },
            cx,
        ))
        .on_open_brief(self.open_brief())
        .on_launch(cx.listener(|this, launch: &Launch, _window, cx| {
            this.start_knowledge_draft(launch.command.to_string(), launch.model.clone(), cx);
        }))
        .on_open(cx.listener(|this, id: &SharedString, _window, cx| {
            this.open_session(id.to_string(), cx);
        }));

        body = body.child(launcher);

        v_flex()
            .id("knowledge-draft-form")
            .flex_1()
            .min_w_0()
            .h_full()
            .border_l_1()
            .border_color(rgb(t.border))
            .child(body)
            .into_any_element()
    }
}
