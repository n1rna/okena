//! Verification runs, written by agents through okena's MCP server.
//!
//! The run model is `okena_core::api::VerificationRun`; this is how a report
//! changes it. Every rule lives in the pure functions over a session's runs, so
//! the executors only find the session, stamp the time and publish.

use super::ActionResult;
use crate::workspace::state::Workspace;
use okena_core::api::{
    VerificationEvidence, VerificationRun, VerificationStep, VerificationStepPlan,
    VerificationStepState, VerificationVerdict,
};
use okena_workspace::context::WorkspaceCx;

const NO_OPEN_RUN: &str =
    "this session has no open verification run — submit a plan with okena_test_plan first";

/// Where a run was started from: the agent's terminal, and the task and
/// checkout its session was started for.
struct RunOrigin {
    terminal_id: String,
    task: Option<okena_core::tasks::TaskRef>,
    worktree: Option<String>,
}

/// The terminal's open run: its newest, unless that one is finished.
fn open_run<'a>(
    runs: &'a mut [VerificationRun],
    terminal_id: &str,
) -> Option<&'a mut VerificationRun> {
    runs.iter_mut()
        .rev()
        .find(|r| r.terminal_id == terminal_id)
        .filter(|r| !r.is_finished())
}

/// A trimmed string, or `None` when nothing is left.
fn non_blank(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Submit a plan: open a run, or replace the plan of one not yet started.
///
/// Returns the run's id and whether an existing plan was replaced. Refused once
/// a step has started — silently dropping steps that already have results would
/// lose exactly what the run is for.
fn submit_plan(
    runs: &mut Vec<VerificationRun>,
    origin: RunOrigin,
    plan: Vec<VerificationStepPlan>,
    now: u64,
) -> Result<(String, bool), String> {
    let mut steps = Vec::with_capacity(plan.len());
    for (i, step) in plan.into_iter().enumerate() {
        let title = step.title.trim();
        if title.is_empty() {
            return Err(format!("step {} has no title", i + 1));
        }
        steps.push(VerificationStep {
            title: title.to_string(),
            proves: step.proves.trim().to_string(),
            state: VerificationStepState::Pending,
            started_at: None,
            ended_at: None,
            reason: None,
            evidence: VerificationEvidence::default(),
        });
    }
    if steps.is_empty() {
        return Err("a plan needs at least one step".into());
    }

    if let Some(run) = open_run(runs, &origin.terminal_id) {
        if run.has_started() {
            let done = run.steps.iter().filter(|s| s.state.is_finished()).count();
            return Err(format!(
                "this run has already started ({done} of {} steps have a result), so its plan \
                 can no longer be replaced. Finish it with okena_test_run_finish, then submit the \
                 new plan to begin another run.",
                run.steps.len()
            ));
        }
        run.steps = steps;
        run.task = origin.task;
        run.worktree = origin.worktree;
        return Ok((run.id.clone(), true));
    }

    let id = uuid::Uuid::new_v4().to_string();
    runs.push(VerificationRun {
        id: id.clone(),
        terminal_id: origin.terminal_id,
        task: origin.task,
        worktree: origin.worktree,
        created_at: now,
        finished_at: None,
        verdict: None,
        summary: None,
        steps,
    });
    Ok((id, false))
}

/// Step `number` of `run`, counting from 1 as the plan is numbered.
fn step_mut(run: &mut VerificationRun, number: usize) -> Result<&mut VerificationStep, String> {
    let count = run.steps.len();
    number
        .checked_sub(1)
        .and_then(|i| run.steps.get_mut(i))
        .ok_or_else(|| format!("there is no step {number} — the plan has {count}, numbered from 1"))
}

/// Mark a step running. Starting one that already has a result runs it again,
/// so an agent re-verifying after a fix does not have to plan a new run.
fn start_step(
    runs: &mut [VerificationRun],
    terminal_id: &str,
    number: usize,
    now: u64,
) -> Result<String, String> {
    let run = open_run(runs, terminal_id).ok_or(NO_OPEN_RUN)?;
    let id = run.id.clone();
    let step = step_mut(run, number)?;
    step.state = VerificationStepState::Running;
    step.started_at = Some(now);
    step.ended_at = None;
    step.reason = None;
    step.evidence = VerificationEvidence::default();
    Ok(id)
}

/// Record a step's outcome, reason and evidence.
fn record_result(
    runs: &mut [VerificationRun],
    terminal_id: &str,
    number: usize,
    outcome: VerificationStepState,
    reason: &str,
    evidence: VerificationEvidence,
    now: u64,
) -> Result<String, String> {
    if !outcome.is_finished() {
        return Err("`outcome` must be passed, failed or skipped".into());
    }
    // One line, as the canvas shows it: a reason spread over several lines is
    // joined rather than cut, since the part cut could be the point.
    let reason = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    if reason.is_empty() {
        return Err("give a one-line `reason` — for a failure, what went wrong".into());
    }
    let run = open_run(runs, terminal_id).ok_or(NO_OPEN_RUN)?;
    let id = run.id.clone();
    let step = step_mut(run, number)?;
    step.state = outcome;
    // A step reported without being started still ran; it just was not watched.
    step.started_at.get_or_insert(now);
    step.ended_at = Some(now);
    step.reason = Some(reason);
    step.evidence = VerificationEvidence {
        log_tail: evidence
            .log_tail
            .map(|l| l.trim_end().to_string())
            .filter(|l| !l.trim().is_empty()),
        url: non_blank(evidence.url),
        screenshot_path: non_blank(evidence.screenshot_path),
    };
    Ok(id)
}

/// Close the open run. Steps it never reached are marked skipped, so a run
/// that stopped early does not leave a step "running" forever.
fn finish_run(
    runs: &mut [VerificationRun],
    terminal_id: &str,
    verdict: VerificationVerdict,
    summary: Option<String>,
    now: u64,
) -> Result<(String, usize), String> {
    let run = open_run(runs, terminal_id).ok_or(NO_OPEN_RUN)?;
    let mut skipped = 0;
    for step in run.steps.iter_mut().filter(|s| !s.state.is_finished()) {
        step.state = VerificationStepState::Skipped;
        step.ended_at = Some(now);
        step.reason = Some("Not reached before the run finished".into());
        skipped += 1;
    }
    run.finished_at = Some(now);
    run.verdict = Some(verdict);
    run.summary = non_blank(summary);
    Ok((run.id.clone(), skipped))
}

// ─── Executors ───────────────────────────────────────────────────────────────

/// Apply `change` to the runs of the session `project_id`, whose agent runs in
/// `terminal_id`, and publish the result.
fn with_session_runs(
    ws: &mut Workspace,
    project_id: &str,
    terminal_id: &str,
    cx: &mut impl WorkspaceCx,
    change: impl FnOnce(&mut Vec<VerificationRun>, RunOrigin) -> Result<serde_json::Value, String>,
) -> ActionResult {
    let Some(project) = ws.data.projects.iter_mut().find(|p| p.id == project_id) else {
        return ActionResult::Err(format!("project not found: {project_id}"));
    };
    // The MCP server finds the session by its terminal, so the two agree; a
    // request naming a terminal of another project is not this session's.
    let owns_terminal = project
        .layout
        .as_ref()
        .is_some_and(|l| l.collect_terminal_ids().iter().any(|t| t == terminal_id))
        || project.terminal_names.contains_key(terminal_id);
    if !owns_terminal {
        return ActionResult::Err(format!(
            "terminal {terminal_id} is not part of project {project_id}"
        ));
    }
    let origin = RunOrigin {
        terminal_id: terminal_id.to_string(),
        task: project.task_ref.clone(),
        worktree: Some(
            project
                .worktree_info
                .as_ref()
                .map(|w| w.worktree_path.clone())
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| project.path.clone()),
        ),
    };
    match change(&mut project.verification_runs, origin) {
        Ok(reply) => {
            ws.notify_data(cx);
            ActionResult::Ok(Some(reply))
        }
        Err(error) => ActionResult::Err(error),
    }
}

pub(super) fn test_plan(
    ws: &mut Workspace,
    project_id: String,
    terminal_id: String,
    steps: Vec<VerificationStepPlan>,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let now = super::tasks::now_millis();
    with_session_runs(ws, &project_id, &terminal_id, cx, |runs, origin| {
        let count = steps.len();
        let (run_id, replaced) = submit_plan(runs, origin, steps, now)?;
        Ok(serde_json::json!({ "run_id": run_id, "steps": count, "replaced": replaced }))
    })
}

pub(super) fn test_step_start(
    ws: &mut Workspace,
    project_id: String,
    terminal_id: String,
    step: usize,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let now = super::tasks::now_millis();
    with_session_runs(ws, &project_id, &terminal_id, cx, |runs, origin| {
        let run_id = start_step(runs, &origin.terminal_id, step, now)?;
        Ok(serde_json::json!({ "run_id": run_id, "step": step, "state": "running" }))
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn test_step_result(
    ws: &mut Workspace,
    project_id: String,
    terminal_id: String,
    step: usize,
    outcome: VerificationStepState,
    reason: String,
    evidence: VerificationEvidence,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let now = super::tasks::now_millis();
    with_session_runs(ws, &project_id, &terminal_id, cx, |runs, origin| {
        let run_id = record_result(
            runs,
            &origin.terminal_id,
            step,
            outcome,
            &reason,
            evidence,
            now,
        )?;
        Ok(serde_json::json!({ "run_id": run_id, "step": step, "state": outcome.label() }))
    })
}

pub(super) fn test_run_finish(
    ws: &mut Workspace,
    project_id: String,
    terminal_id: String,
    verdict: VerificationVerdict,
    summary: Option<String>,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let now = super::tasks::now_millis();
    with_session_runs(ws, &project_id, &terminal_id, cx, |runs, origin| {
        let (run_id, skipped) = finish_run(runs, &origin.terminal_id, verdict, summary, now)?;
        Ok(serde_json::json!({
            "run_id": run_id,
            "verdict": verdict.label(),
            "skipped_steps": skipped,
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(terminal: &str) -> RunOrigin {
        RunOrigin {
            terminal_id: terminal.into(),
            task: None,
            worktree: Some("/p/wt".into()),
        }
    }

    fn plan(titles: &[&str]) -> Vec<VerificationStepPlan> {
        titles
            .iter()
            .map(|t| VerificationStepPlan {
                title: (*t).into(),
                proves: format!("{t} works"),
            })
            .collect()
    }

    fn passed(runs: &mut [VerificationRun], step: usize) {
        record_result(
            runs,
            "t1",
            step,
            VerificationStepState::Passed,
            "ok",
            VerificationEvidence::default(),
            20,
        )
        .expect("result");
    }

    #[test]
    fn a_plan_opens_a_run_of_pending_steps() {
        let mut runs = Vec::new();
        let (id, replaced) =
            submit_plan(&mut runs, origin("t1"), plan(&["build", "test"]), 10).expect("plan");
        assert!(!replaced);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, id);
        assert_eq!(runs[0].created_at, 10);
        assert_eq!(runs[0].worktree.as_deref(), Some("/p/wt"));
        assert!(
            runs[0]
                .steps
                .iter()
                .all(|s| s.state == VerificationStepState::Pending)
        );
        assert_eq!(runs[0].steps[1].proves, "test works");
    }

    #[test]
    fn a_plan_is_replaced_until_its_first_step_starts() {
        let mut runs = Vec::new();
        let (first, _) = submit_plan(&mut runs, origin("t1"), plan(&["a"]), 10).expect("plan");
        let (second, replaced) =
            submit_plan(&mut runs, origin("t1"), plan(&["x", "y"]), 11).expect("replan");
        assert!(replaced);
        assert_eq!(first, second, "the same run, with a new plan");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].steps.len(), 2);
    }

    #[test]
    fn a_plan_after_the_run_started_is_rejected_and_completed_steps_are_kept() {
        let mut runs = Vec::new();
        submit_plan(&mut runs, origin("t1"), plan(&["a", "b"]), 10).expect("plan");
        start_step(&mut runs, "t1", 1, 11).expect("start");
        passed(&mut runs, 1);

        let error =
            submit_plan(&mut runs, origin("t1"), plan(&["other"]), 30).expect_err("rejected");
        assert!(error.contains("already started"), "{error}");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].steps.len(), 2);
        assert_eq!(runs[0].steps[0].state, VerificationStepState::Passed);
        assert_eq!(runs[0].steps[0].reason.as_deref(), Some("ok"));
    }

    #[test]
    fn a_plan_after_a_finished_run_opens_a_new_one() {
        let mut runs = Vec::new();
        submit_plan(&mut runs, origin("t1"), plan(&["a"]), 10).expect("plan");
        passed(&mut runs, 1);
        finish_run(&mut runs, "t1", VerificationVerdict::Passed, None, 21).expect("finish");

        let (_, replaced) = submit_plan(&mut runs, origin("t1"), plan(&["b"]), 30).expect("plan");
        assert!(!replaced);
        assert_eq!(runs.len(), 2);
        assert!(runs[0].is_finished());
        assert!(!runs[1].is_finished());
    }

    #[test]
    fn each_agent_terminal_keeps_its_own_run() {
        let mut runs = Vec::new();
        submit_plan(&mut runs, origin("t1"), plan(&["a"]), 10).expect("plan");
        start_step(&mut runs, "t1", 1, 11).expect("start");
        // Another agent in the same session planning is not a replan of t1's.
        let (_, replaced) = submit_plan(&mut runs, origin("t2"), plan(&["b"]), 12).expect("plan");
        assert!(!replaced);
        assert_eq!(runs.len(), 2);
        assert!(start_step(&mut runs, "t3", 1, 13).is_err());
    }

    #[test]
    fn empty_plans_and_untitled_steps_are_refused() {
        let mut runs = Vec::new();
        assert!(submit_plan(&mut runs, origin("t1"), Vec::new(), 10).is_err());
        let error =
            submit_plan(&mut runs, origin("t1"), plan(&["a", "  "]), 10).expect_err("untitled");
        assert!(error.contains("step 2"), "{error}");
        assert!(runs.is_empty());
    }

    #[test]
    fn steps_are_numbered_from_one() {
        let mut runs = Vec::new();
        submit_plan(&mut runs, origin("t1"), plan(&["a", "b"]), 10).expect("plan");
        assert!(start_step(&mut runs, "t1", 0, 11).is_err());
        let error = start_step(&mut runs, "t1", 3, 11).expect_err("out of range");
        assert!(error.contains("has 2"), "{error}");
        start_step(&mut runs, "t1", 2, 11).expect("last step");
        assert_eq!(runs[0].steps[1].state, VerificationStepState::Running);
        assert_eq!(runs[0].steps[1].started_at, Some(11));
    }

    #[test]
    fn a_result_keeps_its_reason_on_one_line_and_its_evidence() {
        let mut runs = Vec::new();
        submit_plan(&mut runs, origin("t1"), plan(&["login"]), 10).expect("plan");
        start_step(&mut runs, "t1", 1, 11).expect("start");
        record_result(
            &mut runs,
            "t1",
            1,
            VerificationStepState::Failed,
            "  redirect loops\n  back to /login ",
            VerificationEvidence {
                log_tail: Some("GET /login 302\nGET /login 302\n".into()),
                url: Some(" http://localhost:3000/login ".into()),
                screenshot_path: Some("   ".into()),
            },
            12,
        )
        .expect("result");

        let step = &runs[0].steps[0];
        assert_eq!(step.state, VerificationStepState::Failed);
        assert_eq!(
            step.reason.as_deref(),
            Some("redirect loops back to /login")
        );
        assert_eq!((step.started_at, step.ended_at), (Some(11), Some(12)));
        assert_eq!(
            step.evidence.log_tail.as_deref(),
            Some("GET /login 302\nGET /login 302")
        );
        assert_eq!(
            step.evidence.url.as_deref(),
            Some("http://localhost:3000/login")
        );
        assert_eq!(step.evidence.screenshot_path, None);
    }

    #[test]
    fn a_result_needs_an_outcome_and_a_reason() {
        let mut runs = Vec::new();
        submit_plan(&mut runs, origin("t1"), plan(&["a"]), 10).expect("plan");
        let none = VerificationEvidence::default;
        assert!(
            record_result(
                &mut runs,
                "t1",
                1,
                VerificationStepState::Running,
                "x",
                none(),
                11
            )
            .is_err()
        );
        assert!(
            record_result(
                &mut runs,
                "t1",
                1,
                VerificationStepState::Passed,
                " \n ",
                none(),
                11
            )
            .is_err()
        );
        assert_eq!(runs[0].steps[0].state, VerificationStepState::Pending);
    }

    #[test]
    fn restarting_a_finished_step_clears_its_old_result() {
        let mut runs = Vec::new();
        submit_plan(&mut runs, origin("t1"), plan(&["a"]), 10).expect("plan");
        record_result(
            &mut runs,
            "t1",
            1,
            VerificationStepState::Failed,
            "broken",
            VerificationEvidence {
                url: Some("http://x".into()),
                ..Default::default()
            },
            11,
        )
        .expect("result");
        start_step(&mut runs, "t1", 1, 20).expect("restart");
        let step = &runs[0].steps[0];
        assert_eq!(step.state, VerificationStepState::Running);
        assert_eq!((step.started_at, step.ended_at), (Some(20), None));
        assert!(step.reason.is_none() && step.evidence.is_empty());
    }

    #[test]
    fn finishing_skips_the_unreached_steps_and_closes_the_run() {
        let mut runs = Vec::new();
        submit_plan(&mut runs, origin("t1"), plan(&["a", "b", "c"]), 10).expect("plan");
        passed(&mut runs, 1);
        start_step(&mut runs, "t1", 2, 21).expect("start");

        let (_, skipped) = finish_run(
            &mut runs,
            "t1",
            VerificationVerdict::Inconclusive,
            Some(" db would not start ".into()),
            30,
        )
        .expect("finish");
        assert_eq!(skipped, 2);
        let run = &runs[0];
        assert_eq!(run.verdict, Some(VerificationVerdict::Inconclusive));
        assert_eq!(run.summary.as_deref(), Some("db would not start"));
        assert_eq!(run.finished_at, Some(30));
        assert_eq!(run.steps[0].state, VerificationStepState::Passed);
        assert_eq!(run.steps[1].state, VerificationStepState::Skipped);
        assert_eq!(run.steps[2].state, VerificationStepState::Skipped);

        // Closed: nothing more is written into it.
        assert!(start_step(&mut runs, "t1", 1, 31).is_err());
        assert!(finish_run(&mut runs, "t1", VerificationVerdict::Passed, None, 32).is_err());
    }
}

#[cfg(test)]
mod round_trip_tests {
    use crate::workspace::state::Workspace;
    use okena_core::api::{ActionRequest, ApiProject, VerificationStepState, VerificationVerdict};
    use okena_workspace::context::WorkspaceCx;
    use serde_json::json;

    use super::super::ActionResult;

    struct TestCx;

    impl WorkspaceCx for TestCx {
        fn notify(&mut self) {}
        fn refresh_views(&mut self) {}
        fn hook_runner(&self) -> Option<okena_hooks::HookRunner> {
            None
        }
        fn hook_monitor(&self) -> Option<okena_hooks::HookMonitor> {
            None
        }
    }

    fn workspace() -> Workspace {
        let mut ws = Workspace::new(crate::workspace::state::WorkspaceData::empty());
        let session = serde_json::from_value(json!({
            "id": "s1", "name": "QBL-7", "path": "/tmp/session", "layout": null,
            "terminal_names": { "t1": "claude" },
        }))
        .expect("a minimal session");
        ws.data.projects.push(session);
        ws
    }

    /// Run a body shaped as okena's MCP server posts it.
    fn post(ws: &mut Workspace, body: serde_json::Value) -> Result<serde_json::Value, String> {
        let action: ActionRequest = serde_json::from_value(body).expect("a known action");
        let cx = &mut TestCx;
        let result = match action {
            ActionRequest::AgentTestPlan {
                project_id,
                terminal_id,
                steps,
            } => super::test_plan(ws, project_id, terminal_id, steps, cx),
            ActionRequest::AgentTestStepStart {
                project_id,
                terminal_id,
                step,
            } => super::test_step_start(ws, project_id, terminal_id, step, cx),
            ActionRequest::AgentTestStepResult {
                project_id,
                terminal_id,
                step,
                outcome,
                reason,
                evidence,
            } => super::test_step_result(
                ws,
                project_id,
                terminal_id,
                step,
                outcome,
                reason,
                evidence,
                cx,
            ),
            ActionRequest::AgentTestRunFinish {
                project_id,
                terminal_id,
                verdict,
                summary,
            } => super::test_run_finish(ws, project_id, terminal_id, verdict, summary, cx),
            other => panic!("not a test-run action: {other:?}"),
        };
        match result {
            ActionResult::Ok(reply) => Ok(reply.unwrap_or_default()),
            ActionResult::Err(error) => Err(error),
        }
    }

    fn body(action: &str, fields: serde_json::Value) -> serde_json::Value {
        let mut body = fields;
        body["action"] = json!(action);
        body["project_id"] = json!("s1");
        body["terminal_id"] = json!("t1");
        body
    }

    #[test]
    fn a_plan_and_its_results_reach_the_wire_with_their_evidence() {
        let mut ws = workspace();
        post(
            &mut ws,
            body(
                "agent_test_plan",
                json!({ "steps": [
                    { "title": "Build", "proves": "it compiles" },
                    { "title": "Log in", "proves": "sessions survive a reload" },
                ]}),
            ),
        )
        .expect("plan");
        post(&mut ws, body("agent_test_step_start", json!({ "step": 1 }))).expect("start 1");
        post(
            &mut ws,
            body(
                "agent_test_step_result",
                json!({ "step": 1, "outcome": "passed", "reason": "clean build",
                        "evidence": { "log_tail": "Finished `dev` profile" } }),
            ),
        )
        .expect("result 1");
        post(&mut ws, body("agent_test_step_start", json!({ "step": 2 }))).expect("start 2");
        post(
            &mut ws,
            body(
                "agent_test_step_result",
                json!({ "step": 2, "outcome": "failed", "reason": "redirects back to /login",
                        "evidence": { "url": "http://localhost:3000/login",
                                      "screenshot_path": "/tmp/login.png" } }),
            ),
        )
        .expect("result 2");

        // A second plan now would drop the steps already run.
        let refused = post(
            &mut ws,
            body("agent_test_plan", json!({ "steps": [{ "title": "Redo" }] })),
        );
        assert!(refused.is_err_and(|e| e.contains("already started")));

        post(
            &mut ws,
            body("agent_test_run_finish", json!({ "verdict": "failed" })),
        )
        .expect("finish");

        // Across the wire as clients receive it.
        let api = crate::remote_snapshot::build_api_project(
            &ws.data.projects[0],
            &Default::default(),
            &Default::default(),
            &Default::default(),
            &Default::default(),
            &Default::default(),
        );
        let wire = serde_json::to_string(&api).expect("encode");
        let received: ApiProject = serde_json::from_str(&wire).expect("decode");

        assert_eq!(received.verification_runs.len(), 1);
        let run = &received.verification_runs[0];
        assert_eq!(run.terminal_id, "t1");
        assert_eq!(run.worktree.as_deref(), Some("/tmp/session"));
        assert_eq!(run.verdict, Some(VerificationVerdict::Failed));
        assert_eq!(run.steps.len(), 2, "the completed steps were kept");
        assert_eq!(run.steps[0].state, VerificationStepState::Passed);
        assert_eq!(
            run.steps[0].evidence.log_tail.as_deref(),
            Some("Finished `dev` profile")
        );
        assert_eq!(run.steps[1].state, VerificationStepState::Failed);
        assert_eq!(
            run.steps[1].reason.as_deref(),
            Some("redirects back to /login")
        );
        assert_eq!(
            run.steps[1].evidence.url.as_deref(),
            Some("http://localhost:3000/login")
        );
        assert_eq!(
            run.steps[1].evidence.screenshot_path.as_deref(),
            Some("/tmp/login.png")
        );
    }

    #[test]
    fn a_report_for_a_terminal_outside_the_session_is_refused() {
        let mut ws = workspace();
        let mut foreign = body("agent_test_plan", json!({ "steps": [{ "title": "x" }] }));
        foreign["terminal_id"] = json!("someone-else");
        assert!(post(&mut ws, foreign).is_err());
        assert!(ws.data.projects[0].verification_runs.is_empty());
    }

    #[test]
    fn runs_are_not_written_to_workspace_json() {
        let mut ws = workspace();
        post(
            &mut ws,
            body("agent_test_plan", json!({ "steps": [{ "title": "x" }] })),
        )
        .expect("plan");
        let saved = serde_json::to_value(&ws.data.projects[0]).expect("persist");
        assert!(saved.get("verification_runs").is_none(), "{saved}");
    }
}
