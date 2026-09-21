//! The Roots page — add, list, arrange and remove the roots a section reads.
//!
//! Opened by the `+` in the Knowledge and Specs sidebars' ROOTS header, and it
//! stands where a document's text stands, the way the New form does: the
//! sidebar you are changing stays on screen beside it.
//!
//! One page for both sections, because the questions are the same. What
//! differs is what a root *is*:
//!
//! - **Knowledge roots layer**, so they have one saved order (QBL-425) and the
//!   list is dragged into it. The top root holding a template wins, so moving
//!   a row changes which copy the next agent launch uses. `okena-defaults` is
//!   shown last and has no handle: it holds a copy of the compiled-in
//!   defaults, so it must not be draggable above the layers that override it.
//! - **Specs roots do not layer** — a change belongs to one root — so the
//!   Specs list has no order and no handles.
//!
//! Adding is the shared form (`add_root_form`), the same three choices
//! Settings offers, because there are now two places to add a root and they
//! must not drift.

use crate::theme::{ThemeColors, theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms};
use crate::views::components::add_root_form::{
    AddMode, AddRootChrome, AddRootForm, RootKind, describe, render_add_root,
};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::knowledge::{KnowledgeRootKind, KnowledgeStores, Severity};
use okena_core::specs::{SpecRootKind, SpecStores};

use super::{HarnessPane, HarnessSection};

/// The Roots page's own state. One per pane, for that pane's section.
pub(crate) struct RootsPage {
    /// Showing in the right-hand column.
    pub(crate) open: bool,
    pub(crate) form: AddRootForm,
    /// An add or a remove is running on the daemon.
    pub(crate) busy: bool,
    pub(crate) error: Option<String>,
}

impl RootsPage {
    pub(crate) fn new(section: HarnessSection, cx: &mut Context<HarnessPane>) -> Self {
        Self {
            open: false,
            form: AddRootForm::new(Self::kind(section), cx),
            busy: false,
            error: None,
        }
    }

    /// Tasks and Testing have no roots; their page is never opened, so the
    /// kind only has to be something.
    fn kind(section: HarnessSection) -> RootKind {
        match section {
            HarnessSection::Specs => RootKind::Specs,
            _ => RootKind::Knowledge,
        }
    }
}

// ─── The list, as data ──────────────────────────────────────────────────────

/// How a root can be taken off the list, when it can.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Removal {
    /// Unregister a store by id. The checkout stays on disk.
    Store(String),
    /// Drop a folder from `harness.specs.folders`. Specs only.
    Folder(String),
}

/// What the page shows for one root.
///
/// Built from either section's roots so the page renders one shape, and so
/// "which roots can be dragged, which can be removed" is a thing that can be
/// tested without a window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RootRow {
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) path: String,
    /// What the root is: `Store`, `Project`, `Folder`.
    pub(crate) kind: &'static str,
    /// `Ok`, `Check` or `Problem` — the health badge.
    pub(crate) health: Health,
    /// A line of detail under the path: counts, or what points at it.
    pub(crate) detail: Option<String>,
    pub(crate) problems: Vec<String>,
    pub(crate) removal: Option<Removal>,
    /// Takes part in the saved order, so it has a drag handle.
    pub(crate) orderable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Health {
    Ok,
    Check,
    Problem,
}

impl Health {
    fn label(self) -> &'static str {
        match self {
            Health::Ok => "ok",
            Health::Check => "check",
            Health::Problem => "problem",
        }
    }

    fn color(self, t: &ThemeColors) -> u32 {
        match self {
            Health::Ok => t.success,
            Health::Check => t.warning,
            Health::Problem => t.error,
        }
    }
}

/// The Knowledge list: every root in the order it layers in, which is the
/// order discovery already put them in (QBL-425), with `okena-defaults` last
/// and unmovable.
pub(crate) fn knowledge_rows(stores: &KnowledgeStores) -> Vec<RootRow> {
    stores
        .roots
        .iter()
        .map(|root| {
            let c = &root.counts;
            RootRow {
                key: root.key.clone(),
                name: root.name.clone(),
                path: root.path.clone(),
                kind: match (root.builtin, root.kind) {
                    (true, _) => "okena's own",
                    (_, KnowledgeRootKind::Store) => "Store",
                    (_, KnowledgeRootKind::Project) => "Project",
                },
                health: if !root.healthy {
                    Health::Problem
                } else if root.status.iter().any(|d| d.severity == Severity::Warning) {
                    Health::Check
                } else {
                    Health::Ok
                },
                detail: root.healthy.then(|| {
                    format!(
                        "{} docs · {} skills · {} agents · {} templates",
                        c.docs, c.skills, c.agents, c.templates
                    )
                }),
                problems: root.status.iter().map(|d| d.message.clone()).collect(),
                // okena's own store is rewritten on every start, so removing
                // it would only make it come back.
                removal: (!root.builtin)
                    .then(|| root.store_id.clone().map(Removal::Store))
                    .flatten(),
                orderable: !root.builtin,
            }
        })
        .collect()
}

/// The Specs list. No order, so nothing is orderable; a folder root is removed
/// from the setting that added it, and a project root cannot be removed here
/// at all — it belongs to its repository.
pub(crate) fn spec_rows(stores: &SpecStores) -> Vec<RootRow> {
    stores
        .roots
        .iter()
        .map(|root| RootRow {
            key: root.key.clone(),
            name: root.name.clone(),
            path: root.path.clone(),
            kind: match root.kind {
                SpecRootKind::Store => "Store",
                SpecRootKind::Project => "Project",
                SpecRootKind::Folder => "Folder",
            },
            health: if !root.healthy {
                Health::Problem
            } else if root
                .status
                .iter()
                .any(|d| d.severity == okena_core::specs::SpecSeverity::Warning)
            {
                Health::Check
            } else {
                Health::Ok
            },
            detail: root
                .is_default
                .then(|| "The machine default store".to_string())
                .or_else(|| root.schema.clone().map(|s| format!("Schema: {s}"))),
            problems: root.status.iter().map(|d| d.message.clone()).collect(),
            removal: match root.kind {
                SpecRootKind::Store => root.store_id.clone().map(Removal::Store),
                SpecRootKind::Folder => Some(Removal::Folder(root.path.clone())),
                SpecRootKind::Project => None,
            },
            orderable: false,
        })
        .collect()
}

/// What the page is dragging: the root, and where it started.
#[derive(Clone, Debug)]
pub(crate) struct RootDrag {
    pub(crate) key: String,
    pub(crate) name: String,
}

struct RootDragView {
    name: String,
}

impl Render for RootDragView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        div()
            .px(px(8.0))
            .py(px(3.0))
            .rounded(px(4.0))
            .bg(rgb(t.bg_secondary))
            .border_1()
            .border_color(rgb(t.border_active))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_primary))
            .child(self.name.clone())
    }
}

// ─── Rendering ──────────────────────────────────────────────────────────────

impl HarnessPane {
    /// Show the Roots page, from the `+` in the sidebar's ROOTS header.
    pub(super) fn open_roots_page(&mut self, cx: &mut Context<Self>) {
        self.roots.open = true;
        self.roots.error = None;
        // The page and a document share the one column, so opening it is how
        // you leave whatever was there — matching how picking an entry leaves
        // the New form.
        self.knowledge_draft.open = false;
        self.specs.composing = false;
        cx.notify();
    }

    pub(super) fn close_roots_page(&mut self, cx: &mut Context<Self>) {
        self.roots.open = false;
        self.roots.error = None;
        cx.notify();
    }

    /// The roots of the section this pane is showing, as rows.
    fn root_rows(&self) -> Vec<RootRow> {
        match self.section {
            HarnessSection::Specs => self.specs.stores.as_ref().map(spec_rows),
            _ => self.knowledge.stores.as_ref().map(knowledge_rows),
        }
        .unwrap_or_default()
    }

    /// Re-read the section's roots after the daemon changed them.
    fn refresh_roots(&mut self, cx: &mut Context<Self>) {
        match self.section {
            HarnessSection::Specs => self.refresh_specs(cx),
            _ => self.refresh_knowledge(cx),
        }
    }

    /// Run an action that changes the roots, then say what happened.
    fn run_root_action(
        &mut self,
        action: ActionRequest,
        describe: impl Fn(&serde_json::Value) -> String + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.roots.busy {
            return;
        }
        self.roots.busy = true;
        self.roots.error = None;
        cx.notify();

        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || client.post_action(action)).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.roots.busy = false;
                    match result {
                        Ok(v) => {
                            this.report(describe(&v.unwrap_or(serde_json::Value::Null)), cx);
                            this.roots.form.clear(cx);
                            this.refresh_roots(cx);
                        }
                        Err(e) => this.roots.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn submit_add_root(&mut self, cx: &mut Context<Self>) {
        let kind = self.roots.form.kind();
        let mode = self.roots.form.mode;
        match self.roots.form.request(cx) {
            Ok(action) => self.run_root_action(action, move |v| describe(kind, mode, v), cx),
            Err(missing) => {
                self.roots.error = Some(missing);
                cx.notify();
            }
        }
    }

    fn remove_root(&mut self, removal: Removal, cx: &mut Context<Self>) {
        match removal {
            Removal::Store(id) => {
                let action = match self.section {
                    HarnessSection::Specs => ActionRequest::SpecStoreUnregister { id },
                    _ => ActionRequest::KnowledgeStoreUnregister { id },
                };
                self.run_root_action(
                    action,
                    |v| {
                        format!(
                            "Removed '{}'. Its checkout is still at {}.",
                            v["id"].as_str().unwrap_or(""),
                            v["left_on_disk"].as_str().unwrap_or("")
                        )
                    },
                    cx,
                );
            }
            // A folder root is not in a registry: it is a line in settings, so
            // removing it is a settings edit and there is nothing to ask the
            // daemon.
            Removal::Folder(path) => {
                let settings = crate::settings::settings_entity(cx);
                let folders: Vec<String> = settings
                    .read(cx)
                    .settings
                    .active_space()
                    .spec_folders()
                    .into_iter()
                    .filter(|f| okena_core::fs::expand_home(f) != okena_core::fs::expand_home(&path))
                    .collect();
                settings.update(cx, |state, cx| state.set_spec_folders(folders, cx));
                self.report(format!("Removed the folder root {path}."), cx);
                self.refresh_roots(cx);
            }
        }
    }

    /// Save the order with `key` dropped onto the row at `onto`.
    ///
    /// The order written is the whole list as shown, so this is also what
    /// drops keys for roots that are no longer discovered.
    fn reorder_roots(&mut self, key: &str, onto: usize, cx: &mut Context<Self>) {
        let Some(stores) = self.knowledge.stores.as_ref() else {
            return;
        };
        let settings = crate::settings::settings_entity(cx);
        let saved = settings.read(cx).settings.active_space().knowledge.order.clone();
        let shown = okena_core::knowledge_order::normalize(&stores.roots, &saved);
        let next = okena_core::knowledge_order::moved(&shown, key, onto);
        if next == shown {
            return;
        }
        settings.update(cx, |state, cx| {
            state.set_knowledge_root_order(next.clone(), cx)
        });
        // Rearrange what is on screen now rather than waiting for the settings
        // to reach the daemon and a fresh listing to come back: a drag that
        // visibly does nothing for half a second reads as a drag that failed.
        if let Some(stores) = self.knowledge.stores.as_mut() {
            okena_core::knowledge_order::apply(&mut stores.roots, &next);
        }
        cx.notify();
    }

    fn roots_fact(&self, label: &str, value: String, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .min_w_0()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(format!("{label}{value}"))
            .into_any_element()
    }

    fn render_root_manage_row(
        &self,
        row: &RootRow,
        index: usize,
        ordered: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let health = row.health;

        let mut body = v_flex()
            .flex_1()
            .min_w_0()
            .gap(px(3.0))
            .child(
                h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .flex_wrap()
                    .child(
                        div()
                            .text_size(ui_text(13.0, cx))
                            .text_color(rgb(t.text_primary))
                            .child(row.name.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded(px(3.0))
                            .bg(with_alpha(t.border_active, 0.15))
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_secondary))
                            .child(row.kind),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded(px(3.0))
                            .bg(with_alpha(health.color(&t), 0.15))
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(health.color(&t)))
                            .child(health.label()),
                    ),
            )
            .child(self.roots_fact("", row.path.clone(), cx));
        if let Some(detail) = row.detail.clone() {
            body = body.child(self.roots_fact("", detail, cx));
        }
        for problem in &row.problems {
            body = body.child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(health.color(&t)))
                    .child(problem.clone()),
            );
        }

        let mut actions = h_flex().gap(px(6.0)).flex_shrink_0();
        if let Some(removal) = row.removal.clone() {
            let id = format!("roots-remove-{}", row.key);
            actions = actions.child(
                div()
                    .id(SharedString::from(id))
                    .cursor_pointer()
                    .flex_shrink_0()
                    .px(px(10.0))
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(rgb(t.border))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .when(self.roots.busy, |d| d.opacity(0.6))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child("Remove")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.remove_root(removal.clone(), cx)
                        }),
                    ),
            );
        }

        // The handle is the affordance; the whole row is the drag target, so a
        // drop does not need to hit a 12px column.
        let draggable = ordered && row.orderable;
        let mut container = h_flex()
            .id(SharedString::from(format!("roots-row-{}", row.key)))
            .items_start()
            .gap(px(10.0))
            .px(px(12.0))
            .py(px(10.0))
            .when(index > 0, |d| d.border_t_1().border_color(rgb(t.border)))
            .when(ordered, |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .w(px(14.0))
                        .pt(px(2.0))
                        .text_size(ui_text_md(cx))
                        .text_color(rgb(if draggable { t.text_muted } else { t.border }))
                        .child(if draggable { "⠿" } else { "·" }),
                )
            })
            .child(body)
            .child(actions);

        if draggable {
            let drag = RootDrag {
                key: row.key.clone(),
                name: row.name.clone(),
            };
            container = container
                .cursor_pointer()
                .on_drag(drag, move |drag, _position, _window, cx| {
                    cx.new(|_| RootDragView {
                        name: drag.name.clone(),
                    })
                })
                .drag_over::<RootDrag>(move |style, _, _, _| {
                    style.border_t_2().border_color(rgb(t.border_active))
                })
                .on_drop(cx.listener(move |this, drag: &RootDrag, _window, cx| {
                    this.reorder_roots(&drag.key.clone(), index, cx);
                }));
        }
        container.into_any_element()
    }

    /// The right-hand column when the Roots page is open.
    pub(super) fn render_roots_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let rows = self.root_rows();
        // Knowledge roots layer, so they have an order to arrange. Specs roots
        // do not, so the list is just a list.
        let ordered = self.section != HarnessSection::Specs;
        let thing = if ordered { "knowledge" } else { "specs" };

        let header = h_flex()
            .items_center()
            .justify_between()
            .px(px(12.0))
            .py(px(10.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .child(
                div()
                    .text_size(ui_text(15.0, cx))
                    .text_color(rgb(t.text_primary))
                    .child("Roots"),
            )
            .child(self.small_button(
                "roots-close",
                "Done",
                cx.listener(|this, _, _window, cx| this.close_roots_page(cx)),
                cx,
            ));

        let mut col = v_flex()
            .id("roots-page")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .child(header);

        if let Some(error) = self.roots.error.clone() {
            col = col.child(self.error_banner(error, cx));
        }

        col = col.child(
            div().px(px(12.0)).pt(px(10.0)).child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(if ordered {
                        "Templates, partials and skills resolve top to bottom — the first root \
                         with the file wins. Drag a root to change which copy an agent launches \
                         with. okena's own store is always consulted last."
                    } else {
                        "Every root here is searched for specs and changes. Specs are not \
                         layered, so the order does not matter."
                    }),
            ),
        );

        // ── The roots ──────────────────────────────────────────────────────
        let mut list = v_flex()
            .mx(px(12.0))
            .mt(px(10.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(rgb(t.border));
        if rows.is_empty() {
            list = list.child(
                div()
                    .px(px(12.0))
                    .py(px(10.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(format!("No {thing} roots yet — add one below.")),
            );
        }
        for (index, row) in rows.iter().enumerate() {
            list = list.child(self.render_root_manage_row(row, index, ordered, cx));
        }
        col = col.child(list);

        // ── Adding one ─────────────────────────────────────────────────────
        col = col.child(
            div()
                .px(px(12.0))
                .pt(px(16.0))
                .pb(px(4.0))
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child("ADD A ROOT"),
        );
        col = col.child(
            div()
                .mx(px(12.0))
                .mb(px(16.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(rgb(t.border))
                .child(render_add_root(
                    &self.roots.form,
                    AddRootChrome {
                        id_prefix: "roots-page",
                        busy: self.roots.busy,
                    },
                    cx,
                    cx.listener(|this, mode: &AddMode, _window, cx| {
                        this.roots.form.mode = *mode;
                        this.roots.error = None;
                        cx.notify();
                    }),
                    cx.listener(|this, _, _window, cx| {
                        this.roots.form.init_git = !this.roots.form.init_git;
                        cx.notify();
                    }),
                    cx.listener(|this, _, _window, cx| this.submit_add_root(cx)),
                )),
        );

        col.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{Health, Removal, knowledge_rows, spec_rows};
    use okena_core::knowledge::{
        Diagnostic, KnowledgeCounts, KnowledgeRoot, KnowledgeRootKind, KnowledgeStores,
    };
    use okena_core::specs::{SpecRoot, SpecRootKind, SpecStores};

    fn knowledge_root(key: &str, name: &str) -> KnowledgeRoot {
        KnowledgeRoot {
            key: key.into(),
            kind: KnowledgeRootKind::Store,
            name: name.into(),
            path: format!("/k/{name}"),
            store_id: Some(name.into()),
            description: None,
            remote: None,
            healthy: true,
            builtin: false,
            git: None,
            counts: KnowledgeCounts {
                docs: 2,
                skills: 1,
                agents: 0,
                templates: 3,
            },
            used_by: Vec::new(),
            status: Vec::new(),
        }
    }

    #[test]
    fn the_knowledge_list_is_draggable_and_removable_except_okenas_own() {
        // The order discovery hands back is the order shown, so the list is
        // built in place; what the page decides is which rows have a handle
        // and which have a Remove.
        let defaults = KnowledgeRoot {
            builtin: true,
            store_id: Some("okena-defaults".into()),
            ..knowledge_root("store:okena-defaults", "okena-defaults")
        };
        let project = KnowledgeRoot {
            kind: KnowledgeRootKind::Project,
            store_id: None,
            ..knowledge_root("path:/repo/.okena/knowledge", "web")
        };
        let stores = KnowledgeStores {
            roots: vec![knowledge_root("store:acme", "acme"), project, defaults],
            ..Default::default()
        };

        let rows = knowledge_rows(&stores);
        assert_eq!(
            rows.iter().map(|r| r.kind).collect::<Vec<_>>(),
            ["Store", "Project", "okena's own"]
        );
        assert_eq!(
            rows.iter().map(|r| r.orderable).collect::<Vec<_>>(),
            [true, true, false],
            "okena-defaults is shown last and cannot be moved"
        );
        assert_eq!(
            rows[0].removal,
            Some(Removal::Store("acme".into())),
            "a store is unregistered by id"
        );
        assert_eq!(
            rows[1].removal, None,
            "a project root has no registry entry to remove"
        );
        assert_eq!(
            rows[2].removal, None,
            "okena's own store is rewritten on every start, so removing it is meaningless"
        );
        assert_eq!(
            rows[0].detail.as_deref(),
            Some("2 docs · 1 skills · 0 agents · 3 templates")
        );
    }

    #[test]
    fn an_unhealthy_knowledge_root_still_lists_with_its_problem() {
        // The page is where you go to fix a broken root, so a root that cannot
        // be read must be on it, with the reason and a way to remove it.
        let broken = KnowledgeRoot {
            healthy: false,
            status: vec![Diagnostic::error(
                "store_checkout_missing",
                "The checkout of `gone` is gone: /k/gone",
            )],
            ..knowledge_root("store:gone", "gone")
        };
        let stores = KnowledgeStores {
            roots: vec![broken],
            ..Default::default()
        };
        let rows = knowledge_rows(&stores);
        assert_eq!(rows[0].health, Health::Problem);
        assert_eq!(rows[0].detail, None, "counts of a root nobody can read");
        assert_eq!(rows[0].problems.len(), 1);
        assert!(rows[0].orderable, "still part of the order while it exists");
        assert_eq!(rows[0].removal, Some(Removal::Store("gone".into())));
    }

    #[test]
    fn the_specs_list_has_no_order_and_removes_a_folder_from_settings() {
        // Specs roots are not layered, so nothing is orderable; a folder root
        // came from a setting, so that is where it goes back to.
        let root = |key: &str, path: &str, kind: SpecRootKind, id: Option<&str>| SpecRoot {
            key: key.into(),
            kind,
            name: key.into(),
            path: path.into(),
            store_id: id.map(str::to_string),
            remote: None,
            schema: None,
            healthy: true,
            is_default: false,
            git: None,
            references: Vec::new(),
            used_by: Vec::new(),
            status: Vec::new(),
        };
        let stores = SpecStores {
            roots: vec![
                root("store:plans", "/s/plans", SpecRootKind::Store, Some("plans")),
                root("path:/s/folder", "/s/folder", SpecRootKind::Folder, None),
                root("path:/s/repo", "/s/repo", SpecRootKind::Project, None),
            ],
            ..Default::default()
        };

        let rows = spec_rows(&stores);
        assert!(
            rows.iter().all(|r| !r.orderable),
            "specs roots are never dragged"
        );
        assert_eq!(rows[0].removal, Some(Removal::Store("plans".into())));
        assert_eq!(
            rows[1].removal,
            Some(Removal::Folder("/s/folder".into())),
            "a folder root is removed from harness.specs.folders by its path"
        );
        assert_eq!(
            rows[2].removal, None,
            "a project root belongs to its repository"
        );
    }
}
