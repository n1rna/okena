//! What an agent's sidebar card says about it.
//!
//! Pure, and GPUI-free like `activity_order`, so the one part with rules — how
//! the terminal's state and the agent's own report combine — is tested rather
//! than read off a render function.
//!
//! The rule is the agent panel's: an agent's reason for stopping only counts
//! while its prompt is actually waiting. An agent that said "ready for review"
//! and was then told to carry on is working again the moment output resumes,
//! and flagging it until it next reports would cry wolf on exactly the
//! sessions you have just answered.

use okena_core::harness::AgentState;

/// How an agent is doing, as its card shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardState {
    /// Stopped to ask you something.
    NeedsInput,
    /// Finished a piece of work and waiting for you.
    ReadyForReview,
    /// Cannot continue.
    Blocked,
    /// Idle at its prompt without saying why.
    Waiting,
    /// Producing output.
    Working,
    /// Said it is done and is idle.
    Done,
    /// Nothing is running in the session.
    Stopped,
}

impl CardState {
    /// Whether you should look at it. The states an attention marker is for.
    pub const fn wants_attention(self) -> bool {
        matches!(
            self,
            CardState::NeedsInput | CardState::ReadyForReview | CardState::Blocked
        )
    }

    pub const fn label(self) -> &'static str {
        match self {
            CardState::NeedsInput => "needs input",
            CardState::ReadyForReview => "ready for review",
            CardState::Blocked => "blocked",
            CardState::Waiting => "waiting",
            CardState::Working => "working",
            CardState::Done => "done",
            CardState::Stopped => "stopped",
        }
    }
}

/// Combine what the terminal shows with what the agent reported.
pub fn card_state(running: bool, waiting: bool, reported: Option<AgentState>) -> CardState {
    if !running {
        return CardState::Stopped;
    }
    if !waiting {
        return CardState::Working;
    }
    match reported {
        Some(AgentState::NeedsInput) => CardState::NeedsInput,
        Some(AgentState::ReadyForReview) => CardState::ReadyForReview,
        Some(AgentState::Blocked) => CardState::Blocked,
        // A reason this build does not model is still a reason to stop.
        Some(AgentState::Unknown) => CardState::NeedsInput,
        Some(AgentState::Done) => CardState::Done,
        Some(AgentState::Working) | None => CardState::Waiting,
    }
}

/// The colour an agent's card is tinted with, as hue and lightness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AgentColor {
    pub hue: f32,
    pub lightness: f32,
}

/// An agent's colour. A top-level agent gets its own, keyed by `key`; a
/// sub-agent takes its family's exactly, so an epic's agents read as one piece
/// of work.
pub fn agent_color(key: &str, family: Option<AgentColor>) -> AgentColor {
    family.unwrap_or(AgentColor {
        hue: okena_ui::identity_color::identity_hue(key),
        lightness: 0.55,
    })
}

/// A subtree's summary for its container header: how many sub-agents, and how
/// many of them want you.
pub fn subtree_summary(children: &[CardState]) -> String {
    let waiting = children.iter().filter(|c| c.wants_attention()).count();
    let noun = if children.len() == 1 {
        "sub-agent"
    } else {
        "sub-agents"
    };
    match waiting {
        0 => format!("{} {noun}", children.len()),
        n => format!("{} {noun} · {n} need you", children.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::{CardState, agent_color, card_state, subtree_summary};
    use okena_core::harness::AgentState;

    #[test]
    fn a_waiting_agent_that_says_why_is_flagged_with_its_reason() {
        let s = card_state(true, true, Some(AgentState::ReadyForReview));
        assert_eq!(s, CardState::ReadyForReview);
        assert!(s.wants_attention());
    }

    #[test]
    fn a_stale_reason_does_not_flag_an_agent_that_is_working_again() {
        assert_eq!(
            card_state(true, false, Some(AgentState::NeedsInput)),
            CardState::Working
        );
    }

    #[test]
    fn idle_without_a_reason_is_waiting_not_attention() {
        let s = card_state(true, true, None);
        assert_eq!(s, CardState::Waiting);
        assert!(!s.wants_attention());
    }

    #[test]
    fn nothing_running_is_stopped_whatever_it_last_said() {
        assert_eq!(
            card_state(false, true, Some(AgentState::Blocked)),
            CardState::Stopped
        );
    }

    #[test]
    fn an_unrecognised_reason_still_asks_for_attention() {
        assert!(card_state(true, true, Some(AgentState::Unknown)).wants_attention());
    }

    #[test]
    fn a_sub_agent_has_its_parents_colour() {
        let parent = agent_color("epic", None);
        assert_eq!(agent_color("story", Some(parent)), parent);
    }

    #[test]
    fn a_subtree_summary_counts_what_needs_you() {
        assert_eq!(
            subtree_summary(&[CardState::Working, CardState::NeedsInput]),
            "2 sub-agents · 1 need you"
        );
        assert_eq!(subtree_summary(&[CardState::Working]), "1 sub-agent");
    }
}
