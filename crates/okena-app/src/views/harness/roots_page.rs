//! The Origins page — add, list, arrange and remove the origins the Library
//! reads.
//!
//! Opened by the `+` in the Library sidebar's ORIGINS header, and it stands
//! where a document's text stands, the way the New form does: the sidebar you
//! are changing stays on screen beside it.
//!
//! One page for every origin type, because the questions are the same. What
//! differs is whether a type layers:
//!
//! - **Knowledge origins layer**, so they have one saved order (QBL-425) and
//!   their rows are dragged into it. The top one holding a template wins, so
//!   moving a row changes which copy the next agent launch uses.
//!   `okena-defaults` is shown last and has no handle: it holds a copy of the
//!   compiled-in defaults, so it must not be draggable above the layers that
//!   override it.
//! - **Spec and freeform origins do not layer** — a change belongs to one
//!   root, a document to one folder — so they have no order, no handles and
//!   no overrides.
//!
//! Adding is the shared form (`add_root_form`), the same choices Settings
//! offers, because there are two places to add an origin and they must not
//! drift.

use crate::theme::{ThemeColors, theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms};
use crate::views::components::add_root_form::{
    AddMode, AddRootChrome, AddRootForm, describe, render_add_root,
};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::library::{LibraryOrigin, LibraryOrigins, OriginKind, OriginType};

use super::HarnessPane;
use super::library_view::{counts_line, group_label};

/// The Origins page's own state. One per pane.
pub(crate) struct RootsPage {
    /// Showing in the right-hand column.
    pub(crate) open: bool,
    pub(crate) form: AddRootForm,
    /// An add or a remove is running on the daemon.
    pub(crate) busy: bool,
    pub(crate) error: Option<String>,
}

impl RootsPage {
    pub(crate) fn new(cx: &mut Context<HarnessPane>) -> Self {
        Self {
            open: false,
            // Knowledge first: it is the type the page's order is about. The
            // form's own pills change it.
            form: AddRootForm::new(OriginType::Knowledge, cx),
            busy: false,
            error: None,
        }
    }
}

// ─── The list, as data ──────────────────────────────────────────────────────

/// What the page shows for one origin.
///
/// One shape for every type, so the page renders one kind of row and "which
/// origins can be dragged, which can be removed, which take overrides" is a
/// thing that can be tested without a window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RootRow {
    /// The origin's Library key: what a remove is sent with.
    pub(crate) key: String,
    pub(crate) origin_type: OriginType,
    pub(crate) name: String,
    pub(crate) path: String,
    /// Where the origin was found: `Store`, `Project`, `Folder`, or
    /// `okena's own`.
    pub(crate) kind: &'static str,
    /// `Ok`, `Check` or `Problem` — the health badge.
    pub(crate) health: Health,
    /// A line of detail under the path: counts, or what it is.
    pub(crate) detail: Option<String>,
    pub(crate) problems: Vec<String>,
    /// Can be taken off the list here. A project's origin cannot: it belongs
    /// to its repository. Neither can okena's own store, which would only
    /// come back.
    pub(crate) removable: bool,
    /// Takes part in the saved layering order, so it has a drag handle. Only
    /// knowledge origins layer, and okena's own is always last.
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

/// The row for one origin.
pub(crate) fn origin_row(origin: &LibraryOrigin) -> RootRow {
    RootRow {
        key: origin.key.clone(),
        origin_type: origin.origin_type,
        name: origin.name.clone(),
        path: origin.path.clone(),
        kind: match (origin.builtin, origin.kind) {
            (true, _) => "okena's own",
            (_, OriginKind::Store) => "Store",
            (_, OriginKind::Project) => "Project",
            (_, OriginKind::Folder) => "Folder",
        },
        health: if !origin.healthy {
            Health::Problem
        } else if origin.warned() {
            Health::Check
        } else {
            Health::Ok
        },
        detail: match origin.origin_type {
            // Counts of an origin nobody can read would be zeros that look
            // like a fact.
            OriginType::Knowledge => origin
                .counts
                .as_ref()
                .filter(|_| origin.healthy)
                .map(counts_line),
            OriginType::Spec => origin
                .is_default
                .then(|| "The machine default store".to_string())
                .or_else(|| origin.schema.clone().map(|s| format!("Schema: {s}"))),
            OriginType::Freeform => origin.documents.map(|n| {
                format!("{n} {}", if n == 1 { "document" } else { "documents" })
            }),
        },
        problems: origin.status.iter().map(|d| d.message.clone()).collect(),
        removable: !origin.builtin && origin.kind != OriginKind::Project,
        orderable: origin.takes_part_in_order(),
    }
}

/// Every origin as a row, in the order the Library lists them: knowledge in
/// the order it layers in — which is the order discovery already put it in
/// (QBL-425), `okena-defaults` last — then specs, then freeform.
pub(crate) fn origin_rows(origins: &LibraryOrigins) -> Vec<RootRow> {
    origins.origins.iter().map(origin_row).collect()
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
    /// Show the Origins page, from the `+` in the sidebar's ORIGINS header.
    pub(super) fn open_roots_page(&mut self, cx: &mut Context<Self>) {
        self.roots.open = true;
        self.roots.error = None;
        // The page and a document share the one column, so opening it is how
        // you leave whatever was there — matching how picking an entry leaves
        // the New form.
        self.knowledge_draft.open = false;
        self.library.composing = false;
        cx.notify();
    }

    pub(super) fn close_roots_page(&mut self, cx: &mut Context<Self>) {
        self.roots.open = false;
        self.roots.error = None;
        cx.notify();
    }

    /// The Library's origins, as rows.
    fn root_rows(&self) -> Vec<RootRow> {
        self.library
            .origins
            .as_ref()
            .map(origin_rows)
            .unwrap_or_default()
    }

    /// Run an action that changes the origins, then say what happened.
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
                            this.refresh_library(cx);
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
        let origin_type = self.roots.form.origin_type();
        let mode = self.roots.form.mode;
        match self.roots.form.request(cx) {
            Ok(action) => {
                self.run_root_action(action, move |v| describe(origin_type, mode, v), cx)
            }
            Err(missing) => {
                self.roots.error = Some(missing);
                cx.notify();
            }
        }
    }

    /// Take the origin keyed `key` off the list. The daemon knows how: a
    /// store leaves its registry, a folder leaves the space's settings.
    /// Either way the folder stays on disk.
    fn remove_root(&mut self, key: String, cx: &mut Context<Self>) {
        self.run_root_action(
            ActionRequest::LibraryStoreUnregister { root: key },
            |v| {
                format!(
                    "Removed '{}'. Its folder is still at {}.",
                    v["id"].as_str().unwrap_or(""),
                    v["left_on_disk"].as_str().unwrap_or("")
                )
            },
            cx,
        );
    }

    /// Save the layering order with the knowledge origin keyed `key` dropped
    /// onto the knowledge row at `onto`.
    ///
    /// The order written is the whole list as shown, so this is also what
    /// drops keys for origins that are no longer discovered.
    fn reorder_roots(&mut self, key: &str, onto: usize, cx: &mut Context<Self>) {
        let Some(origins) = self.library.origins.as_ref() else {
            return;
        };
        let layers: Vec<LibraryOrigin> = origins.of_type(OriginType::Knowledge).cloned().collect();
        // The order is written in the keys discovery gives, as it always was.
        let Some((OriginType::Knowledge, key)) = okena_core::library::split_key(key) else {
            return;
        };
        let settings = crate::settings::settings_entity(cx);
        let saved = settings
            .read(cx)
            .settings
            .active_space()
            .library
            .knowledge
            .order
            .clone();
        let shown = okena_core::knowledge_order::normalize(&layers, &saved);
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
        if let Some(origins) = self.library.origins.as_mut() {
            origins.arrange_layers(&next);
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
        if row.removable {
            let key = row.key.clone();
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
                            this.remove_root(key.clone(), cx)
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

    /// The right-hand column when the Origins page is open.
    pub(super) fn render_roots_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let rows = self.root_rows();

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
                    .child("Origins"),
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

        let (note_size, note_color) = (ui_text_ms(cx), rgb(t.text_muted));
        let note = move |text: &'static str| {
            div()
                .px(px(12.0))
                .pt(px(10.0))
                .text_size(note_size)
                .text_color(note_color)
                .child(text)
        };
        if rows.is_empty() {
            col = col.child(note("No origins yet — add one below."));
        }

        // ── The origins, a list per type ───────────────────────────────────
        for origin_type in OriginType::all() {
            let of_type: Vec<&RootRow> = rows
                .iter()
                .filter(|r| r.origin_type == origin_type)
                .collect();
            if of_type.is_empty() {
                continue;
            }
            // Only knowledge origins layer, so only their list has an order
            // to arrange and handles to arrange it with.
            let ordered = origin_type.layers();
            col = col.child(
                div()
                    .px(px(12.0))
                    .pt(px(16.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(group_label(origin_type).to_uppercase()),
            );
            col = col.child(note(match origin_type {
                OriginType::Knowledge => {
                    "Templates, partials and skills resolve top to bottom — the first origin \
                     with the file wins. Drag an origin to change which copy an agent launches \
                     with. okena's own store is always consulted last."
                }
                OriginType::Spec => {
                    "Every origin here is searched for specs and changes. Specs are not \
                     layered, so there is no order and nothing here overrides anything."
                }
                OriginType::Freeform => {
                    "Folders of markdown, listed, searched and handed to agents as context. \
                     Freeform origins are not layered and take no overrides."
                }
            }));
            let mut list = v_flex()
                .mx(px(12.0))
                .mt(px(8.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(rgb(t.border));
            // The index a drop is measured against is the row's place among
            // the knowledge origins, which is the place it layers at.
            for (index, row) in of_type.into_iter().enumerate() {
                list = list.child(self.render_root_manage_row(row, index, ordered, cx));
            }
            col = col.child(list);
        }

        // ── Adding one ─────────────────────────────────────────────────────
        col = col.child(
            div()
                .px(px(12.0))
                .pt(px(16.0))
                .pb(px(4.0))
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child("ADD AN ORIGIN"),
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
                    cx.listener(|this, origin_type: &OriginType, _window, cx| {
                        this.roots.form.set_origin_type(*origin_type, cx);
                        this.roots.error = None;
                        cx.notify();
                    }),
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
    use super::{Health, origin_rows};
    use okena_core::knowledge::{
        Diagnostic, KnowledgeCounts, KnowledgeRoot, KnowledgeRootKind, KnowledgeStores,
    };
    use okena_core::library::{LibraryOrigin, LibraryOrigins, OriginType};
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

    fn spec_root(key: &str, path: &str, kind: SpecRootKind, id: Option<&str>) -> SpecRoot {
        SpecRoot {
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
        }
    }

    fn origins(knowledge: Vec<KnowledgeRoot>, specs: Vec<SpecRoot>) -> LibraryOrigins {
        LibraryOrigins::assemble(
            KnowledgeStores {
                roots: knowledge,
                ..Default::default()
            },
            SpecStores {
                roots: specs,
                ..Default::default()
            },
            Vec::new(),
        )
    }

    #[test]
    fn knowledge_origins_are_draggable_and_removable_except_okenas_own() {
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
        let rows = origin_rows(&origins(
            vec![knowledge_root("store:acme", "acme"), project, defaults],
            Vec::new(),
        ));
        assert_eq!(
            rows.iter().map(|r| r.kind).collect::<Vec<_>>(),
            ["Store", "Project", "okena's own"]
        );
        assert_eq!(
            rows.iter().map(|r| r.orderable).collect::<Vec<_>>(),
            [true, true, false],
            "okena-defaults is shown last and cannot be moved"
        );
        assert!(rows[0].removable, "a store is unregistered");
        assert_eq!(rows[0].key, "knowledge:store:acme", "by its Library key");
        assert!(
            !rows[1].removable,
            "a project's origin has no registry entry to remove"
        );
        assert!(
            !rows[2].removable,
            "okena's own store is rewritten on every start, so removing it is meaningless"
        );
        assert_eq!(
            rows[0].detail.as_deref(),
            Some("2 docs · 1 skill · 0 agents · 3 templates")
        );
    }

    #[test]
    fn an_unhealthy_knowledge_origin_still_lists_with_its_problem() {
        // The page is where you go to fix a broken origin, so one that cannot
        // be read must be on it, with the reason and a way to remove it.
        let broken = KnowledgeRoot {
            healthy: false,
            status: vec![Diagnostic::error(
                "store_checkout_missing",
                "The checkout of `gone` is gone: /k/gone",
            )],
            ..knowledge_root("store:gone", "gone")
        };
        let rows = origin_rows(&origins(vec![broken], Vec::new()));
        assert_eq!(rows[0].health, Health::Problem);
        assert_eq!(rows[0].detail, None, "counts of an origin nobody can read");
        assert_eq!(rows[0].problems.len(), 1);
        assert!(rows[0].orderable, "still part of the order while it exists");
        assert!(rows[0].removable);
    }

    #[test]
    fn spec_origins_have_no_order_and_a_folder_is_removable_like_a_store() {
        // Spec origins are not layered, so nothing is orderable; a folder came
        // from a setting, and the daemon takes it back out of it.
        let rows = origin_rows(&origins(
            Vec::new(),
            vec![
                spec_root("store:plans", "/s/plans", SpecRootKind::Store, Some("plans")),
                spec_root("path:/s/folder", "/s/folder", SpecRootKind::Folder, None),
                spec_root("path:/s/repo", "/s/repo", SpecRootKind::Project, None),
            ],
        ));
        assert!(
            rows.iter().all(|r| !r.orderable),
            "spec origins are never dragged"
        );
        assert_eq!(
            rows.iter().map(|r| r.removable).collect::<Vec<_>>(),
            [true, true, false],
            "a project's origin belongs to its repository"
        );
        assert_eq!(rows[1].key, "spec:path:/s/folder");
        assert_eq!(rows[1].kind, "Folder");
    }

    #[test]
    fn a_freeform_origin_is_removable_and_has_neither_an_order_nor_overrides() {
        let mut notes = LibraryOrigin::freeform("notes", "/notes");
        notes.documents = Some(1);
        let mut all = origins(vec![knowledge_root("store:acme", "acme")], Vec::new());
        all.origins.push(notes.clone());
        let rows = origin_rows(&all);
        let row = rows.last().expect("the freeform row");
        assert_eq!(row.origin_type, OriginType::Freeform);
        assert_eq!(row.kind, "Folder");
        assert_eq!(row.detail.as_deref(), Some("1 document"));
        assert!(row.removable);
        assert!(!row.orderable, "no handle: freeform origins do not layer");
        assert!(!notes.takes_overrides(), "and nothing is overridden into one");
        // The knowledge origin beside it keeps both.
        assert!(rows[0].orderable && all.origins[0].takes_overrides());
    }
}
