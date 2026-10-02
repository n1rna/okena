//! What a `spec` origin holds, in the Library: an OpenSpec planning tree, and
//! the form that drafts a new change into one.
//!
//! Spec origins are whatever the daemon discovered the way the `openspec` CLI
//! would: stores registered on this machine, projects with their own
//! `openspec/` tree, folders from settings. The Library wraps them without
//! changing their format, so the tree here is OpenSpec's own shape — changes,
//! capabilities, the archive — not a folder listing.
//!
//! The page around this — the origin list, loading, the document panel — is
//! `library_view.rs`, shared with every other origin type.

use crate::theme::theme;
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms};
use crate::views::components::SimpleInput;
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::library::{LibraryOrigins, OriginKind, OriginType};
use okena_core::specs::{SpecChange, SpecDoc, SpecTree};
use okena_ui::agent_launcher::Launch;

use super::HarnessPane;

impl HarnessPane {
    /// The spec origin a new change goes into: the one picked in the form,
    /// else the open one when it is a spec origin.
    fn draft_target(&self) -> Option<String> {
        self.library.draft_root.clone().or_else(|| {
            self.library_open_origin()
                .filter(|o| o.origin_type == OriginType::Spec)
                .map(|o| o.key.clone())
        })
    }

    /// Scaffold the configured change, start `agent_command` on it, and return
    /// to the tree. An empty command scaffolds only.
    pub(super) fn draft_spec_change(
        &mut self,
        agent_command: String,
        model: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.library.drafting {
            return;
        }
        let idea = self.library.idea_input.value(cx).trim().to_string();
        if idea.is_empty() {
            self.library.error = Some("Describe the change first.".into());
            cx.notify();
            return;
        }
        let name = self.library.name_input.read(cx).value().trim().to_string();
        self.library.drafting = true;
        self.library.error = None;
        cx.notify();

        let client = self.client.clone();
        // An explicit empty string is the daemon's "scaffold only, no agent".
        let agent_command = Some(agent_command);
        let name = (!name.is_empty()).then_some(name);
        let root = self.draft_target();
        let context = super::context_dialog::picked_context(self.library.pickers.as_ref(), cx);
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::LibraryDraft {
                        context,
                        root,
                        request: idea,
                        name,
                        agent_command,
                        model,
                    })
                    .and_then(|v| v.ok_or_else(|| "Missing draft result".to_string()))
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.library.drafting = false;
                    match result {
                        Ok(v) => {
                            let change = v
                                .get("change")
                                .and_then(|c| c.as_str())
                                .unwrap_or("change")
                                .to_string();
                            // Back to the specs: the session runs in its own
                            // terminal, reachable from the sidebar, and the
                            // thing worth looking at here is the change itself.
                            this.library.composing = false;
                            this.library.draft_root = None;
                            this.library.idea_input.clear();
                            this.library
                                .name_input
                                .update(cx, |i, cx| i.set_value("", cx));
                            super::context_dialog::clear_picked_context(
                                this.library.pickers.clone(),
                                cx,
                            );
                            // Open the root it went into, with the change
                            // expanded: the user just made it.
                            if let Some(root) = v.get("root").and_then(|r| r.as_str()) {
                                this.library.leave_selection();
                                this.library.root_key = Some(root.to_string());
                            }
                            this.library.collapsed.remove(&change);
                            this.refresh_library(cx);
                            // Jump straight to the stub okena wrote, so there
                            // is something to read while the agent works.
                            if let Some(path) = v.get("path").and_then(|p| p.as_str()) {
                                this.open_library_file(format!("{path}/proposal.md"), cx);
                            }
                        }
                        Err(e) => this.library.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// One change directory: a disclosure row over its documents.
    fn render_change(&self, change: &SpecChange, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let collapsed = self.library.collapsed.contains(&change.name);
        let name = change.name.clone();
        // Artifacts then the change's delta specs, which is the order they are
        // written in and the order they are read in.
        let docs: Vec<SpecDoc> = change
            .artifacts
            .iter()
            .chain(change.specs.iter())
            .cloned()
            .collect();

        let mut col = v_flex().w_full().min_w_0().child(
            h_flex()
                .id(SharedString::from(format!("spec-change-{}", change.name)))
                .cursor_pointer()
                .w_full()
                .min_w_0()
                .items_center()
                .gap(px(4.0))
                .px(px(6.0))
                .py(px(3.0))
                .rounded(px(3.0))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .child(
                    div()
                        .w(px(10.0))
                        .flex_shrink_0()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child(if collapsed { "›" } else { "⌄" }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(ui_text_md(cx))
                        .text_color(rgb(t.text_primary))
                        .child(change.name.clone()),
                )
                .when_some(
                    change.created.clone().filter(|_| !change.archived),
                    |d, created| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_muted))
                                .child(created),
                        )
                    },
                )
                // Count rather than a spinner: it says at a glance whether the
                // agent has written anything yet.
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child(format!("{}", docs.len())),
                )
                // Archived changes are history; nothing is added to them.
                .children((!change.archived).then(|| {
                    let change_path = change.path.clone();
                    self.add_button(
                        SharedString::from(format!("spec-new-doc-{}", change.name)),
                        "New document",
                        move |this, window, cx| {
                            this.open_new_form(
                                super::file_ops::NewItem::SpecDocument {
                                    change: change_path.clone(),
                                },
                                window,
                                cx,
                            )
                        },
                        cx,
                    )
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        this.toggle_library_fold(name.clone(), cx);
                    }),
                ),
        );
        // At the folder okena scaffolded, whether or not it is folded: an
        // agent writing into it is worth seeing either way.
        col = col.children(self.render_spec_drafts(change, cx));
        if !collapsed {
            for doc in &docs {
                col = col.child(self.render_document_row(
                    &doc.path,
                    &doc.name,
                    22.0,
                    Vec::new(),
                    cx,
                ));
            }
            if docs.is_empty() {
                col = col.child(
                    div()
                        .pl(px(22.0))
                        .py(px(2.0))
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child("no artifacts yet"),
                );
            }
        }
        col.into_any_element()
    }

    /// The rows under the origin list for a spec origin: its changes, its
    /// capabilities and its archive.
    pub(super) fn render_spec_entries(
        &self,
        tree: &SpecTree,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        use super::file_ops::NewItem;
        let mut rows = Vec::new();
        if !tree.initialized {
            rows.push(self.muted_line(
                "No openspec/ directory yet — drafting a change creates one.",
                cx,
            ));
        }

        let creating = self.file_ops.creating.clone();
        rows.push(self.tree_heading_with_add(
            "Changes",
            "spec-new-change-folder",
            "New change",
            |this, window, cx| this.open_new_form(NewItem::SpecChange, window, cx),
            cx,
        ));
        if matches!(
            creating,
            Some(NewItem::SpecChange | NewItem::SpecDocument { .. })
        ) {
            rows.extend(self.render_new_form(cx));
        }
        if tree.changes.is_empty() {
            rows.push(self.muted_line("No changes in flight.", cx));
        }
        for change in &tree.changes {
            rows.push(self.render_change(change, cx));
        }

        rows.push(self.tree_heading_with_add(
            "Specs",
            "spec-new-capability",
            "New spec",
            |this, window, cx| this.open_new_form(NewItem::SpecCapability, window, cx),
            cx,
        ));
        if matches!(creating, Some(NewItem::SpecCapability)) {
            rows.extend(self.render_new_form(cx));
        }
        if tree.specs.is_empty() {
            rows.push(self.muted_line("No specs yet.", cx));
        }
        for doc in &tree.specs {
            rows.push(self.render_document_row(&doc.path, &doc.name, 10.0, Vec::new(), cx));
        }

        // Archived changes are history, so they are listed but never in the
        // way: the section only appears once something has been archived.
        if !tree.archived.is_empty() {
            rows.push(self.section_label("Archive", cx));
            for change in &tree.archived {
                rows.push(self.render_change(change, cx));
            }
        }
        // Agents are listed where they work: a draft in its change's folder,
        // and every agent on a file on that file's card.
        rows
    }

    /// The full-view new-change form: where, name, prompt, agent.
    pub(super) fn render_new_change_form(
        &self,
        origins: &LibraryOrigins,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);

        let target = self.draft_target();
        let mut roots = h_flex().gap(px(6.0)).flex_wrap();
        // Spec origins only: a change is an OpenSpec thing.
        for root in origins.of_type(OriginType::Spec).filter(|o| o.healthy) {
            let key = root.key.clone();
            roots = roots.child(self.choice_chip(
                format!("spec-target-{}", root.key),
                format!("{} · {}", root.name, root.kind.label()),
                target.as_deref() == Some(root.key.as_str()),
                move |this, _cx| this.library.draft_root = Some(key.clone()),
                cx,
            ));
        }
        let target_hint = match target.as_deref().and_then(|k| origins.origin(k)) {
            Some(root) => match (root.kind, root.store_id.as_deref()) {
                (OriginKind::Store, Some(id)) => format!(
                    "Drafted in store '{id}' at {}. The agent is told to pass --store {id} to the openspec CLI.",
                    root.path
                ),
                _ => format!("Drafted under openspec/changes/ in {}.", root.path),
            },
            None => "Pick where the change should live.".to_string(),
        };

        let drafting = self.library.drafting;
        let mut body = v_flex()
            .id("spec-new-change-body")
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
                                    .child("New change"),
                            )
                            .child(self.small_button(
                                "spec-cancel",
                                "Cancel",
                                cx.listener(move |this, _, _window, cx| {
                                    this.close_new_change(cx);
                                }),
                                cx,
                            )),
                    )
                    .child(self.field_hint(
                        "okena scaffolds the change under openspec/changes/ — the \
                         .openspec.yaml openspec new change writes, plus a stub \
                         proposal — and starts an agent briefed on the OpenSpec \
                         conventions.",
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
                    .child(self.field_label("Change name", cx))
                    .child(
                        okena_ui::input::input_container(&t, None)
                            .w_full()
                            .px(px(8.0))
                            .py(px(6.0))
                            .child(
                                SimpleInput::new(&self.library.name_input)
                                    .text_size(ui_text(13.0, cx)),
                            ),
                    )
                    .child(self.field_hint(
                        "Becomes the directory name. Leave blank to derive one \
                         from the prompt.",
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .gap(px(5.0))
                    .child(self.field_label("Prompt", cx))
                    .child(self.library.idea_input.render(110.0, cx))
                    .child(self.field_hint(
                        "What the change is for. This is what the agent is \
                         briefed with, so context beats brevity.",
                        cx,
                    )),
            );

        if let Some(err) = self.library.error.clone() {
            body = body.child(self.error_banner(err, cx));
        }

        let mut options =
            crate::views::agent_session::launch_options(self.tasks.default_agent.as_deref(), &t);
        // Last: scaffolding without an agent is a legitimate choice, not the
        // one most people came for.
        options.push(crate::views::agent_session::no_agent_option(
            "Scaffold only",
            &t,
        ));
        let launcher = okena_ui::agent_launcher::AgentLauncher::new(
            "spec-launcher",
            match target.as_deref().and_then(|k| origins.origin(k)) {
                Some(root) => format!("Draft the change in {}", root.name),
                None => "Draft the change".to_string(),
            },
        )
        .options(options)
        .preferred(self.tasks.default_agent.clone())
        // The drafts already going into this root, so a second idea is not
        // started blind.
        .sessions(self.launcher_sessions(
            self.sessions_for(cx, |purpose| {
                matches!(purpose, okena_core::harness::AgentPurpose::SpecDraft { .. })
                    && target
                        .as_deref()
                        .is_some_and(|t| super::doc_agents::drafts_in(purpose, t))
            }),
            cx,
        ))
        .launch_alongside_sessions()
        .busy(drafting.then_some("Starting…"))
        // Projects and context are picked in a dialog, not on the form.
        .on_configure(
            "Choose projects and context…",
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.open_context_dialog(super::context_dialog::ContextTarget::SpecDraft, cx);
            }),
        )
        .brief(crate::views::launch_briefs::brief_for(
            &self.client,
            "spec-draft",
            cx,
        ))
        .on_open_brief(self.open_brief())
        .on_launch(cx.listener(|this, launch: &Launch, _window, cx| {
            this.draft_spec_change(launch.command.to_string(), launch.model.clone(), cx);
        }))
        .on_open(cx.listener(|this, id: &SharedString, _window, cx| {
            this.open_session(id.to_string(), cx);
        }));

        body = body.child(launcher);

        v_flex()
            .id("spec-new-change-form")
            .flex_1()
            .min_w_0()
            .h_full()
            .border_l_1()
            .border_color(rgb(t.border))
            .child(body)
            .into_any_element()
    }

    /// Shut the new-change form, leaving the panel on the root overview.
    fn close_new_change(&mut self, cx: &mut Context<Self>) {
        self.library.composing = false;
        self.library.draft_root = None;
        self.library.error = None;
        cx.notify();
    }

    /// Open the new-change form in the document panel.
    pub(super) fn open_new_change(&mut self, cx: &mut Context<Self>) {
        // The form takes the panel, so nothing is selected while it is open: a
        // highlighted document whose text you cannot see reads as a bug.
        // Unsaved edits to it are kept, and come back when it is reopened.
        self.library.leave_selection();
        self.roots.open = false;
        self.knowledge_draft.open = false;
        self.library.composing = true;
        self.library.error = None;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.

    /// Group projects the way the sidebar and the sessions pane both do.
    fn split_sessions(projects: &[crate::workspace::state::ProjectData]) -> (Vec<&str>, Vec<&str>) {
        let mut specs = Vec::new();
        let mut tasks = Vec::new();
        for p in projects {
            if p.is_spec_session() {
                specs.push(p.id.as_str());
            } else if p.is_agent_session() {
                tasks.push(p.id.as_str());
            }
        }
        (tasks, specs)
    }

    fn project(json: serde_json::Value) -> crate::workspace::state::ProjectData {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn spec_sessions_are_grouped_apart_from_task_sessions() {
        // The sidebar lists the two kinds under separate headings, so a session
        // must land in exactly one group and a plain repo in neither.
        let projects = vec![
            project(serde_json::json!({
                "id": "spec1", "name": "add-login (spec)", "path": "/specs",
                "spec_change": "add-login",
            })),
            project(serde_json::json!({
                "id": "task1", "name": "QBL-1 (agent)", "path": "/p",
                "task_ref": {
                    "id": { "provider": "linear", "external_id": "u1" },
                    "display_key": "QBL-1", "title": "t", "url": "http://x",
                },
            })),
            project(serde_json::json!({
                "id": "repo1", "name": "okena", "path": "/p/okena",
            })),
        ];
        let (tasks, specs) = split_sessions(&projects);
        assert_eq!(tasks, ["task1"]);
        assert_eq!(specs, ["spec1"]);
    }

    #[test]
    fn a_spec_session_never_lands_in_the_task_group() {
        // It has no task link, so the task branch must not claim it even
        // though both are sessions rooted above the repos.
        let p = project(serde_json::json!({
            "id": "spec1", "name": "x (spec)", "path": "/specs",
            "spec_change": "x",
        }));
        assert!(p.is_spec_session() && !p.is_agent_session());
    }
}
