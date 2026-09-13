//! Rendering for the project info panel.
//!
//! Sections in the agent panel's order — what this is, where the work lands,
//! who is working on it — so a repo and a session read alike beside their
//! terminals.

use super::model::{area_labels, group_interfaces};
use super::{ProjectInfo, ProjectInfoKind, ProjectInfoPanel};
use crate::theme::theme;
use crate::ui::tokens::ui_text_ms;
use crate::views::agent_session::AgentSessionInfo;
use crate::views::components::WorktreeSummary;
use crate::views::components::worktree_card::{chip, ci_chip_style, pr_chip_style};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::project_map::{ProjectMap, ProjectMapState};

impl ProjectInfoPanel {
    fn section_heading(&self, label: &str, count: Option<usize>, cx: &App) -> AnyElement {
        let t = theme(cx);
        h_flex()
            .items_center()
            .justify_between()
            .pt(px(8.0))
            .pb(px(2.0))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(label.to_string()),
            )
            // Optional: a bare "0" beside a heading that is not a list reads as
            // "none found" rather than "not countable".
            .children(count.map(|n| {
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(format!("{n}"))
                    .into_any_element()
            }))
            .into_any_element()
    }

    fn note(&self, text: impl Into<String>, cx: &App) -> AnyElement {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(text.into())
            .into_any_element()
    }

    /// Chips for the checkout itself: branch, diff, divergence, PR, pipeline.
    fn git_chips(&self, info: &ProjectInfo, cx: &App) -> Vec<AnyElement> {
        let t = theme(cx);
        let git = &info.git;
        let mut chips = Vec::new();
        if let Some(branch) = &git.branch {
            chips.push(chip(branch.clone(), t.text_secondary, cx));
        }
        if git.has_changes() {
            chips.push(chip(
                format!("+{} −{}", git.lines_added, git.lines_removed),
                t.text_muted,
                cx,
            ));
        }
        match (git.ahead, git.behind) {
            (Some(a), Some(b)) if a > 0 && b > 0 => {
                chips.push(chip(format!("↑{a} ↓{b}"), t.warning, cx));
            }
            (Some(a), _) if a > 0 => chips.push(chip(format!("↑{a}"), t.warning, cx)),
            (_, Some(b)) if b > 0 => chips.push(chip(format!("↓{b}"), t.warning, cx)),
            _ => {}
        }
        if let Some((number, state)) = &git.pr {
            let (color, label) = pr_chip_style(*number, state, &t);
            chips.push(chip(label, color, cx));
        }
        if let Some((status, passed, failed, pending)) = &git.ci {
            let (color, label) = ci_chip_style(status, *passed, *failed, *pending, &t);
            chips.push(chip(label, color, cx));
        }
        chips
    }

    /// The repository's map: its status, and the launcher that scans it.
    fn render_map(&self, project_id: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx);
        let mut out = Vec::new();
        let state = self.map.as_ref().map(|report| &report.state);
        let (status, color) = match state {
            None => ("Reading…".to_string(), t.text_muted),
            Some(ProjectMapState::NotScanned) => ("Not scanned".to_string(), t.text_muted),
            Some(ProjectMapState::Scanned { map }) => (
                match &map.scanned {
                    Some(stamp) => format!(
                        "Scanned at {}",
                        stamp.commit.get(..7).unwrap_or(&stamp.commit)
                    ),
                    None => "Scanned".to_string(),
                },
                t.success,
            ),
            Some(ProjectMapState::Invalid { .. }) => ("Invalid".to_string(), t.warning),
        };
        out.push(
            h_flex()
                .gap(px(4.0))
                .child(chip(status, color, cx))
                .into_any_element(),
        );
        if let Some(problem) = state.and_then(ProjectMapState::problem) {
            out.push(self.note(problem.message.clone(), cx));
            if let Some(fix) = &problem.fix {
                out.push(self.note(fix.clone(), cx));
            }
        }
        if let Some(notice) = &self.scan_notice {
            out.push(self.note(notice.clone(), cx));
        }
        if let Some(error) = &self.scan_error {
            out.push(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.warning))
                    .child(error.clone())
                    .into_any_element(),
            );
        }

        // Any manifest, even a broken one, is a map to bring up to date.
        let mapped = matches!(
            state,
            Some(ProjectMapState::Scanned { .. } | ProjectMapState::Invalid { .. })
        );
        let launcher = okena_ui::agent_launcher::AgentLauncher::new(
            SharedString::from(format!("project-info-scan-{project_id}")),
            if mapped { "Rescan" } else { "Scan" },
        )
        .subtitle("An agent maps this repository into its knowledge folder. Nothing is committed.")
        .options(crate::views::agent_session::launch_options(
            self.default_agent.as_deref(),
            &t,
        ))
        .preferred(self.default_agent.clone())
        .busy(self.scan_starting.then_some("Starting…"))
        .on_launch(cx.listener(|this, command: &SharedString, _window, cx| {
            this.start_scan(command.to_string(), cx);
        }));
        out.push(launcher.into_any_element());

        if let Some(map) = state.and_then(ProjectMapState::map) {
            out.extend(self.render_map_contents(map, cx));
        }
        out
    }

    /// What the scan found: the project, its areas and concepts, what crosses
    /// its boundary, and how it is built and run. Entries with a doc open it.
    fn render_map_contents(&self, map: &ProjectMap, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        out.push(self.map_item(
            "project".to_string(),
            map.project.name.clone(),
            Some(map.project.description.clone()),
            None,
            map.project.doc.clone(),
            cx,
        ));

        if !map.areas.is_empty() {
            out.push(self.map_subheading("Areas", map.areas.len(), cx));
            for area in &map.areas {
                out.push(self.map_item(
                    format!("area-{}", area.id),
                    area.label().to_string(),
                    Some(area.description.clone()),
                    Some(area.paths.join(", ")),
                    area.doc.clone(),
                    cx,
                ));
            }
        }

        if !map.concepts.is_empty() {
            out.push(self.map_subheading("Concepts", map.concepts.len(), cx));
            for concept in &map.concepts {
                out.push(self.map_item(
                    format!("concept-{}", concept.id),
                    concept.label().to_string(),
                    Some(concept.description.clone()),
                    Some(format!("in {}", area_labels(map, &concept.areas))),
                    concept.doc.clone(),
                    cx,
                ));
            }
        }

        for (heading, list) in [("Exposes", &map.exposes), ("Consumes", &map.consumes)] {
            if list.is_empty() {
                continue;
            }
            out.push(self.map_subheading(heading, list.len(), cx));
            for (kind, items) in group_interfaces(list) {
                out.push(self.note(kind.label(), cx));
                for (i, item) in items.into_iter().enumerate() {
                    out.push(
                        self.map_item(
                            format!("{heading}-{}-{i}", kind.id()),
                            item.name.clone(),
                            item.description.clone(),
                            (!item.areas.is_empty())
                                .then(|| format!("in {}", area_labels(map, &item.areas))),
                            None,
                            cx,
                        ),
                    );
                }
            }
        }

        if !map.ci.is_empty() {
            out.push(self.map_subheading("CI/CD", map.ci.len(), cx));
            for (i, pipeline) in map.ci.iter().enumerate() {
                let meta = match &pipeline.provider {
                    Some(provider) => format!("{provider} · {}", pipeline.files.join(", ")),
                    None => pipeline.files.join(", "),
                };
                out.push(self.map_item(
                    format!("ci-{i}"),
                    pipeline.name.clone(),
                    pipeline.description.clone(),
                    Some(meta),
                    None,
                    cx,
                ));
            }
        }

        if !map.infrastructure.is_empty() {
            out.push(self.map_subheading("Infrastructure", map.infrastructure.len(), cx));
            for (i, resource) in map.infrastructure.iter().enumerate() {
                let meta = resource
                    .kind
                    .iter()
                    .cloned()
                    .chain((!resource.files.is_empty()).then(|| resource.files.join(", ")))
                    .collect::<Vec<_>>()
                    .join(" · ");
                out.push(self.map_item(
                    format!("infra-{i}"),
                    resource.name.clone(),
                    resource.description.clone(),
                    (!meta.is_empty()).then_some(meta),
                    None,
                    cx,
                ));
            }
        }
        out
    }

    /// A group heading inside the map, a step below the panel's sections.
    fn map_subheading(&self, label: &str, count: usize, cx: &App) -> AnyElement {
        let t = theme(cx);
        h_flex()
            .items_center()
            .justify_between()
            .pt(px(6.0))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(label.to_string()),
            )
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(format!("{count}")),
            )
            .into_any_element()
    }

    /// One entry of the map: a name, what it is, where it is, and its doc.
    fn map_item(
        &self,
        key: String,
        title: String,
        detail: Option<String>,
        meta: Option<String>,
        doc: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let mut item = v_flex()
            .id(SharedString::from(format!("project-info-map-{key}")))
            .w_full()
            .min_w_0()
            .gap(px(2.0))
            .px(px(8.0))
            .py(px(5.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(rgb(t.border))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_primary))
                            .child(title),
                    )
                    .children(doc.is_some().then(|| {
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child("doc ↗")
                    })),
            )
            .children(detail.filter(|d| !d.trim().is_empty()).map(|detail| {
                div()
                    .w_full()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(detail)
            }))
            .children(meta.map(|meta| {
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(meta)
            }));
        if let Some(path) = doc {
            item = item
                .cursor_pointer()
                .hover(|style| style.bg(rgb(t.bg_hover)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        this.open_map_doc(path.clone(), cx);
                    }),
                );
        }
        item.into_any_element()
    }

    /// One agent session working this project.
    ///
    /// Its own card rather than the session panel at a smaller size: here the
    /// question is only which agents are on this project and whether one wants
    /// you — the rest is a click away, in the session's own column.
    fn render_session_card(&self, s: &AgentSessionInfo, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let activity = s.activity();
        let mut chips = vec![chip(activity.label(), activity.color(&t), cx)];
        if let Some(agent) = &s.agent {
            chips.push(chip(agent.clone(), t.text_secondary, cx));
        }
        if !s.assets.is_empty() {
            chips.push(chip(format!("{} produced", s.assets.len()), t.success, cx));
        }

        let open_id = s.project_id.clone();
        v_flex()
            .id(SharedString::from(format!(
                "project-info-session-{}",
                s.project_id
            )))
            .cursor_pointer()
            .w_full()
            .min_w_0()
            .gap(px(3.0))
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(4.0))
            .bg(rgb(t.bg_primary))
            .border_1()
            .border_color(rgb(t.border))
            .hover(|style| style.bg(rgb(t.bg_hover)))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_primary))
                    .child(s.name.clone()),
            )
            .children(s.kind.subject().map(|subject| {
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(subject)
            }))
            // What the agent says it is doing, beside what its terminal shows.
            .children(s.status.clone().map(|status| {
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(format!("“{status}”"))
            }))
            .child(h_flex().gap(px(4.0)).flex_wrap().children(chips))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.open_project(open_id.clone(), cx);
                }),
            )
            .into_any_element()
    }
}

impl Render for ProjectInfoPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let Some(info) = self.info(cx) else {
            // The project went away. Say so rather than rendering an empty
            // shell that looks like a load that never finishes.
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .bg(rgb(t.bg_primary))
                .child(self.note("This project is gone.", cx))
                .into_any_element();
        };

        // Read out of the workspace up front: the cards below take listeners,
        // which need the context mutably.
        let (worktrees, sessions): (Vec<WorktreeSummary>, Vec<AgentSessionInfo>) = {
            let ws = self.workspace.read(cx);
            (
                info.worktrees
                    .iter()
                    .filter_map(|id| WorktreeSummary::collect(ws, id))
                    .collect(),
                info.sessions
                    .iter()
                    .filter_map(|id| AgentSessionInfo::collect(ws, &self.terminals, id))
                    .collect(),
            )
        };

        let mut body = v_flex()
            .id(SharedString::from(format!(
                "project-info-body-{}",
                info.project_id
            )))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(10.0))
            .pb(px(12.0))
            .gap(px(4.0));

        // ── What this is ─────────────────────────────────────────────────────
        let heading = match &info.kind {
            ProjectInfoKind::Repo => "REPOSITORY",
            ProjectInfoKind::Worktree { .. } => "WORKTREE",
        };
        body = body.child(self.section_heading(heading, None, cx));
        if let ProjectInfoKind::Worktree { repo: Some(repo) } = &info.kind {
            body = body.child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(format!("Worktree of {repo}")),
            );
        }
        let chips = self.git_chips(&info, cx);
        if !chips.is_empty() {
            body = body.child(h_flex().gap(px(4.0)).flex_wrap().children(chips));
        }
        body = body.child(
            div()
                .w_full()
                .min_w_0()
                .truncate()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(info.path.clone()),
        );
        // Only when something changed: a button that opens an empty diff is
        // worse than no button.
        if info.git.has_changes() {
            let diff_id = info.project_id.clone();
            body = body.child(
                div()
                    .id("project-info-diff")
                    .cursor_pointer()
                    .mt(px(4.0))
                    .px(px(10.0))
                    .py(px(5.0))
                    .rounded(px(4.0))
                    .bg(rgb(t.bg_secondary))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_primary))
                    .child("Review changes")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.open_diff(&diff_id, cx);
                        }),
                    ),
            );
        }

        // ── What it is made of ───────────────────────────────────────────────
        // Only a repository is mapped: a worktree is a checkout of one, and the
        // map is committed in the repository.
        if info.kind == ProjectInfoKind::Repo {
            body = body.child(self.section_heading("MAP", None, cx));
            for element in self.render_map(&info.project_id, cx) {
                body = body.child(element);
            }
        }

        // ── Where the work lands ─────────────────────────────────────────────
        // A worktree is itself where work lands; only a repo has others.
        if info.kind == ProjectInfoKind::Repo {
            body = body.child(self.section_heading("WORKTREES", Some(worktrees.len()), cx));
            if worktrees.is_empty() {
                body = body.child(self.note("None open.", cx));
            }
            for summary in &worktrees {
                body = body.child(crate::views::components::render_worktree_card(
                    summary,
                    |this: &mut Self, id, cx| this.open_project(id.to_string(), cx),
                    |this: &mut Self, id, cx| this.open_diff(id, cx),
                    cx,
                ));
            }
        }

        // ── Who is working on it ─────────────────────────────────────────────
        body = body.child(self.section_heading("AGENTS", Some(sessions.len()), cx));
        if sessions.is_empty() {
            body = body.child(self.note("No sessions working its tasks.", cx));
        }
        for session in &sessions {
            body = body.child(self.render_session_card(session, cx));
        }

        v_flex()
            .size_full()
            .bg(rgb(t.bg_primary))
            .child(body)
            .into_any_element()
    }
}
