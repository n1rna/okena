//! Agents on documents: the card under an open spec or knowledge file that
//! starts one to change it, and the Drafting rows a new change or a knowledge
//! draft shows in the tree while its agent writes.
//!
//! Every card and row lists only the sessions started for its own target, read
//! from the purpose the daemon records when it starts one. A session shows
//! where it belongs rather than in a catch-all list under the tree, which
//! listed every spec or knowledge session whatever it was writing.

use super::{HarnessPane, HarnessSection};
use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text_md, ui_text_ms};
use crate::views::components::source_editor::BriefInput;
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use okena_core::api::ActionRequest;
use okena_core::harness::AgentPurpose;
use okena_core::knowledge::KnowledgeTree;
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
/// root `root` of `section`.
///
/// A change's drafting agent belongs on every file of that change: that is
/// what it is writing, and once those files are in the tree the Drafting row
/// that stood in for them is gone.
pub(crate) fn serves_document(
    section: HarnessSection,
    purpose: &AgentPurpose,
    root: &str,
    path: &str,
) -> bool {
    match (section, purpose) {
        (HarnessSection::Specs, AgentPurpose::SpecEdit { root: r, path: p })
        | (HarnessSection::Knowledge, AgentPurpose::KnowledgeEdit { root: r, path: p }) => {
            r == root && p == path
        }
        (HarnessSection::Specs, AgentPurpose::SpecDraft { root: r, change }) => {
            in_root(r, root) && change_of(path) == Some(change.as_str())
        }
        _ => false,
    }
}

/// Whether a draft recorded in `recorded` is in `root`. A draft from before
/// the root was recorded has none, and is shown wherever its change is.
fn in_root(recorded: &str, root: &str) -> bool {
    recorded.is_empty() || recorded == root
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
    before.is_some_and(|before| tree.entries.iter().any(|e| !before.contains(&e.path)))
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

    fn doc_refine_mut(&mut self, section: HarnessSection) -> &mut DocRefine {
        match section {
            HarnessSection::Specs => &mut self.spec_refine,
            _ => &mut self.knowledge_refine,
        }
    }

    /// Root key and path of the document `section` has open, and whether it
    /// holds unsaved edits.
    fn open_document(&self, section: HarnessSection) -> Option<(String, String, bool)> {
        let (root, path, documents) = match section {
            HarnessSection::Specs => (
                self.specs.root_key.clone()?,
                self.specs.selected.clone()?,
                &self.specs.documents,
            ),
            HarnessSection::Knowledge => (
                self.knowledge.root_key.clone()?,
                self.knowledge.selected.clone()?,
                &self.knowledge.documents,
            ),
            HarnessSection::Tasks | HarnessSection::Testing => return None,
        };
        let dirty = documents.is_dirty(&root, &path);
        Some((root, path, dirty))
    }

    /// The card under an open document: say what to change, start an agent on
    /// it, and see the agents already on this file.
    pub(super) fn render_document_agent(
        &self,
        section: HarnessSection,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (root, path, dirty) = self.open_document(section)?;
        let t = theme(cx);
        let state = match section {
            HarnessSection::Specs => &self.spec_refine,
            _ => &self.knowledge_refine,
        };
        let sessions = self.launcher_sessions(
            self.sessions_for(cx, |purpose| {
                serves_document(section, purpose, &root, &path)
            }),
            cx,
        );
        // A request can run to a few lines, so the box shows a few.
        let request = state.request.render(72.0, cx);

        Some(
            okena_ui::agent_launcher::AgentLauncher::new(
                format!("{}-document-agent", section.slug()),
                "Refine with agent",
            )
            .subtitle("Changes only this file, and commits nothing")
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
                    let target = match section {
                        HarnessSection::Specs => super::context_dialog::ContextTarget::SpecRefine,
                        _ => super::context_dialog::ContextTarget::KnowledgeRefine,
                    };
                    this.open_context_dialog(target, cx);
                }),
            )
            .brief(crate::views::launch_briefs::brief_for(
                &self.client,
                "doc-refine",
                cx,
            ))
            .on_launch(cx.listener(move |this, launch: &Launch, _window, cx| {
                this.start_document_refine(
                    section,
                    launch.command.to_string(),
                    launch.model.clone(),
                    cx,
                );
            }))
            .on_open(cx.listener(|this, id: &SharedString, _window, cx| {
                this.open_session(id.to_string(), cx);
            }))
            .into_any_element(),
        )
    }

    fn start_document_refine(
        &mut self,
        section: HarnessSection,
        agent: String,
        model: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some((root, path, dirty)) = self.open_document(section) else {
            return;
        };
        if self.doc_refine_mut(section).starting {
            return;
        }
        // Checked again here, not only on the card: a save can fail between
        // the frame that drew the button and the click.
        if dirty {
            self.report_error("Save your edits first.", cx);
            return;
        }
        let request = self.doc_refine_mut(section).request.value(cx).trim().to_string();
        if request.is_empty() {
            self.report_error("Say what to change first.", cx);
            return;
        }
        self.doc_refine_mut(section).starting = true;
        cx.notify();

        let agent_command = Some(agent);
        let context = super::context_dialog::picked_context(
            self.doc_refine_mut(section).pickers.as_ref(),
            cx,
        );
        let action = match section {
            HarnessSection::Specs => ActionRequest::SpecRefineDocument {
                context,
                root,
                path,
                request,
                agent_command,
                model,
            },
            _ => ActionRequest::KnowledgeRefineDocument {
                context,
                root,
                path,
                request,
                agent_command,
                model,
            },
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
                    let state = this.doc_refine_mut(section);
                    state.starting = false;
                    let pickers = state.pickers.clone();
                    // Not opening it: the card lists it, and you are reading
                    // the file it is about to change.
                    match result {
                        Ok(_) => {
                            this.doc_refine_mut(section).request.clear();
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
        let root = self.specs.root_key.clone().unwrap_or_default();
        self.sessions_for(cx, |purpose| {
            matches!(purpose, AgentPurpose::SpecDraft { root: r, change: c }
                if in_root(r, &root) && *c == change.name)
        })
        .iter()
        .filter_map(|id| self.render_drafting_row(id, change.name.clone(), 12.0, cx))
        .collect()
    }

    /// Drafting rows for knowledge drafts into the open root whose files have
    /// not shown up yet.
    pub(super) fn render_knowledge_drafts(
        &self,
        tree: &KnowledgeTree,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(root) = self.knowledge.root_key.clone() else {
            return Vec::new();
        };
        let ids = self.sessions_for(
            cx,
            |purpose| matches!(purpose, AgentPurpose::KnowledgeDraft { root: r } if *r == root),
        );
        ids.iter()
            .filter(|id| {
                !draft_landed(
                    self.knowledge_draft.baselines.get(&self.daemon_id(id)),
                    tree,
                )
            })
            .filter_map(|id| {
                // The request, which is what the user will recognize it by.
                let title = self
                    .workspace
                    .read(cx)
                    .project(id)
                    .and_then(|p| p.custom_session.clone())
                    .map(|goal| {
                        goal.strip_prefix("Knowledge: ")
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
    use super::{change_of, draft_landed, only_scaffolded, serves_document};
    use okena_core::harness::{AgentPurpose, HarnessSection};
    use okena_core::knowledge::{KnowledgeEntry, KnowledgeKind, KnowledgeTree};
    use okena_core::specs::{SpecChange, SpecDoc};
    use std::collections::HashSet;

    const PROPOSAL: &str = "openspec/changes/add-login/proposal.md";

    #[test]
    fn a_document_card_lists_only_its_own_file() {
        let edit = AgentPurpose::SpecEdit {
            root: "store:plans".into(),
            path: "openspec/specs/auth/spec.md".into(),
        };
        let s = HarnessSection::Specs;
        assert!(serves_document(
            s,
            &edit,
            "store:plans",
            "openspec/specs/auth/spec.md"
        ));
        assert!(!serves_document(
            s,
            &edit,
            "store:plans",
            "openspec/specs/billing/spec.md"
        ));
        assert!(!serves_document(
            s,
            &edit,
            "store:other",
            "openspec/specs/auth/spec.md"
        ));
        // The same root key and path in the other section is another file.
        assert!(!serves_document(
            HarnessSection::Knowledge,
            &edit,
            "store:plans",
            "openspec/specs/auth/spec.md"
        ));
    }

    #[test]
    fn a_change_s_drafting_agent_is_on_every_file_of_that_change() {
        let draft = AgentPurpose::SpecDraft {
            root: "store:plans".into(),
            change: "add-login".into(),
        };
        let s = HarnessSection::Specs;
        assert!(serves_document(s, &draft, "store:plans", PROPOSAL));
        assert!(serves_document(
            s,
            &draft,
            "store:plans",
            "openspec/changes/add-login/specs/auth/spec.md"
        ));
        assert!(!serves_document(
            s,
            &draft,
            "store:plans",
            "openspec/changes/add-login-v2/proposal.md"
        ));
        // A draft from before roots were recorded is shown in any root.
        let legacy = AgentPurpose::SpecDraft {
            root: String::new(),
            change: "add-login".into(),
        };
        assert!(serves_document(s, &legacy, "store:other", PROPOSAL));
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
