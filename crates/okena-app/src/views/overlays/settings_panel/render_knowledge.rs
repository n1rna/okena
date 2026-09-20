//! Knowledge settings — the stores on this machine, adding one, and where
//! okena looks.
//!
//! Stores are listed in okena's own registry (`knowledge/stores.yaml` in the
//! profile), not in these settings: cloning, adding, creating and forgetting a
//! store go through the daemon, which owns that file (ADR-0003). The project
//! discovery switch and the clone folder are ordinary settings.

use super::SettingsPanel;
use super::components::{hook_input_row, section_container, section_header};
use super::render_specs::{
    badge, banner, diagnostic, muted, muted_row, path_line, text_input,
};
use crate::views::components::add_root_form::{
    AddMode, AddRootChrome, AddRootForm, RootKind, describe, render_add_root,
};
use crate::settings::settings_entity;
use crate::theme::{ThemeColors, theme};
use crate::ui::tokens::{ui_text, ui_text_ms};
use crate::views::components::simple_input::{InputChangedEvent, SimpleInputState};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::knowledge::{
    KnowledgeGitStatus, KnowledgePointer, KnowledgeRoot, KnowledgeRootKind, KnowledgeStores,
    Severity,
};

/// State of the Knowledge page.
pub(super) struct KnowledgePage {
    /// Show the "add a store" form at the top of the page.
    ///
    /// Set when something sent you here to add a root, so the form is the
    /// first thing on screen instead of something to scroll for.
    pub(super) add_first: bool,
    stores: Option<KnowledgeStores>,
    loading: bool,
    /// The discovery settings the current snapshot was read with; a change
    /// triggers a re-read.
    fetched_for: Option<String>,
    refresh_pending: bool,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
    /// The add-a-root form, shared with the Roots page (QBL-429).
    add: AddRootForm,
    clone_dir_input: Entity<SimpleInputState>,
}

impl KnowledgePage {
    pub(super) fn new(clone_dir: Option<String>, cx: &mut Context<SettingsPanel>) -> Self {
        let clone_dir_input = text_input(cx, "~/knowledge", clone_dir);
        cx.subscribe(
            &clone_dir_input,
            |_this, entity, _: &InputChangedEvent, cx| {
                let val = entity.read(cx).value().to_string();
                settings_entity(cx).update(cx, |state, cx| state.set_knowledge_clone_dir(val, cx));
            },
        )
        .detach();

        Self {
            stores: None,
            add_first: false,
            loading: false,
            fetched_for: None,
            refresh_pending: false,
            busy: false,
            error: None,
            notice: None,
            add: AddRootForm::new(RootKind::Knowledge, cx),
            clone_dir_input,
        }
    }
}

fn health(root: &KnowledgeRoot, t: &ThemeColors) -> (&'static str, u32) {
    if !root.healthy {
        ("problem", t.error)
    } else if root.status.iter().any(|d| d.severity == Severity::Warning) {
        ("check", t.warning)
    } else {
        ("ok", t.success)
    }
}

/// `main → origin/main · 3 to pull · uncommitted changes`
fn git_line(git: &KnowledgeGitStatus) -> String {
    let mut parts = vec![match (&git.branch, &git.upstream) {
        (Some(branch), Some(upstream)) => format!("{branch} → {upstream}"),
        (Some(branch), None) => format!("{branch}, no upstream"),
        (None, _) => "detached HEAD".to_string(),
    }];
    if git.behind > 0 {
        parts.push(format!("{} to pull", git.behind));
    }
    if git.ahead > 0 {
        parts.push(format!("{} to push", git.ahead));
    }
    if git.dirty {
        parts.push("uncommitted changes".to_string());
    }
    parts.join(" · ")
}

impl SettingsPanel {
    fn knowledge_fingerprint(cx: &App) -> String {
        format!(
            "{:?}",
            settings_entity(cx).read(cx).settings.harness.knowledge
        )
    }

    /// Read what the daemon discovers, with each store's local sync state.
    fn refresh_knowledge_stores(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.action_client.clone() else {
            return;
        };
        self.knowledge.fetched_for = Some(Self::knowledge_fingerprint(cx));
        self.knowledge.loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::KnowledgeStores)
                    .and_then(|v| v.ok_or_else(|| "Missing knowledge stores".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<KnowledgeStores>(v)
                            .map_err(|e| format!("Unexpected knowledge stores: {e}"))
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.knowledge.loading = false;
                    match result {
                        Ok(stores) => this.knowledge.stores = Some(stores),
                        Err(e) => this.knowledge.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Re-read after a settings change, once it has reached the daemon —
    /// settings travel as a separate message.
    fn refresh_knowledge_stores_soon(&mut self, cx: &mut Context<Self>) {
        self.knowledge.refresh_pending = true;
        cx.spawn(async move |this, cx| {
            smol::Timer::after(std::time::Duration::from_millis(500)).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.knowledge.refresh_pending = false;
                    this.refresh_knowledge_stores(cx);
                });
            });
        })
        .detach();
    }

    /// Run a store change on the daemon, then say what happened and re-read.
    fn run_knowledge_action(
        &mut self,
        action: ActionRequest,
        describe: impl Fn(&serde_json::Value) -> String + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.knowledge.busy {
            return;
        }
        let Some(client) = self.action_client.clone() else {
            self.knowledge.error = Some("The local daemon is unavailable.".into());
            cx.notify();
            return;
        };
        self.knowledge.busy = true;
        self.knowledge.error = None;
        self.knowledge.notice = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || client.post_action(action)).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.knowledge.busy = false;
                    match result {
                        Ok(v) => {
                            this.knowledge.notice =
                                Some(describe(&v.unwrap_or(serde_json::Value::Null)));
                            this.knowledge.add.clear(cx);
                            this.refresh_knowledge_stores(cx);
                        }
                        Err(e) => this.knowledge.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Send whatever the shared form is asking for, or say what is missing.
    fn submit_add_knowledge_root(&mut self, cx: &mut Context<Self>) {
        let mode = self.knowledge.add.mode;
        match self.knowledge.add.request(cx) {
            Ok(action) => self.run_knowledge_action(
                action,
                move |v| describe(RootKind::Knowledge, mode, v),
                cx,
            ),
            Err(missing) => {
                self.knowledge.error = Some(missing);
                cx.notify();
            }
        }
    }

    fn knowledge_button(
        &self,
        id: String,
        label: &str,
        primary: bool,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        div()
            .id(SharedString::from(id))
            .cursor_pointer()
            .flex_shrink_0()
            .px(px(10.0))
            .py(px(3.0))
            .rounded(px(4.0))
            .when(primary, |d| {
                d.bg(rgb(t.button_primary_bg))
                    .hover(|s| s.bg(rgb(t.button_primary_hover)))
                    .text_color(rgb(t.button_primary_fg))
            })
            .when(!primary, |d| {
                d.border_1()
                    .border_color(rgb(t.border))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .text_color(rgb(t.text_secondary))
            })
            .when(self.knowledge.busy, |d| d.opacity(0.6))
            .text_size(ui_text_ms(cx))
            .child(label.to_string())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| on_click(this, cx)),
            )
            .into_any_element()
    }

    fn render_knowledge_root_row(
        &self,
        root: &KnowledgeRoot,
        border_top: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let (label, color) = health(root, &t);

        let mut actions = h_flex().gap(px(6.0)).flex_shrink_0();
        if root.kind == KnowledgeRootKind::Store
            && let Some(id) = root.store_id.clone()
        {
            let target = id.clone();
            actions = actions.child(self.knowledge_button(
                format!("knowledge-unregister-{id}"),
                "Remove",
                false,
                move |this, cx| {
                    this.run_knowledge_action(
                        ActionRequest::KnowledgeStoreUnregister { id: target.clone() },
                        |v| {
                            format!(
                                "Removed '{}' from okena. Its checkout is still at {}.",
                                v["id"].as_str().unwrap_or(""),
                                v["left_on_disk"].as_str().unwrap_or("")
                            )
                        },
                        cx,
                    )
                },
                cx,
            ));
        }

        let c = &root.counts;
        let mut body = v_flex()
            .gap(px(3.0))
            .flex_1()
            .min_w_0()
            .child(
                h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .flex_wrap()
                    .child(
                        div()
                            .text_size(ui_text(13.0, cx))
                            .text_color(rgb(t.text_primary))
                            .child(root.name.clone()),
                    )
                    .when_some(
                        root.store_id.clone().filter(|id| *id != root.name),
                        |d, id| d.child(badge(&id, t.border_active, cx)),
                    )
                    .child(badge(label, color, cx)),
            )
            .child(path_line(root.path.clone(), &t, cx))
            .when_some(root.description.clone(), |d, description| {
                d.child(muted(description, &t, cx))
            })
            .when_some(root.remote.clone(), |d, remote| {
                d.child(muted(format!("Remote: {remote}"), &t, cx))
            })
            .when_some(root.git.as_ref().map(git_line), |d, line| {
                d.child(muted(line, &t, cx))
            })
            .when(root.healthy, |d| {
                d.child(muted(
                    format!(
                        "{} docs · {} skills · {} agents · {} templates",
                        c.docs, c.skills, c.agents, c.templates
                    ),
                    &t,
                    cx,
                ))
            })
            .when(!root.used_by.is_empty(), |d| {
                d.child(muted(
                    format!("Used by: {}", root.used_by.join(", ")),
                    &t,
                    cx,
                ))
            });
        for d in &root.status {
            body = body.child(diagnostic(d, &t, cx));
        }

        h_flex()
            .items_start()
            .gap(px(12.0))
            .px(px(12.0))
            .py(px(10.0))
            .when(border_top, |d| d.border_t_1().border_color(rgb(t.border)))
            .child(body)
            .child(actions)
            .into_any_element()
    }

    fn render_knowledge_pointer_row(
        &self,
        pointer: &KnowledgePointer,
        border_top: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let resolved = pointer.root_key.is_some();
        v_flex()
            .gap(px(3.0))
            .px(px(12.0))
            .py(px(10.0))
            .when(border_top, |d| d.border_t_1().border_color(rgb(t.border)))
            .child(
                h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .child(
                        div()
                            .text_size(ui_text(13.0, cx))
                            .text_color(rgb(t.text_primary))
                            .child(format!("{} follows {}", pointer.project, pointer.store_id)),
                    )
                    .when(resolved, |d| {
                        d.child(badge("on this machine", t.success, cx))
                    }),
            )
            .child(path_line(pointer.path.clone(), &t, cx))
            .children(pointer.status.iter().map(|d| diagnostic(d, &t, cx)))
            .into_any_element()
    }

    fn render_add_knowledge_store(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        section_container(&t)
            .child(render_add_root(
                &self.knowledge.add,
                AddRootChrome {
                    id_prefix: "knowledge",
                    busy: self.knowledge.busy,
                },
                cx,
                cx.listener(|this, mode: &AddMode, _window, cx| {
                    this.knowledge.add.mode = *mode;
                    this.knowledge.error = None;
                    cx.notify();
                }),
                cx.listener(|this, _, _window, cx| {
                    this.knowledge.add.init_git = !this.knowledge.add.init_git;
                    cx.notify();
                }),
                cx.listener(|this, _, _window, cx| this.submit_add_knowledge_root(cx)),
            ))
            .into_any_element()
    }

    pub(super) fn render_knowledge(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        // Read on first paint rather than when the panel opens, so a user who
        // never visits this page makes no call; re-read when discovery changes.
        let fingerprint = Self::knowledge_fingerprint(cx);
        if self.action_client.is_some()
            && !self.knowledge.loading
            && !self.knowledge.refresh_pending
            && self.knowledge.fetched_for.as_deref() != Some(fingerprint.as_str())
        {
            if self.knowledge.fetched_for.is_none() {
                self.refresh_knowledge_stores(cx);
            } else {
                self.refresh_knowledge_stores_soon(cx);
            }
        }

        let discovery = settings_entity(cx)
            .read(cx)
            .settings
            .harness
            .knowledge
            .clone();
        let stores = self.knowledge.stores.clone();
        let roots: Vec<KnowledgeRoot> =
            stores.as_ref().map(|s| s.roots.clone()).unwrap_or_default();
        let daemon = self.action_client.is_some();

        let mut page = v_flex();

        // Sent here to add a root: the form leads, so it is on screen without
        // scrolling past the stores you already have.
        if self.knowledge.add_first {
            page = page
                .child(section_header("Add a store", &t, cx))
                .child(self.render_add_knowledge_store(cx));
        }

        // ── Overview ───────────────────────────────────────────────────────
        let mut facts = v_flex()
            .flex_1()
            .min_w_0()
            .gap(px(2.0))
            .px(px(12.0))
            .py(px(8.0));
        if let Some(s) = &stores {
            facts = facts.child(path_line(
                format!("Store registry: {}", s.registry_path),
                &t,
                cx,
            ));
        }
        page = page.child(section_header("Knowledge", &t, cx)).child(
            section_container(&t)
                .child(
                    h_flex()
                        .items_end()
                        .justify_between()
                        .border_t_1()
                        .border_color(rgb(t.border))
                        .child(facts)
                        .child(div().flex_shrink_0().px(px(12.0)).py(px(8.0)).child(
                            self.knowledge_button(
                                "knowledge-refresh".into(),
                                if self.knowledge.loading {
                                    "Reading…"
                                } else {
                                    "Refresh"
                                },
                                false,
                                |this, cx| this.refresh_knowledge_stores(cx),
                                cx,
                            ),
                        )),
                ),
        );
        if let Some(notice) = self.knowledge.notice.clone() {
            page = page.child(banner(notice, t.success, cx));
        }
        if let Some(error) = self.knowledge.error.clone() {
            page = page.child(banner(error, t.error, cx));
        }
        if let Some(s) = &stores {
            for d in &s.status {
                page = page.child(div().mx(px(12.0)).mt(px(8.0)).child(diagnostic(d, &t, cx)));
            }
        }

        // ── Stores ─────────────────────────────────────────────────────────
        page = page.child(section_header("Stores", &t, cx));
        let mut container = section_container(&t);
        let store_roots: Vec<&KnowledgeRoot> = roots
            .iter()
            .filter(|r| r.kind == KnowledgeRootKind::Store)
            .collect();
        if !daemon {
            container = container.child(muted_row(
                "The local daemon is unavailable, so stores cannot be read.",
                &t,
                cx,
            ));
        } else if stores.is_none() {
            container = container.child(muted_row("Reading stores…", &t, cx));
        } else if store_roots.is_empty() {
            container = container.child(muted_row(
                "No knowledge stores on this machine yet. Clone your team's below.",
                &t,
                cx,
            ));
        }
        for (i, root) in store_roots.iter().enumerate() {
            container = container.child(self.render_knowledge_root_row(root, i > 0, cx));
        }
        if !self.knowledge.add_first {
            page = page
                .child(container)
                .child(section_header("Add a store", &t, cx))
                .child(self.render_add_knowledge_store(cx));
        } else {
            page = page.child(container);
        }

        // ── Discovery ──────────────────────────────────────────────────────
        page = page.child(section_header("Discovery", &t, cx)).child(
            section_container(&t)
                .child(self.render_toggle(
                    "knowledge-discover-projects",
                    "Find knowledge in projects",
                    discovery.projects,
                    true,
                    |state, val, cx| state.set_knowledge_discovery_projects(val, cx),
                    cx,
                ))
                .child(hook_input_row(
                    "knowledge-clone-dir",
                    "Clone folder",
                    "Where a clone goes when no destination is given. Empty is ~/knowledge.",
                    &self.knowledge.clone_dir_input,
                    &t,
                    false,
                    cx,
                )),
        );

        if discovery.projects && stores.is_some() {
            let project_roots: Vec<&KnowledgeRoot> = roots
                .iter()
                .filter(|r| r.kind == KnowledgeRootKind::Project)
                .collect();
            let pointers = stores
                .as_ref()
                .map(|s| s.pointers.clone())
                .unwrap_or_default();
            let mut container = section_container(&t);
            if project_roots.is_empty() && pointers.is_empty() {
                container = container.child(muted_row(
                    "No project follows a store in .okena/knowledge.yaml or keeps its own \
                     .okena/knowledge/ folder.",
                    &t,
                    cx,
                ));
            }
            let mut first = true;
            for root in project_roots {
                container = container.child(self.render_knowledge_root_row(root, !first, cx));
                first = false;
            }
            for pointer in &pointers {
                container = container.child(self.render_knowledge_pointer_row(pointer, !first, cx));
                first = false;
            }
            page = page
                .child(section_header("Found in projects", &t, cx))
                .child(container);
        }

        page.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::git_line;
    use okena_core::knowledge::KnowledgeGitStatus;

    #[test]
    fn the_git_line_names_the_branch_and_only_what_needs_doing() {
        let tracked = KnowledgeGitStatus {
            branch: Some("main".into()),
            upstream: Some("origin/main".into()),
            ..Default::default()
        };
        assert_eq!(git_line(&tracked), "main → origin/main");
        assert_eq!(
            git_line(&KnowledgeGitStatus {
                behind: 3,
                ahead: 1,
                dirty: true,
                ..tracked.clone()
            }),
            "main → origin/main · 3 to pull · 1 to push · uncommitted changes"
        );
        assert_eq!(
            git_line(&KnowledgeGitStatus {
                upstream: None,
                ..tracked
            }),
            "main, no upstream"
        );
        assert_eq!(git_line(&KnowledgeGitStatus::default()), "detached HEAD");
    }
}
