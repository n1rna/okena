//! Agents on Library documents: the card under an open document that starts
//! one to change it, and the Drafting rows a new change, a knowledge draft or
//! a freeform draft shows in the tree while its agent writes.
//!
//! Every card and row lists only the sessions started for its own target, read
//! from the purpose the daemon records when it starts one. A session shows
//! where it belongs rather than in a catch-all list under the tree, which
//! listed every spec or knowledge session whatever it was writing.

use super::HarnessPane;
use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text_md, ui_text_ms};
use crate::views::components::source_editor::BriefInput;
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use okena_core::api::ActionRequest;
use okena_core::harness::AgentPurpose;
use okena_core::knowledge::KnowledgeTree;
use okena_core::library::FreeformTree;
use okena_core::specs::SpecChange;
use okena_ui::agent_launcher::Launch;
use std::collections::HashSet;

/// What a document's agent card holds between frames.
pub(crate) struct DocRefine {
    /// What to change, typed on the card.
    pub(crate) request: BriefInput,
    /// A start is in flight, which blocks a second one.
    pub(crate) starting: bool,
    /// Projects and context for the agent, once its dialog has been opened.
    pub(crate) pickers: Option<Entity<crate::views::components::launch_pickers::LaunchPickers>>,
}

impl DocRefine {
    pub(crate) fn new() -> Self {
        Self {
            request: BriefInput::new("What should change in this file?"),
            starting: false,
            pickers: None,
        }
    }
}

/// Whether a session started for `purpose` belongs on the card of `path` in
/// the origin keyed `root`.
///
/// The purpose names its origin by Library key, so a spec session can never
/// land on a knowledge file that happens to share a store id and a path.
///
/// A change's drafting agent belongs on every file of that change: that is
/// what it is writing, and once those files are in the tree the Drafting row
/// that stood in for them is gone.
pub(crate) fn serves_document(purpose: &AgentPurpose, root: &str, path: &str) -> bool {
    match purpose {
        AgentPurpose::SpecEdit { path: p, .. }
        | AgentPurpose::KnowledgeEdit { path: p, .. }
        | AgentPurpose::FreeformEdit { path: p, .. } => {
            purpose.origin().as_deref() == Some(root) && p == path
        }
        AgentPurpose::SpecDraft { change, .. } => {
            drafts_in(purpose, root) && change_of(path) == Some(change.as_str())
        }
        _ => false,
    }
}

/// Whether the spec draft `purpose` is in the origin keyed `root`. A draft
/// from before its origin was recorded names none, and is shown wherever its
/// change is — in a spec origin, since that is all a change can be in.
pub(crate) fn drafts_in(purpose: &AgentPurpose, root: &str) -> bool {
    match purpose.origin() {
        Some(origin) => origin == root,
        None => {
            okena_core::library::split_key(root).map(|(t, _)| t)
                == Some(okena_core::library::OriginType::Spec)
        }
    }
}

/// The change a path of a spec root is inside, if any. Archived changes are
/// history, and nothing drafts into them.
fn change_of(path: &str) -> Option<&str> {
    path.strip_prefix("openspec/changes/")?
        .split_once('/')
        .map(|(change, _)| change)
        .filter(|change| *change != "archive")
}

/// Whether a change still holds only what okena scaffolded: the proposal stub.
/// Anything else in it was written by its agent.
pub(crate) fn only_scaffolded(change: &SpecChange) -> bool {
    change.specs.is_empty() && change.artifacts.iter().all(|d| d.name == "proposal.md")
}

/// Whether a knowledge draft's files have shown up: the tree lists an entry
/// that was not there when it started. Without a record of what was there — a
/// draft started elsewhere, or before a restart — it cannot tell, and says no.
pub(crate) fn draft_landed(before: Option<&HashSet<String>>, tree: &KnowledgeTree) -> bool {
    any_new(before, tree.entries.iter().map(|e| e.path.as_str()))
}

/// The same for a freeform draft: the tree lists a document that was not
/// there when it started.
pub(crate) fn freeform_draft_landed(before: Option<&HashSet<String>>, tree: &FreeformTree) -> bool {
    any_new(before, tree.documents.iter().map(|d| d.path.as_str()))
}

fn any_new<'a>(before: Option<&HashSet<String>>, mut now: impl Iterator<Item = &'a str>) -> bool {
    before.is_some_and(|before| now.any(|path| !before.contains(path)))
}

impl HarnessPane {
    /// Sessions whose purpose satisfies `wanted`, by project id.
    pub(super) fn sessions_for(
        &self,
        cx: &App,
        wanted: impl Fn(&AgentPurpose) -> bool,
    ) -> Vec<String> {
        self.workspace
            .read(cx)
            .projects()
            .iter()
            .filter(|p| p.purpose().is_some_and(|purpose| wanted(&purpose)))
            .map(|p| p.id.clone())
            .collect()
    }

    /// Origin key and path of the open document, and whether it holds
    /// unsaved edits.
    fn open_document(&self) -> Option<(String, String, bool)> {
        let root = self.library.root_key.clone()?;
        let path = self.library.selected.clone()?;
        let dirty = self.library.documents.is_dirty(&root, &path);
        Some((root, path, dirty))
    }

    /// The card under an open document: say what to change, start an agent on
    /// it, and see the agents already on this file.
    pub(super) fn render_document_agent(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (root, path, dirty) = self.open_document()?;
        let t = theme(cx);
        let state = &self.refine;
        let sessions = self.launcher_sessions(
            self.sessions_for(cx, |purpose| serves_document(purpose, &root, &path)),
            cx,
        );
        // A request can run to a few lines, so the box shows a few.
        let request = state.request.render(72.0, cx);

        Some(
            okena_ui::agent_launcher::AgentLauncher::new(
                "library-document-agent",
                "Refine with agent",
            )
            .options(crate::views::agent_session::launch_options(
                self.tasks.default_agent.as_deref(),
                &t,
            ))
            .preferred(self.tasks.default_agent.clone())
            .body(request)
            .sessions(sessions)
            // Each asks for something different, so one running is no reason
            // to hide the way to ask for another.
            .launch_alongside_sessions()
            .busy(state.starting.then_some("Starting…"))
            // The agent writes the file on disk; edits held here would
            // either overwrite its work on save or be lost under it.
            .disabled(dirty.then_some("Save your edits first"))
            // Projects and context are picked in a dialog, not on the card.
            .on_configure(
                "Choose projects and context…",
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.open_context_dialog(super::context_dialog::ContextTarget::Refine, cx);
                }),
            )
            .brief(crate::views::launch_briefs::brief_for(
                &self.client,
                "doc-refine",
                cx,
            ))
            .on_open_brief(self.open_brief())
            .on_launch(cx.listener(move |this, launch: &Launch, _window, cx| {
                this.start_document_refine(launch.command.to_string(), launch.model.clone(), cx);
            }))
            .on_open(cx.listener(|this, id: &SharedString, _window, cx| {
                this.open_session(id.to_string(), cx);
            }))
            .into_any_element(),
        )
    }

    fn start_document_refine(
        &mut self,
        agent: String,
        model: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some((root, path, dirty)) = self.open_document() else {
            return;
        };
        if self.refine.starting {
            return;
        }
        // Checked again here, not only on the card: a save can fail between
        // the frame that drew the button and the click.
        if dirty {
            self.report_error("Save your edits first.", cx);
            return;
        }
        let request = self.refine.request.value(cx).trim().to_string();
        if request.is_empty() {
            self.report_error("Say what to change first.", cx);
            return;
        }
        self.refine.starting = true;
        cx.notify();

        let agent_command = Some(agent);
        let context = super::context_dialog::picked_context(self.refine.pickers.as_ref(), cx);
        let action = ActionRequest::LibraryRefineDocument {
            context,
            root,
            path,
            request,
            agent_command,
            model,
        };
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(action)
                    .and_then(|v| v.ok_or_else(|| "Missing session result".to_string()))
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    let state = &mut this.refine;
                    state.starting = false;
                    let pickers = state.pickers.clone();
                    // Not opening it: the card lists it, and you are reading
                    // the file it is about to change.
                    match result {
                        Ok(_) => {
                            this.refine.request.clear();
                            super::context_dialog::clear_picked_context(pickers, cx);
                        }
                        Err(e) => this.report_error(e, cx),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// A session writing something that is not in the tree yet, shown where
    /// it will land: open it by clicking, or discard it.
    fn render_drafting_row(
        &self,
        project_id: &str,
        title: String,
        indent: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let t = theme(cx);
        let info = crate::views::agent_session::AgentSessionInfo::collect(
            self.workspace.read(cx),
            &self.terminals,
            project_id,
        )?;
        let activity = info.activity();
        let open = project_id.to_string();
        let discard = project_id.to_string();
        Some(
            h_flex()
                .id(SharedString::from(format!("drafting-{project_id}")))
                .cursor_pointer()
                .w_full()
                .min_w_0()
                .items_center()
                .gap(px(6.0))
                .pl(px(6.0 + indent))
                .pr(px(4.0))
                .py(px(3.0))
                .rounded(px(3.0))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .child(
                    div()
                        .flex_shrink_0()
                        .px(px(5.0))
                        .rounded(px(3.0))
                        .border_1()
                        .border_color(with_alpha(t.border_active, 0.6))
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child("Drafting"),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .size(px(6.0))
                        .rounded_full()
                        .bg(rgb(activity.color(&t))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(ui_text_md(cx))
                        .text_color(rgb(t.text_secondary))
                        .child(title),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("drafting-discard-{project_id}")))
                        .cursor_pointer()
                        .flex_shrink_0()
                        .px(px(4.0))
                        .rounded(px(3.0))
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .hover(|s| s.bg(rgb(t.bg_hover)).text_color(rgb(t.text_primary)))
                        .child("✕")
                        .tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new("Discard this draft")
                                .build(window, cx)
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                // Discarding is not opening it.
                                cx.stop_propagation();
                                this.discard_draft(discard.clone(), cx);
                            }),
                        ),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        this.open_session(open.clone(), cx);
                    }),
                )
                .into_any_element(),
        )
    }

    /// Drafting rows for `change`, while it still holds only its scaffold.
    pub(super) fn render_spec_drafts(
        &self,
        change: &SpecChange,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if change.archived || !only_scaffolded(change) {
            return Vec::new();
        }
        let root = self.library.root_key.clone().unwrap_or_default();
        self.sessions_for(cx, |purpose| {
            matches!(purpose, AgentPurpose::SpecDraft { change: c, .. } if *c == change.name)
                && drafts_in(purpose, &root)
        })
        .iter()
        .filter_map(|id| self.render_drafting_row(id, change.name.clone(), 12.0, cx))
        .collect()
    }

    /// Drafting rows for knowledge drafts into the open origin whose files
    /// have not shown up yet.
    pub(super) fn render_knowledge_drafts(
        &self,
        tree: &KnowledgeTree,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        self.render_draft_rows(
            |purpose| matches!(purpose, AgentPurpose::KnowledgeDraft { .. }),
            |before| draft_landed(before, tree),
            "Knowledge: ",
            cx,
        )
    }

    /// Drafting rows for freeform drafts into the open origin whose documents
    /// have not shown up yet.
    pub(super) fn render_freeform_drafts(
        &self,
        tree: &FreeformTree,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        self.render_draft_rows(
            |purpose| matches!(purpose, AgentPurpose::FreeformDraft { .. }),
            |before| freeform_draft_landed(before, tree),
            "Documents: ",
            cx,
        )
    }

    /// Drafting rows for the sessions of one kind writing into the open
    /// origin, until `landed` says what they wrote is in the tree.
    /// `goal_prefix` is what the daemon puts before the request in the
    /// session's goal.
    fn render_draft_rows(
        &self,
        is_draft: impl Fn(&AgentPurpose) -> bool,
        landed: impl Fn(Option<&HashSet<String>>) -> bool,
        goal_prefix: &str,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(root) = self.library.root_key.clone() else {
            return Vec::new();
        };
        let ids = self.sessions_for(cx, |purpose| {
            is_draft(purpose) && purpose.origin().as_deref() == Some(root.as_str())
        });
        ids.iter()
            .filter(|id| !landed(self.knowledge_draft.baselines.get(&self.daemon_id(id))))
            .filter_map(|id| {
                // The request, which is what the user will recognize it by.
                let title = self
                    .workspace
                    .read(cx)
                    .project(id)
                    .and_then(|p| p.custom_session.clone())
                    .map(|goal| {
                        goal.strip_prefix(goal_prefix)
                            .map(str::to_string)
                            .unwrap_or(goal)
                    })
                    .unwrap_or_default();
                self.render_drafting_row(id, title, 0.0, cx)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{
        change_of, draft_landed, drafts_in, freeform_draft_landed, only_scaffolded,
        serves_document,
    };
    use okena_core::harness::AgentPurpose;
    use okena_core::knowledge::{KnowledgeEntry, KnowledgeKind, KnowledgeTree};
    use okena_core::specs::{SpecChange, SpecDoc};
    use std::collections::HashSet;

    const PROPOSAL: &str = "openspec/changes/add-login/proposal.md";

    #[test]
    fn a_document_card_lists_only_its_own_file() {
        let edit = AgentPurpose::SpecEdit {
            root: "spec:store:plans".into(),
            path: "openspec/specs/auth/spec.md".into(),
        };
        assert!(serves_document(
            &edit,
            "spec:store:plans",
            "openspec/specs/auth/spec.md"
        ));
        assert!(!serves_document(
            &edit,
            "spec:store:plans",
            "openspec/specs/billing/spec.md"
        ));
        assert!(!serves_document(
            &edit,
            "spec:store:other",
            "openspec/specs/auth/spec.md"
        ));
        // The same store id and path in a knowledge origin is another file.
        assert!(!serves_document(
            &edit,
            "knowledge:store:plans",
            "openspec/specs/auth/spec.md"
        ));
    }

    #[test]
    fn a_freeform_document_s_card_lists_the_sessions_refining_it() {
        let edit = AgentPurpose::FreeformEdit {
            root: "freeform:path:/notes".into(),
            path: "workflows/release.md".into(),
        };
        assert!(serves_document(&edit, "freeform:path:/notes", "workflows/release.md"));
        assert!(!serves_document(&edit, "freeform:path:/notes", "README.md"));
        assert!(!serves_document(&edit, "freeform:path:/other", "workflows/release.md"));
        // A draft writes wherever it likes, so it is on no one document.
        let draft = AgentPurpose::FreeformDraft {
            root: "freeform:path:/notes".into(),
        };
        assert!(!serves_document(&draft, "freeform:path:/notes", "README.md"));
    }

    #[test]
    fn a_freeform_draft_has_landed_once_a_new_document_appears() {
        use okena_core::library::{FreeformDoc, FreeformTree};
        let tree = |paths: &[&str]| FreeformTree {
            documents: paths
                .iter()
                .map(|p| FreeformDoc {
                    path: (*p).to_string(),
                    title: (*p).to_string(),
                })
                .collect(),
            ..Default::default()
        };
        let before: HashSet<String> = ["README.md".to_string()].into();
        assert!(!freeform_draft_landed(Some(&before), &tree(&["README.md"])));
        assert!(freeform_draft_landed(
            Some(&before),
            &tree(&["README.md", "workflows/release.md"])
        ));
        // Nothing recorded: it cannot tell, and keeps the row.
        assert!(!freeform_draft_landed(None, &tree(&["README.md", "x.md"])));
    }

    #[test]
    fn a_session_from_before_typed_keys_still_lists_on_its_document() {
        // Recorded as `store:eng` by a session started before the Library;
        // the purpose's kind says it was a knowledge origin.
        let edit = AgentPurpose::KnowledgeEdit {
            root: "store:eng".into(),
            path: "docs/ci.md".into(),
        };
        assert!(serves_document(&edit, "knowledge:store:eng", "docs/ci.md"));
        assert!(!serves_document(&edit, "spec:store:eng", "docs/ci.md"));
    }

    #[test]
    fn a_change_s_drafting_agent_is_on_every_file_of_that_change() {
        let draft = AgentPurpose::SpecDraft {
            root: "spec:store:plans".into(),
            change: "add-login".into(),
        };
        assert!(serves_document(&draft, "spec:store:plans", PROPOSAL));
        assert!(serves_document(
            &draft,
            "spec:store:plans",
            "openspec/changes/add-login/specs/auth/spec.md"
        ));
        assert!(!serves_document(
            &draft,
            "spec:store:plans",
            "openspec/changes/add-login-v2/proposal.md"
        ));
        // A draft from before origins were recorded is shown in any spec
        // origin, and in no other type's.
        let legacy = AgentPurpose::SpecDraft {
            root: String::new(),
            change: "add-login".into(),
        };
        assert!(serves_document(&legacy, "spec:store:other", PROPOSAL));
        assert!(drafts_in(&legacy, "spec:path:/repo"));
        assert!(!drafts_in(&legacy, "knowledge:store:other"));
    }

    #[test]
    fn archived_changes_belong_to_no_draft() {
        assert_eq!(change_of(PROPOSAL), Some("add-login"));
        assert_eq!(
            change_of("openspec/changes/archive/2026-01-01-x/proposal.md"),
            None
        );
        assert_eq!(change_of("openspec/specs/auth/spec.md"), None);
    }

    fn doc(path: &str) -> SpecDoc {
        SpecDoc {
            path: path.into(),
            name: path.rsplit('/').next().unwrap_or(path).into(),
        }
    }

    #[test]
    fn a_change_is_only_scaffolded_until_its_agent_writes_something() {
        let mut change = SpecChange {
            name: "add-login".into(),
            path: "openspec/changes/add-login".into(),
            artifacts: vec![doc(PROPOSAL)],
            specs: Vec::new(),
            archived: false,
            schema: None,
            created: None,
        };
        assert!(only_scaffolded(&change));
        change
            .artifacts
            .push(doc("openspec/changes/add-login/design.md"));
        assert!(!only_scaffolded(&change));
    }

    fn tree(paths: &[&str]) -> KnowledgeTree {
        KnowledgeTree {
            entries: paths
                .iter()
                .map(|p| KnowledgeEntry {
                    kind: KnowledgeKind::Doc,
                    path: (*p).into(),
                    name: (*p).into(),
                    title: (*p).into(),
                    description: None,
                    tags: Vec::new(),
                    files: Vec::new(),
                    flows: Vec::new(),
                    variables: Vec::new(),
                    models: Default::default(),
                    status: Vec::new(),
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_knowledge_draft_has_landed_once_a_new_entry_appears() {
        let before: HashSet<String> = ["docs/a.md".to_string()].into();
        assert!(!draft_landed(Some(&before), &tree(&["docs/a.md"])));
        assert!(draft_landed(
            Some(&before),
            &tree(&["docs/a.md", "docs/ci.md"])
        ));
        // Nothing to compare against: keep showing it.
        assert!(!draft_landed(None, &tree(&["docs/ci.md"])));
    }
}
