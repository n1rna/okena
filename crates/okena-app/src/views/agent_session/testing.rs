//! The Testing tab: the verification runs this agent reported.
//!
//! Every run the agent planned through okena's MCP, newest first. A run shows
//! its plan as soon as it is written, then each step as it advances — the
//! running one marked live, finished ones with the agent's reason and the
//! evidence it attached.
//!
//! Beside the agent rather than on a page of its own: a run is how one agent
//! is getting on, and looking for its card among every session's runs meant
//! leaving the agent to find out.

use super::{AgentSessionPanel, PanelTab};
use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text_md, ui_text_ms, ui_text_sm};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::{
    VerificationRun, VerificationStep, VerificationStepState, VerificationVerdict,
};
use okena_ui::ago::{format_ago, now_millis};

// ─── What the tab shows ──────────────────────────────────────────────────────

/// A session's runs, newest first.
pub(super) fn runs_newest_first(runs: &[VerificationRun]) -> Vec<VerificationRun> {
    let mut runs = runs.to_vec();
    runs.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    runs
}

/// Whether any run is under way: a step has started and no verdict is in. A
/// plan nobody has started yet is not activity, so it does not light the tab.
pub(super) fn any_run_in_progress(runs: &[VerificationRun]) -> bool {
    runs.iter().any(|r| r.has_started() && !r.is_finished())
}

/// The tab the panel shows. Testing only exists while the agent has runs, so a
/// selection left on it after they went — a daemon restart drops them — falls
/// back to Info rather than to an empty tab nobody can see is selected.
pub(super) fn visible_tab(selected: PanelTab, has_runs: bool) -> PanelTab {
    match selected {
        PanelTab::Testing if !has_runs => PanelTab::Info,
        tab => tab,
    }
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

// ─── Rendering ───────────────────────────────────────────────────────────────

impl AgentSessionPanel {
    /// This session's runs as the mirror has them now. Read each frame, so
    /// every step the agent reports lands here as the workspace updates.
    pub(super) fn verification_runs(&self, cx: &App) -> Vec<VerificationRun> {
        self.workspace
            .read(cx)
            .project(&self.project_id)
            .map(|p| p.verification_runs.clone())
            .unwrap_or_default()
    }

    fn toggle_log(&mut self, run_id: String, index: usize, cx: &mut Context<Self>) {
        let key = (run_id, index);
        if !self.open_logs.remove(&key) {
            self.open_logs.insert(key);
        }
        cx.notify();
    }

    /// The Testing tab's scrolling body: every run, newest first.
    pub(super) fn render_testing(
        &self,
        runs: &[VerificationRun],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = now_millis();
        v_flex()
            .id(SharedString::from(format!(
                "agent-panel-testing-{}",
                self.project_id
            )))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(10.0))
            .pt(px(8.0))
            .pb(px(12.0))
            .gap(px(10.0))
            .children(
                runs_newest_first(runs)
                    .iter()
                    .map(|run| self.render_run(run, now, cx)),
            )
            .into_any_element()
    }

    fn render_run(&self, run: &VerificationRun, now: u64, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let (status, tone) = run_status(run);
        let color = tone.color(cx);
        let passed = run
            .steps
            .iter()
            .filter(|s| s.state == VerificationStepState::Passed)
            .count();

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

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{
        Tone, any_run_in_progress, format_duration, run_status, runs_newest_first, step_row,
        visible_tab,
    };
    use crate::views::agent_session::PanelTab;
    use okena_core::api::{
        VerificationEvidence, VerificationRun, VerificationStep, VerificationStepState as S,
        VerificationVerdict,
    };

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

    fn run(id: &str, created_at: u64, steps: Vec<VerificationStep>) -> VerificationRun {
        VerificationRun {
            id: id.into(),
            terminal_id: "t1".into(),
            task: None,
            worktree: None,
            created_at,
            finished_at: None,
            verdict: None,
            summary: None,
            steps,
        }
    }

    #[test]
    fn runs_list_newest_first() {
        let runs = [
            run("r1", 100, vec![]),
            run("r3", 300, vec![]),
            run("r2", 200, vec![]),
        ];
        let ids: Vec<String> = runs_newest_first(&runs).into_iter().map(|r| r.id).collect();
        assert_eq!(ids, ["r3", "r2", "r1"]);
    }

    #[test]
    fn only_a_started_open_run_counts_as_in_progress() {
        assert!(!any_run_in_progress(&[]));
        let planned = run("p", 0, vec![step("a", S::Pending)]);
        assert!(!any_run_in_progress(std::slice::from_ref(&planned)));

        let running = run("r", 0, vec![step("a", S::Running)]);
        assert!(any_run_in_progress(&[planned.clone(), running.clone()]));

        // Every step done but no verdict yet: still the agent's to close.
        let awaiting = run("w", 0, vec![step("a", S::Passed)]);
        assert!(any_run_in_progress(std::slice::from_ref(&awaiting)));

        let mut finished = awaiting;
        finished.finished_at = Some(10);
        finished.verdict = Some(VerificationVerdict::Passed);
        assert!(!any_run_in_progress(&[planned, finished]));
    }

    #[test]
    fn testing_falls_back_to_info_once_the_runs_are_gone() {
        assert_eq!(visible_tab(PanelTab::Testing, true), PanelTab::Testing);
        assert_eq!(visible_tab(PanelTab::Testing, false), PanelTab::Info);
        assert_eq!(visible_tab(PanelTab::Info, true), PanelTab::Info);
        assert_eq!(visible_tab(PanelTab::Terminal, false), PanelTab::Terminal);
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
        let planned = run("r", 0, vec![step("a", S::Pending)]);
        assert_eq!(run_status(&planned).0, "planned");

        let partway = run("r", 0, vec![step("a", S::Passed), step("b", S::Pending)]);
        assert_eq!(run_status(&partway).0, "1 of 2 done");

        let unclosed = run("r", 0, vec![step("a", S::Passed)]);
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
