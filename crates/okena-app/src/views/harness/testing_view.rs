//! Harness → Testing: watching agents verify their work.
//!
//! Every verification run agents reported through okena's MCP, newest first and
//! grouped by the task each one verified. A run shows its plan as soon as it is
//! written, then each step as it advances — the running one marked live,
//! finished ones with the agent's reason and the evidence it attached.
//!
//! An entity rather than a render helper on the pane: it follows the workspace
//! mirror the runs arrive in, and it keeps its own state (which log tails are
//! open) and its own ways out — to the session that reported a run.

use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms, ui_text_sm};
use crate::workspace::focus::FocusManager;
use crate::workspace::state::{ProjectData, WindowId, Workspace};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::{
    VerificationRun, VerificationStep, VerificationStepState, VerificationVerdict,
};
use okena_core::tasks::TaskRef;
use std::collections::HashSet;

use super::HarnessPane;

// ─── What the canvas shows ───────────────────────────────────────────────────

/// A run, with the session that reported it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ListedRun {
    pub(crate) project_id: String,
    pub(crate) session: String,
    pub(crate) run: VerificationRun,
}

/// The runs of one task, newest first.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RunGroup {
    /// `None` for runs of sessions not started for a task.
    pub(crate) task: Option<TaskRef>,
    pub(crate) runs: Vec<ListedRun>,
}

/// Every session's runs, grouped by task. Groups are ordered by their newest
/// run, so the task verified most recently leads.
pub(crate) fn group_runs<'a>(projects: impl IntoIterator<Item = &'a ProjectData>) -> Vec<RunGroup> {
    let mut listed: Vec<ListedRun> = projects
        .into_iter()
        .flat_map(|p| {
            p.verification_runs.iter().map(|run| ListedRun {
                project_id: p.id.clone(),
                session: p.name.clone(),
                run: run.clone(),
            })
        })
        .collect();
    listed.sort_by(|a, b| {
        b.run
            .created_at
            .cmp(&a.run.created_at)
            .then_with(|| a.run.id.cmp(&b.run.id))
    });

    let mut groups: Vec<RunGroup> = Vec::new();
    for entry in listed {
        let key = entry.run.task.as_ref().map(|t| &t.id);
        match groups
            .iter_mut()
            .find(|g| g.task.as_ref().map(|t| &t.id) == key)
        {
            Some(group) => group.runs.push(entry),
            None => groups.push(RunGroup {
                task: entry.run.task.clone(),
                runs: vec![entry],
            }),
        }
    }
    groups
}

/// How a state is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Muted,
    Live,
    Good,
    Bad,
}

impl Tone {
    fn color(self, cx: &App) -> u32 {
        let t = theme(cx);
        match self {
            Tone::Muted => t.text_muted,
            Tone::Live => t.term_blue,
            Tone::Good => t.success,
            Tone::Bad => t.error,
        }
    }
}

/// Where a run as a whole stands.
pub(crate) fn run_status(run: &VerificationRun) -> (String, Tone) {
    if let Some(verdict) = run.verdict {
        let tone = match verdict {
            VerificationVerdict::Passed => Tone::Good,
            VerificationVerdict::Failed => Tone::Bad,
            VerificationVerdict::Inconclusive => Tone::Muted,
        };
        return (verdict.label().to_string(), tone);
    }
    let total = run.steps.len();
    if let Some(i) = run
        .steps
        .iter()
        .position(|s| s.state == VerificationStepState::Running)
    {
        return (format!("running step {} of {total}", i + 1), Tone::Live);
    }
    if !run.has_started() {
        return ("planned".into(), Tone::Muted);
    }
    if run.steps.iter().all(|s| s.state.is_finished()) {
        return ("awaiting verdict".into(), Tone::Live);
    }
    let done = run.steps.iter().filter(|s| s.state.is_finished()).count();
    (format!("{done} of {total} done"), Tone::Live)
}

/// One step, as its row shows it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StepRow {
    pub(crate) number: usize,
    pub(crate) title: String,
    pub(crate) proves: Option<String>,
    pub(crate) glyph: &'static str,
    pub(crate) tone: Tone,
    pub(crate) state: &'static str,
    /// The agent's account of the result. Always present for a failure: a red
    /// mark alone says nothing about what to fix.
    pub(crate) reason: Option<String>,
    pub(crate) duration: Option<String>,
    pub(crate) log_tail: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) screenshot: Option<String>,
}

pub(crate) fn step_row(index: usize, step: &VerificationStep) -> StepRow {
    let (glyph, tone) = match step.state {
        VerificationStepState::Pending => ("○", Tone::Muted),
        VerificationStepState::Running => ("●", Tone::Live),
        VerificationStepState::Passed => ("✓", Tone::Good),
        VerificationStepState::Failed => ("✕", Tone::Bad),
        VerificationStepState::Skipped => ("–", Tone::Muted),
    };
    let reason = match (step.state, step.reason.clone()) {
        (VerificationStepState::Failed, None) => Some("Failed — the agent gave no reason".into()),
        (_, reason) => reason,
    };
    let duration = match (step.state.is_finished(), step.started_at, step.ended_at) {
        (true, Some(start), Some(end)) => Some(format_duration(end.saturating_sub(start))),
        _ => None,
    };
    StepRow {
        number: index + 1,
        title: step.title.clone(),
        proves: (!step.proves.is_empty()).then(|| step.proves.clone()),
        glyph,
        tone,
        state: step.state.label(),
        reason,
        duration,
        log_tail: step.evidence.log_tail.clone(),
        url: step.evidence.url.clone(),
        screenshot: step.evidence.screenshot_path.clone(),
    }
}

/// A span of milliseconds, to the precision a person reads it at.
pub(crate) fn format_duration(ms: u64) -> String {
    match ms {
        0..1_000 => format!("{ms} ms"),
        1_000..60_000 => format!("{} s", ms / 1_000),
        _ => format!("{} m {} s", ms / 60_000, (ms / 1_000) % 60),
    }
}

use okena_ui::ago::{format_ago, now_millis};

// ─── The view ────────────────────────────────────────────────────────────────

pub(crate) struct TestingView {
    workspace: Entity<Workspace>,
    focus_manager: Entity<FocusManager>,
    window_id: WindowId,
    /// Log tails shown in full, by run id and step index. Shut by default: a
    /// wall of output per step would bury the plan the canvas is for.
    open_logs: HashSet<(String, usize)>,
    scroll: ScrollHandle,
}

impl TestingView {
    pub(crate) fn new(
        workspace: Entity<Workspace>,
        focus_manager: Entity<FocusManager>,
        window_id: WindowId,
        cx: &mut Context<Self>,
    ) -> Self {
        // Runs arrive with the daemon's state, so every step an agent reports
        // lands here as the mirror updates.
        cx.observe(&workspace, |_, _, cx| cx.notify()).detach();
        Self {
            workspace,
            focus_manager,
            window_id,
            open_logs: HashSet::new(),
            scroll: ScrollHandle::new(),
        }
    }

    fn toggle_log(&mut self, run_id: String, index: usize, cx: &mut Context<Self>) {
        let key = (run_id, index);
        if !self.open_logs.remove(&key) {
            self.open_logs.insert(key);
        }
        cx.notify();
    }

    fn render_empty(&self, cx: &App) -> AnyElement {
        let t = theme(cx);
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .px(px(24.0))
            .child(
                div()
                    .text_size(ui_text(14.0, cx))
                    .text_color(rgb(t.text_primary))
                    .child("No verification runs yet"),
            )
            .child(
                div()
                    .max_w(px(460.0))
                    .text_center()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(t.text_muted))
                    .child(
                        "When an agent verifies its work, its plan shows up here as soon as it \
                         is written, then each step as it passes or fails. Agents okena starts \
                         report through its okena_test_* tools.",
                    ),
            )
            .into_any_element()
    }

    fn render_group(&self, group: &RunGroup, now: u64, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let heading = match &group.task {
            Some(task) => h_flex()
                .gap(px(8.0))
                .items_center()
                .min_w_0()
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child(task.display_key.clone()),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(ui_text(13.0, cx))
                        .text_color(rgb(t.text_primary))
                        .child(task.title.clone()),
                ),
            None => h_flex().child(
                div()
                    .text_size(ui_text(13.0, cx))
                    .text_color(rgb(t.text_secondary))
                    .child("Not started for a task"),
            ),
        };
        v_flex()
            .gap(px(8.0))
            .child(heading)
            .children(group.runs.iter().map(|r| self.render_run(r, now, cx)))
            .into_any_element()
    }

    fn render_run(&self, listed: &ListedRun, now: u64, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let run = &listed.run;
        let (status, tone) = run_status(run);
        let color = tone.color(cx);
        let passed = run
            .steps
            .iter()
            .filter(|s| s.state == VerificationStepState::Passed)
            .count();
        let project_id = listed.project_id.clone();

        let header = h_flex()
            .gap(px(8.0))
            .items_center()
            .min_w_0()
            .child(
                div()
                    .flex_shrink_0()
                    .px(px(6.0))
                    .py(px(1.0))
                    .rounded(px(3.0))
                    .bg(with_alpha(color, 0.15))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(color))
                    .child(status),
            )
            .child(
                div()
                    .id(SharedString::from(format!("testing-session-{}", run.id)))
                    .min_w_0()
                    .truncate()
                    .cursor_pointer()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(t.text_primary))
                    .hover(|s| s.underline())
                    .tooltip(|window, cx| {
                        gpui_component::tooltip::Tooltip::new("Go to this agent's session")
                            .build(window, cx)
                    })
                    .child(listed.session.clone())
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        crate::views::components::project_nav::focus_project(
                            &this.workspace,
                            &this.focus_manager,
                            this.window_id,
                            &project_id,
                            cx,
                        );
                    })),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(format!(
                        "{passed}/{} passed · {}",
                        run.steps.len(),
                        format_ago(run.created_at, now)
                    )),
            );

        let mut card = v_flex()
            .gap(px(2.0))
            .p(px(10.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_secondary))
            .child(header);
        if let Some(worktree) = &run.worktree {
            card = card.child(
                div()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(worktree.clone()),
            );
        }
        if let Some(summary) = &run.summary {
            card = card.child(
                div()
                    .pt(px(4.0))
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(summary.clone()),
            );
        }
        card.child(
            v_flex().pt(px(6.0)).gap(px(4.0)).children(
                run.steps
                    .iter()
                    .enumerate()
                    .map(|(i, step)| self.render_step(&run.id, i, step, cx)),
            ),
        )
        .into_any_element()
    }

    fn render_step(
        &self,
        run_id: &str,
        index: usize,
        step: &VerificationStep,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let row = step_row(index, step);
        let color = row.tone.color(cx);
        let live = step.state == VerificationStepState::Running;
        let log_open = self.open_logs.contains(&(run_id.to_string(), index));

        let title = h_flex()
            .gap(px(8.0))
            .items_center()
            .min_w_0()
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(18.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(format!("{}.", row.number)),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(14.0))
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(color))
                    .child(row.glyph),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(if step.state == VerificationStepState::Pending {
                        t.text_secondary
                    } else {
                        t.text_primary
                    }))
                    .child(row.title.clone()),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(color))
                    .child(match &row.duration {
                        Some(d) => format!("{} · {d}", row.state),
                        None => row.state.to_string(),
                    }),
            );

        // Everything under the title is indented past the number and glyph, so
        // the titles line up down the plan.
        let mut detail = v_flex().pl(px(40.0)).gap(px(2.0));
        if let Some(proves) = &row.proves {
            detail = detail.child(
                div()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(format!("Proves: {proves}")),
            );
        }
        if let Some(reason) = &row.reason {
            detail = detail.child(
                div()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(if step.state == VerificationStepState::Failed {
                        t.error
                    } else {
                        t.text_secondary
                    }))
                    .child(reason.clone()),
            );
        }

        let mut evidence: Vec<AnyElement> = Vec::new();
        if row.log_tail.is_some() {
            let run_id = run_id.to_string();
            evidence.push(self.evidence_link(
                SharedString::from(format!("testing-log-{run_id}-{index}")),
                if log_open { "Hide log" } else { "Log" },
                cx.listener(move |this, _, _window, cx| this.toggle_log(run_id.clone(), index, cx)),
                cx,
            ));
        }
        if let Some(url) = row.url.clone() {
            evidence.push(self.evidence_link(
                SharedString::from(format!("testing-url-{run_id}-{index}")),
                "Open link",
                move |_, _, _| okena_core::process::open_url(&url),
                cx,
            ));
        }
        if let Some(path) = row.screenshot.clone() {
            evidence.push(self.evidence_link(
                SharedString::from(format!("testing-shot-{run_id}-{index}")),
                "Screenshot",
                move |_, _, _| okena_core::process::open_url(&path),
                cx,
            ));
        }
        if !evidence.is_empty() {
            detail = detail.child(h_flex().gap(px(6.0)).pt(px(2.0)).children(evidence));
        }
        if log_open && let Some(log) = &row.log_tail {
            detail = detail.child(
                div()
                    .mt(px(2.0))
                    .p(px(6.0))
                    .rounded(px(4.0))
                    .bg(rgb(t.bg_primary))
                    .border_1()
                    .border_color(rgb(t.border))
                    .font_family("monospace")
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(log.clone()),
            );
        }

        v_flex()
            .py(px(3.0))
            .px(px(4.0))
            .rounded(px(4.0))
            .when(live, |d| d.bg(with_alpha(color, 0.08)))
            .child(title)
            .child(detail)
            .into_any_element()
    }

    fn evidence_link(
        &self,
        id: SharedString,
        label: &'static str,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        cx: &App,
    ) -> AnyElement {
        let t = theme(cx);
        div()
            .id(id)
            .cursor_pointer()
            .px(px(6.0))
            .py(px(1.0))
            .rounded(px(3.0))
            .border_1()
            .border_color(rgb(t.border))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_secondary))
            .child(label)
            .on_click(on_click)
            .into_any_element()
    }
}

impl Render for TestingView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let groups = group_runs(&self.workspace.read(cx).data().projects);
        if groups.is_empty() {
            return self.render_empty(cx);
        }
        let now = now_millis();
        div()
            .id("testing-runs")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                v_flex()
                    .p(px(16.0))
                    .gap(px(20.0))
                    .max_w(px(960.0))
                    .children(groups.iter().map(|g| self.render_group(g, now, cx))),
            )
            .into_any_element()
    }
}

impl HarnessPane {
    pub(super) fn render_testing_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let toolbar = self.render_toolbar(Vec::new(), cx);
        let body = match &self.testing {
            Some(view) => view.clone().into_any_element(),
            None => div().into_any_element(),
        };
        v_flex()
            .size_full()
            .child(toolbar)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{Tone, format_duration, group_runs, run_status, step_row};
    use crate::workspace::state::ProjectData;
    use okena_core::api::{
        VerificationEvidence, VerificationRun, VerificationStep, VerificationStepState as S,
        VerificationVerdict,
    };
    use okena_core::tasks::{TaskId, TaskRef};

    fn step(title: &str, state: S) -> VerificationStep {
        VerificationStep {
            title: title.into(),
            proves: String::new(),
            state,
            started_at: None,
            ended_at: None,
            reason: None,
            evidence: VerificationEvidence::default(),
        }
    }

    fn run(
        id: &str,
        created_at: u64,
        task: Option<&str>,
        steps: Vec<VerificationStep>,
    ) -> VerificationRun {
        VerificationRun {
            id: id.into(),
            terminal_id: "t1".into(),
            task: task.map(|key| TaskRef {
                id: TaskId {
                    provider: "linear".into(),
                    external_id: format!("id-{key}"),
                },
                display_key: key.into(),
                title: format!("Task {key}"),
                url: String::new(),
                parent_id: None,
                parent_key: None,
            }),
            worktree: None,
            created_at,
            finished_at: None,
            verdict: None,
            summary: None,
            steps,
        }
    }

    fn session(id: &str, runs: Vec<VerificationRun>) -> ProjectData {
        let mut p: ProjectData = serde_json::from_value(serde_json::json!({
            "id": id, "name": format!("session {id}"), "path": "/p",
        }))
        .expect("project");
        p.verification_runs = runs;
        p
    }

    #[test]
    fn runs_list_newest_first_grouped_by_task() {
        let projects = [
            session(
                "a",
                vec![
                    run("r1", 100, Some("QBL-1"), vec![]),
                    run("r3", 300, Some("QBL-2"), vec![]),
                ],
            ),
            session(
                "b",
                vec![
                    run("r2", 200, Some("QBL-1"), vec![]),
                    run("r4", 400, None, vec![]),
                ],
            ),
        ];
        let groups = group_runs(&projects);
        let shape: Vec<(Option<String>, Vec<&str>)> = groups
            .iter()
            .map(|g| {
                (
                    g.task.as_ref().map(|t| t.display_key.clone()),
                    g.runs.iter().map(|r| r.run.id.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                (None, vec!["r4"]),
                (Some("QBL-2".into()), vec!["r3"]),
                (Some("QBL-1".into()), vec!["r2", "r1"]),
            ]
        );
        assert_eq!(groups[2].runs[0].session, "session b");
    }

    #[test]
    fn no_runs_means_the_empty_state() {
        assert!(group_runs(&[session("a", vec![])]).is_empty());
    }

    #[test]
    fn a_seeded_run_renders_every_step_state() {
        let mut passed = step("Build", S::Passed);
        passed.started_at = Some(1_000);
        passed.ended_at = Some(13_000);
        passed.reason = Some("compiles clean".into());
        passed.evidence.log_tail = Some("Finished dev".into());
        let mut failed = step("Log in", S::Failed);
        failed.reason = Some("redirect loops back to /login".into());
        failed.evidence.url = Some("http://localhost:3000/login".into());
        failed.evidence.screenshot_path = Some("/tmp/login.png".into());
        let mut running = step("Checkout", S::Running);
        running.proves = "orders are charged once".into();
        let r = run(
            "r",
            0,
            None,
            vec![
                passed,
                failed,
                running,
                step("Refund", S::Pending),
                step("Mobile", S::Skipped),
            ],
        );

        let rows: Vec<_> = r
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| step_row(i, s))
            .collect();
        let glance: Vec<_> = rows
            .iter()
            .map(|row| (row.number, row.glyph, row.tone, row.state))
            .collect();
        assert_eq!(
            glance,
            vec![
                (1, "✓", Tone::Good, "passed"),
                (2, "✕", Tone::Bad, "failed"),
                (3, "●", Tone::Live, "running"),
                (4, "○", Tone::Muted, "pending"),
                (5, "–", Tone::Muted, "skipped"),
            ]
        );
        assert_eq!(rows[0].duration.as_deref(), Some("12 s"));
        assert_eq!(rows[0].log_tail.as_deref(), Some("Finished dev"));
        assert_eq!(
            rows[1].reason.as_deref(),
            Some("redirect loops back to /login")
        );
        assert_eq!(rows[1].url.as_deref(), Some("http://localhost:3000/login"));
        assert_eq!(rows[1].screenshot.as_deref(), Some("/tmp/login.png"));
        assert_eq!(rows[2].proves.as_deref(), Some("orders are charged once"));
        assert_eq!(rows[2].duration, None, "a running step has no duration yet");
        assert_eq!(run_status(&r), ("running step 3 of 5".into(), Tone::Live));
    }

    #[test]
    fn a_failure_always_says_something() {
        let row = step_row(0, &step("x", S::Failed));
        assert!(row.reason.is_some_and(|r| !r.is_empty()));
    }

    #[test]
    fn a_run_reads_as_planned_in_progress_or_decided() {
        let planned = run("r", 0, None, vec![step("a", S::Pending)]);
        assert_eq!(run_status(&planned).0, "planned");

        let partway = run(
            "r",
            0,
            None,
            vec![step("a", S::Passed), step("b", S::Pending)],
        );
        assert_eq!(run_status(&partway).0, "1 of 2 done");

        let unclosed = run("r", 0, None, vec![step("a", S::Passed)]);
        assert_eq!(run_status(&unclosed).0, "awaiting verdict");

        let mut failed = unclosed.clone();
        failed.verdict = Some(VerificationVerdict::Failed);
        assert_eq!(run_status(&failed), ("failed".into(), Tone::Bad));
    }

    #[test]
    fn durations_read_at_a_human_precision() {
        assert_eq!(format_duration(850), "850 ms");
        assert_eq!(format_duration(12_400), "12 s");
        assert_eq!(format_duration(184_000), "3 m 4 s");
    }
}
