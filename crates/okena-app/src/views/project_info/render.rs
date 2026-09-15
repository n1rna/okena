//! Rendering for the project info panel.
//!
//! Sections in the agent panel's order — what this is, where the work lands,
//! who is working on it — so a repo and a session read alike beside their
//! terminals.

use super::model::{compact_links, pr_caption};
use super::{ProjectInfo, ProjectInfoKind, ProjectInfoPanel};
use crate::theme::theme;
use crate::ui::tokens::ui_text_ms;
use crate::views::agent_session::AgentSessionInfo;
use crate::views::components::WorktreeSummary;
use crate::views::components::asset_row::{ci_checks_chip, readiness_chips};
use crate::views::components::worktree_card::{chip, ci_chip_style, pr_chip_style};
use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::{h_flex, v_flex};
use okena_core::api::RepoPullRequest;
use okena_core::project_map::{MapStatus, ProjectMapState};

/// The map manifest's file name, as `okena-knowledge` writes it.
pub(super) const MANIFEST_FILE: &str = "project-map.yaml";

/// What pressing one of the panel's chips opens.
#[derive(Clone, Debug)]
pub(super) enum ChipTarget {
    /// The project's `project-map.yaml`.
    Manifest,
    /// Knowledge on a store the project follows, by root key.
    Store(String),
    /// Another project's info panel, by its daemon id.
    Project(String),
}

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

    /// The repository's map: the manifest chip, the stores the project
    /// follows, the menu over everything it owns, and the launcher that scans
    /// it. The entries themselves live in the menu, not on the panel.
    fn render_map(&self, project_id: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx);
        let mut out = Vec::new();
        let state = self.map.as_ref().map(|report| &report.state);
        let (status, color) = match state {
            None => ("Reading\u{2026}".to_string(), t.text_muted),
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
        // Any manifest, even a broken one, is a map to bring up to date — and
        // a file to open. Unscanned there is nothing to open, so the chip is
        // only a status.
        let mapped = matches!(
            state,
            Some(ProjectMapState::Scanned { .. } | ProjectMapState::Invalid { .. })
        );
        out.push(
            self.action_chip(
                "manifest".to_string(),
                if mapped {
                    format!("{MANIFEST_FILE} \u{b7} {status}")
                } else {
                    status
                },
                color,
                mapped.then(|| format!("Open {MANIFEST_FILE}.")),
                mapped.then_some(ChipTarget::Manifest),
                cx,
            ),
        );

        // One chip per store the project follows. A project that follows none
        // shows no row at all rather than a placeholder.
        let stores = self.store_chips(cx);
        if !stores.is_empty() {
            out.push(
                h_flex()
                    .gap(px(4.0))
                    .flex_wrap()
                    .children(stores.into_iter().map(|store| {
                        let available = store.root_key.is_some();
                        self.action_chip(
                            format!("store-{}", store.name),
                            store.name.clone(),
                            if available {
                                t.text_secondary
                            } else {
                                t.text_muted
                            },
                            store
                                .reason
                                .clone()
                                .or_else(|| Some(format!("Open {} in Knowledge.", store.name))),
                            store.root_key.clone().map(ChipTarget::Store),
                            cx,
                        )
                    }))
                    .into_any_element(),
            );
        }

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

        // Everything the project owns, behind one button: its map entries,
        // specs, knowledge docs, skills, agents and links.
        out.push(self.menu.clone().into_any_element());

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
        .busy(self.scan_starting.then_some("Starting\u{2026}"))
        .on_launch(cx.listener(|this, command: &SharedString, _window, cx| {
            this.start_scan(command.to_string(), cx);
        }));
        out.push(launcher.into_any_element());
        out
    }

    /// This project's links, compact: a chip per other project it uses and
    /// per project using it, what neither matched, and the scan that looks for
    /// more. Every link's detail is a row in the menu.
    fn render_links(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx);
        let mut out = Vec::new();
        let Some(links) = self.links.as_ref() else {
            out.push(self.note("Reading\u{2026}", cx));
            return out;
        };
        let Some(me) = self.daemon_map_id(cx) else {
            return out;
        };
        let compact = compact_links(links, &me);

        for (heading, chips) in [("Uses", &compact.uses), ("Used by", &compact.used_by)] {
            if chips.is_empty() {
                continue;
            }
            out.push(
                h_flex()
                    .gap(px(4.0))
                    .flex_wrap()
                    .items_center()
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(heading),
                    )
                    .children(chips.iter().map(|chip| {
                        self.action_chip(
                            format!("link-{heading}-{}", chip.project_id),
                            chip.name.clone(),
                            t.text_secondary,
                            // What the link is made of, which the chip itself
                            // has no room for.
                            Some(chip.interfaces.join(", ")),
                            Some(ChipTarget::Project(chip.project_id.clone())),
                            cx,
                        )
                    }))
                    .into_any_element(),
            );
        }
        if compact.unresolved > 0 {
            out.push(self.note(format!("{} unresolved", compact.unresolved), cx));
        }
        if compact.uses.is_empty() && compact.used_by.is_empty() && compact.unresolved == 0 {
            out.push(self.note("No links to other scanned projects.", cx));
        }
        if let Some(notice) = &self.links_notice {
            out.push(self.note(notice.clone(), cx));
        }
        if let Some(error) = &self.links_error {
            out.push(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.warning))
                    .child(error.clone())
                    .into_any_element(),
            );
        }

        // Over this repository and every other one with a map: a picker over
        // the whole set belongs to the cross-project view.
        let others: Vec<String> = links
            .projects
            .iter()
            .filter(|p| p.project_id != me && p.status == MapStatus::Scanned)
            .map(|p| p.project_id.clone())
            .collect();
        if others.is_empty() {
            out.push(self.note("Scan another project to look for links between them.", cx));
            return out;
        }
        let subtitle = format!(
            "One agent looks for links between this and {} other scanned {}, and writes each into both maps.",
            others.len(),
            if others.len() == 1 {
                "project"
            } else {
                "projects"
            }
        );
        let mut project_ids = vec![me.clone()];
        project_ids.extend(others);
        let launcher = okena_ui::agent_launcher::AgentLauncher::new(
            SharedString::from(format!("project-info-links-{me}")),
            "Scan links",
        )
        .subtitle(subtitle)
        .options(crate::views::agent_session::launch_options(
            self.default_agent.as_deref(),
            &t,
        ))
        .preferred(self.default_agent.clone())
        .busy(self.links_starting.then_some("Starting\u{2026}"))
        .on_launch(
            cx.listener(move |this, command: &SharedString, _window, cx| {
                this.start_links_scan(command.to_string(), project_ids.clone(), cx);
            }),
        );
        out.push(launcher.into_any_element());
        out
    }

    /// Open what a chip stands for.
    fn open_chip(&mut self, target: ChipTarget, cx: &mut Context<Self>) {
        match target {
            ChipTarget::Manifest => self.open_manifest(cx),
            ChipTarget::Store(root_key) => self.open_knowledge_root(root_key, cx),
            ChipTarget::Project(daemon_id) => self.open_project_info(daemon_id, cx),
        }
    }

    /// A chip that opens something when pressed. Without a `target` it is
    /// muted and inert — a store that is not on this machine — and `tip` then
    /// says why.
    fn action_chip(
        &self,
        key: String,
        label: String,
        color: u32,
        tip: Option<String>,
        target: Option<ChipTarget>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let mut chip = h_flex()
            .id(SharedString::from(format!("project-info-chip-{key}")))
            .flex_shrink_0()
            .max_w_full()
            .min_w_0()
            .items_center()
            .gap(px(4.0))
            .px(px(6.0))
            .py(px(1.0))
            .rounded(px(4.0))
            .bg(rgb(t.bg_secondary))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(color))
            .child(div().min_w_0().truncate().child(label));
        if let Some(tip) = tip {
            let tip = SharedString::from(tip);
            chip = chip.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx));
        }
        if let Some(target) = target {
            chip = chip
                .cursor_pointer()
                .hover(|style| style.bg(rgb(t.bg_hover)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        this.open_chip(target.clone(), cx);
                    }),
                );
        }
        chip.into_any_element()
    }

    /// One open pull request of the project's repository: the session PR
    /// card's chips, with who opened it and where it merges. The title line
    /// opens it on GitHub; the chips below keep their own clicks, so the CI
    /// chip still opens its checks.
    fn render_pull_request(&self, pr: &RepoPullRequest, cx: &App) -> AnyElement {
        let t = theme(cx);
        let (color, label) = pr_chip_style(pr.pr.number, &pr.pr.state, &t);
        let mut chips = vec![chip(label, color, cx)];
        chips.extend(readiness_chips(&pr.pr, cx));
        if let Some(summary) = pr.ci.clone() {
            chips.push(ci_checks_chip(
                SharedString::from(format!("project-info-pr-ci-{}", pr.pr.url)),
                summary,
                Some(pr.pr.clone()),
                cx,
            ));
        }
        let url = pr.pr.url.clone();
        v_flex()
            .w_full()
            .min_w_0()
            .gap(px(4.0))
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(4.0))
            .bg(rgb(t.bg_primary))
            .border_1()
            .border_color(rgb(t.border))
            .child(
                h_flex()
                    .id(SharedString::from(format!("project-info-pr-{}", pr.pr.url)))
                    .cursor_pointer()
                    .w_full()
                    .min_w_0()
                    .items_start()
                    .gap(px(7.0))
                    .rounded(px(3.0))
                    .hover(|style| style.bg(rgb(t.bg_hover)))
                    .child(
                        svg()
                            .path("icons/git-pull-request.svg")
                            .flex_shrink_0()
                            .mt(px(2.0))
                            .size(px(13.0))
                            .text_color(rgb(color)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(1.0))
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(t.text_primary))
                                    .child(pr.title.clone()),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(pr_caption(pr)),
                            ),
                    )
                    .child(
                        svg()
                            .path("icons/external-link.svg")
                            .flex_shrink_0()
                            .mt(px(2.0))
                            .size(px(12.0))
                            .text_color(rgb(t.text_muted)),
                    )
                    .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                        cx.open_url(&url);
                    }),
            )
            .child(
                h_flex()
                    .pl(px(20.0))
                    .gap(px(4.0))
                    .flex_wrap()
                    .children(chips),
            )
            .into_any_element()
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
            .children(s.subject().map(|subject| {
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
                            this.open_diff(&diff_id, None, cx);
                        }),
                    ),
            );
        }

        // ── What it is made of ───────────────────────────────────────────────
        // Only a repository is mapped: a worktree is a checkout of one, and the
        // map is committed in the repository.
        if let Some(map_project) = self.map_project_id(cx) {
            body = body.child(self.section_heading("MAP", None, cx));
            for element in self.render_map(&map_project, cx) {
                body = body.child(element);
            }
            body = body.child(self.section_heading("LINKS", None, cx));
            for element in self.render_links(cx) {
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
                    |this: &mut Self, id, mode, cx| this.open_diff(id, mode, cx),
                    cx,
                ));
            }
        }

        // ── What is waiting to merge ─────────────────────────────────────────
        // Every open PR in the repository, from anyone; a worktree shows its
        // repository's. No list — not on github.com, or no token — no section.
        if let Some(prs) = &info.pull_requests {
            body = body.child(self.section_heading("PULL REQUESTS", Some(prs.len()), cx));
            if prs.is_empty() {
                body = body.child(self.note("None open.", cx));
            }
            for pr in prs {
                body = body.child(self.render_pull_request(pr, cx));
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
