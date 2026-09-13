//! What an agent is doing, decided from the agent's own signals.
//!
//! The daemon keeps the signals for every agent terminal and resolves them
//! here; clients only read the result. Kept pure, with every time passed in as
//! Unix millis, so each precedence rule is a unit test rather than a guess at
//! what a render function did.
//!
//! The signals, strongest first:
//!
//! * **Native events** — lifecycle hooks the agent itself fires (Claude Code's
//!   `UserPromptSubmit` / `PreToolUse` / `Notification` / `Stop`, Codex's
//!   `notify`), delivered through `okena agent-event`.
//! * **Attention** — a bell or an OSC 9/777/99 notification from the terminal.
//! * **Output** — the terminal producing output, and going quiet.
//! * **The agent's report** — `okena_report_status`. It never decides that an
//!   agent is working or stopped; it only says *why* a stopped agent stopped.
//!
//! Input reaching the terminal outdates everything the agent said before it: a
//! question you have just answered is no longer a question.

use serde::{Deserialize, Serialize};

use crate::harness::AgentState;

/// How long an agent without native events has to stay silent before it counts
/// as waiting. Agents redraw a spinner or a timer while they work, so a few
/// seconds of nothing is a prompt, not a slow step.
pub const QUIET_PERIOD_MS: u64 = 3_000;

/// What an agent is doing, as every view shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentActivity {
    /// Busy; nothing needed from you.
    Working,
    /// Finished its turn without saying why.
    Waiting,
    /// Stopped to ask you something: a permission prompt, a question.
    NeedsInput,
    /// Finished a piece of work and waiting for you to look.
    ReadyForReview,
    /// Cannot continue.
    Blocked,
    /// Said it is done and is idle.
    Done,
    /// Nothing is running.
    Stopped,
    /// A state a newer daemon sent that this build doesn't model. Treated as
    /// wanting attention, like an unknown reported state.
    #[serde(other)]
    Unknown,
}

impl AgentActivity {
    /// Whether you should look at it.
    pub const fn wants_attention(self) -> bool {
        matches!(
            self,
            AgentActivity::NeedsInput
                | AgentActivity::ReadyForReview
                | AgentActivity::Blocked
                | AgentActivity::Unknown
        )
    }

    /// Whether the agent is sitting at its prompt, for whatever reason. What a
    /// pane's idle border stands for.
    pub const fn is_idle(self) -> bool {
        !matches!(self, AgentActivity::Working | AgentActivity::Stopped)
    }

    pub const fn label(self) -> &'static str {
        match self {
            AgentActivity::Working => "working",
            AgentActivity::Waiting => "waiting",
            AgentActivity::NeedsInput => "needs input",
            AgentActivity::ReadyForReview => "ready for review",
            AgentActivity::Blocked => "blocked",
            AgentActivity::Done => "done",
            AgentActivity::Stopped => "stopped",
            AgentActivity::Unknown => "needs attention",
        }
    }
}

/// A lifecycle event an agent fires about itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHookEvent {
    /// A prompt was submitted and a turn began.
    TurnStarted,
    /// A tool is about to run or has just run.
    ToolActivity,
    /// The agent is blocked on you: a permission or approval prompt.
    NeedsInput,
    /// The turn ended and the agent is back at its prompt.
    TurnEnded,
}

/// Everything okena has seen from one agent terminal. Times are Unix millis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AgentSignals {
    /// The latest native event and when it arrived.
    pub native: Option<(AgentHookEvent, u64)>,
    /// The latest bell or OSC notification.
    pub attention_at: Option<u64>,
    /// The latest input that reached the terminal.
    pub input_at: Option<u64>,
    /// The latest output from the terminal.
    pub output_at: Option<u64>,
}

/// What the agent last reported, and when okena received it.
///
/// `reported_at` is `None` for a report stored before okena stamped them; such
/// a report counts until the first input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    pub state: AgentState,
    pub reported_at: Option<u64>,
}

/// Whether something that happened at `at` still stands, given the latest
/// input. An event in the same millisecond as the input stands: a prompt
/// submission's hook can land in the very tick its Enter did.
fn stands(at: u64, input_at: Option<u64>) -> bool {
    input_at.is_none_or(|input| at >= input)
}

/// Whether input has reached the terminal since the agent's report, so the
/// report — and its question and suggestions — no longer apply.
pub fn report_is_stale(reported_at: Option<u64>, input_at: Option<u64>) -> bool {
    input_at.is_some() && !stands(reported_at.unwrap_or(0), input_at)
}

/// Resolve an agent terminal's activity.
///
/// Precedence:
/// 1. nothing running → [`Stopped`](AgentActivity::Stopped);
/// 2. a "needs you" signal — a native permission prompt, or a bell or OSC
///    notification newer than any native working event → needs input;
/// 3. a native turn start or tool run → working;
/// 4. a native turn end → the report's reason, else waiting;
/// 5. no native event → output within [`QUIET_PERIOD_MS`] is working, silence
///    is settled the same way as a turn end.
///
/// Input outdates every native event, notification and report before it. A
/// bell after a turn has ended is ignored: agents ring one to remind you they
/// are idle, which is not a new reason to stop.
pub fn resolve(
    running: bool,
    signals: &AgentSignals,
    report: Option<Report>,
    now: u64,
) -> AgentActivity {
    if !running {
        return AgentActivity::Stopped;
    }
    let input_at = signals.input_at;
    let native = signals.native.filter(|(_, at)| stands(*at, input_at));

    let attention = signals
        .attention_at
        .filter(|at| stands(*at, input_at))
        .is_some_and(|at| match native {
            None => true,
            Some((AgentHookEvent::TurnEnded, _)) => false,
            Some((_, native_at)) => at > native_at,
        });
    if attention {
        return AgentActivity::NeedsInput;
    }

    match native.map(|(event, _)| event) {
        Some(AgentHookEvent::NeedsInput) => AgentActivity::NeedsInput,
        Some(AgentHookEvent::TurnStarted | AgentHookEvent::ToolActivity) => AgentActivity::Working,
        Some(AgentHookEvent::TurnEnded) => settled(report, input_at),
        None if signals
            .output_at
            .is_some_and(|out| now.saturating_sub(out) < QUIET_PERIOD_MS) =>
        {
            AgentActivity::Working
        }
        None => settled(report, input_at),
    }
}

/// An agent at rest: the reason it reported, if the report still stands.
fn settled(report: Option<Report>, input_at: Option<u64>) -> AgentActivity {
    let reported = report
        .filter(|r| !report_is_stale(r.reported_at, input_at))
        .map(|r| r.state);
    match reported {
        Some(AgentState::NeedsInput | AgentState::Unknown) => AgentActivity::NeedsInput,
        Some(AgentState::ReadyForReview) => AgentActivity::ReadyForReview,
        Some(AgentState::Blocked) => AgentActivity::Blocked,
        Some(AgentState::Done) => AgentActivity::Done,
        // "Working" reported by an agent that has stopped says nothing about
        // why it stopped.
        Some(AgentState::Working) | None => AgentActivity::Waiting,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgentActivity, AgentHookEvent, AgentSignals, QUIET_PERIOD_MS, Report, report_is_stale,
        resolve,
    };
    use crate::harness::AgentState;

    const NOW: u64 = 1_000_000;

    fn report(state: AgentState, at: u64) -> Option<Report> {
        Some(Report {
            state,
            reported_at: Some(at),
        })
    }

    fn native(event: AgentHookEvent, at: u64) -> AgentSignals {
        AgentSignals {
            native: Some((event, at)),
            ..Default::default()
        }
    }

    #[test]
    fn an_exited_agent_is_stopped_whatever_it_said() {
        let s = native(AgentHookEvent::NeedsInput, NOW);
        assert_eq!(
            resolve(false, &s, report(AgentState::Blocked, NOW), NOW),
            AgentActivity::Stopped
        );
    }

    #[test]
    fn a_native_permission_prompt_needs_input_without_any_report() {
        let s = native(AgentHookEvent::NeedsInput, NOW - 10);
        assert_eq!(resolve(true, &s, None, NOW), AgentActivity::NeedsInput);
    }

    #[test]
    fn a_native_prompt_outranks_a_report() {
        let s = native(AgentHookEvent::NeedsInput, NOW - 10);
        assert_eq!(
            resolve(true, &s, report(AgentState::ReadyForReview, NOW - 20), NOW),
            AgentActivity::NeedsInput
        );
    }

    #[test]
    fn a_bell_while_working_needs_input() {
        let mut s = native(AgentHookEvent::ToolActivity, NOW - 100);
        s.attention_at = Some(NOW - 50);
        assert_eq!(resolve(true, &s, None, NOW), AgentActivity::NeedsInput);
    }

    #[test]
    fn a_tool_run_after_a_bell_is_working_again() {
        let mut s = native(AgentHookEvent::ToolActivity, NOW - 50);
        s.attention_at = Some(NOW - 100);
        assert_eq!(resolve(true, &s, None, NOW), AgentActivity::Working);
    }

    #[test]
    fn an_idle_reminder_after_a_turn_ended_does_not_override_the_report() {
        let mut s = native(AgentHookEvent::TurnEnded, NOW - 100);
        s.attention_at = Some(NOW - 10);
        assert_eq!(
            resolve(true, &s, report(AgentState::ReadyForReview, NOW - 200), NOW),
            AgentActivity::ReadyForReview
        );
    }

    #[test]
    fn native_turn_start_and_tool_runs_are_working() {
        for event in [AgentHookEvent::TurnStarted, AgentHookEvent::ToolActivity] {
            let s = native(event, NOW - 60_000);
            assert_eq!(resolve(true, &s, None, NOW), AgentActivity::Working);
        }
    }

    #[test]
    fn working_ignores_a_stale_reason_from_the_last_turn() {
        let s = native(AgentHookEvent::TurnStarted, NOW - 10);
        assert_eq!(
            resolve(true, &s, report(AgentState::NeedsInput, NOW - 20), NOW),
            AgentActivity::Working
        );
    }

    #[test]
    fn a_turn_that_ended_without_a_report_is_waiting() {
        let s = native(AgentHookEvent::TurnEnded, NOW - 10);
        assert_eq!(resolve(true, &s, None, NOW), AgentActivity::Waiting);
    }

    #[test]
    fn a_turn_end_takes_the_reports_reason() {
        let s = native(AgentHookEvent::TurnEnded, NOW - 10);
        for (state, expected) in [
            (AgentState::NeedsInput, AgentActivity::NeedsInput),
            (AgentState::ReadyForReview, AgentActivity::ReadyForReview),
            (AgentState::Blocked, AgentActivity::Blocked),
            (AgentState::Done, AgentActivity::Done),
            (AgentState::Unknown, AgentActivity::NeedsInput),
            (AgentState::Working, AgentActivity::Waiting),
        ] {
            assert_eq!(
                resolve(true, &s, report(state, NOW - 20), NOW),
                expected,
                "{state:?}"
            );
        }
    }

    #[test]
    fn input_outdates_the_report() {
        let mut s = native(AgentHookEvent::TurnEnded, NOW - 300);
        s.input_at = Some(NOW - 100);
        // The native turn end predates the input too, so the fallback applies:
        // no output since, so it is settled — without the old reason.
        assert_eq!(
            resolve(true, &s, report(AgentState::NeedsInput, NOW - 200), NOW),
            AgentActivity::Waiting
        );
        assert!(report_is_stale(Some(NOW - 200), Some(NOW - 100)));
    }

    #[test]
    fn typing_a_reply_makes_the_agent_working_once_it_responds() {
        // Waiting on a permission prompt; you answer, and it produces output.
        let s = AgentSignals {
            native: Some((AgentHookEvent::NeedsInput, NOW - 5_000)),
            input_at: Some(NOW - 1_000),
            output_at: Some(NOW - 500),
            ..Default::default()
        };
        assert_eq!(resolve(true, &s, None, NOW), AgentActivity::Working);
    }

    #[test]
    fn a_report_after_the_input_counts_again() {
        let s = AgentSignals {
            native: Some((AgentHookEvent::TurnEnded, NOW - 10)),
            input_at: Some(NOW - 1_000),
            ..Default::default()
        };
        assert_eq!(
            resolve(true, &s, report(AgentState::ReadyForReview, NOW - 20), NOW),
            AgentActivity::ReadyForReview
        );
        assert!(!report_is_stale(Some(NOW - 20), Some(NOW - 1_000)));
    }

    #[test]
    fn a_hook_in_the_same_tick_as_its_enter_still_stands() {
        let s = AgentSignals {
            native: Some((AgentHookEvent::TurnStarted, NOW - 100)),
            input_at: Some(NOW - 100),
            ..Default::default()
        };
        assert_eq!(resolve(true, &s, None, NOW), AgentActivity::Working);
    }

    #[test]
    fn a_report_from_before_stamping_counts_until_input() {
        let legacy = Some(Report {
            state: AgentState::Blocked,
            reported_at: None,
        });
        let quiet = AgentSignals::default();
        assert_eq!(resolve(true, &quiet, legacy, NOW), AgentActivity::Blocked);
        let answered = AgentSignals {
            input_at: Some(NOW - 60_000),
            ..Default::default()
        };
        assert_eq!(
            resolve(true, &answered, legacy, NOW),
            AgentActivity::Waiting
        );
    }

    #[test]
    fn without_native_events_output_is_working() {
        let s = AgentSignals {
            output_at: Some(NOW - QUIET_PERIOD_MS + 1),
            ..Default::default()
        };
        assert_eq!(resolve(true, &s, None, NOW), AgentActivity::Working);
    }

    #[test]
    fn without_native_events_silence_is_waiting_or_the_report() {
        let s = AgentSignals {
            output_at: Some(NOW - QUIET_PERIOD_MS),
            ..Default::default()
        };
        assert_eq!(resolve(true, &s, None, NOW), AgentActivity::Waiting);
        assert_eq!(
            resolve(true, &s, report(AgentState::ReadyForReview, NOW - 1), NOW),
            AgentActivity::ReadyForReview
        );
    }

    #[test]
    fn without_native_events_a_bell_needs_input_until_answered() {
        let rang = AgentSignals {
            attention_at: Some(NOW - 10_000),
            output_at: Some(NOW - 10),
            ..Default::default()
        };
        assert_eq!(resolve(true, &rang, None, NOW), AgentActivity::NeedsInput);
        let answered = AgentSignals {
            input_at: Some(NOW - 5_000),
            ..rang
        };
        assert_eq!(resolve(true, &answered, None, NOW), AgentActivity::Working);
    }

    #[test]
    fn only_stopping_states_want_attention() {
        assert!(!AgentActivity::Working.wants_attention());
        assert!(!AgentActivity::Waiting.wants_attention());
        assert!(!AgentActivity::Done.wants_attention());
        assert!(!AgentActivity::Stopped.wants_attention());
        for a in [
            AgentActivity::NeedsInput,
            AgentActivity::ReadyForReview,
            AgentActivity::Blocked,
            AgentActivity::Unknown,
        ] {
            assert!(a.wants_attention(), "{a:?}");
        }
    }

    #[test]
    fn an_activity_from_a_newer_daemon_decodes_as_unknown() {
        let a: AgentActivity = serde_json::from_str("\"compacting\"").expect("decode");
        assert_eq!(a, AgentActivity::Unknown);
        assert!(a.wants_attention());
    }

    #[test]
    fn hook_events_use_snake_case_on_the_wire() {
        assert_eq!(
            serde_json::to_string(&AgentHookEvent::TurnEnded).expect("encode"),
            "\"turn_ended\""
        );
    }
}
