//! What an agent's sidebar card says about it.
//!
//! Pure, and GPUI-free like `activity_order`. What an agent is doing is decided
//! by the daemon from the agent's own signals — see `okena_core::agent_activity`
//! — so the sidebar, the session panel and the Tasks view cannot disagree. A
//! card adds only what the sidebar knows itself: whether anything is running
//! in the session at all.

pub use okena_core::agent_activity::AgentActivity as CardState;

/// A card's state: stopped when nothing runs, otherwise what the daemon says.
///
/// A running agent the daemon has not described — one on a daemon from before
/// agent activity — shows as working, which is what it was always shown as.
pub fn card_state(running: bool, activity: Option<CardState>) -> CardState {
    if !running {
        return CardState::Stopped;
    }
    activity.unwrap_or(CardState::Working)
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

    #[test]
    fn a_running_agent_shows_what_the_daemon_says() {
        let s = card_state(true, Some(CardState::ReadyForReview));
        assert_eq!(s, CardState::ReadyForReview);
        assert!(s.wants_attention());
        assert_eq!(
            card_state(true, Some(CardState::NeedsInput)),
            CardState::NeedsInput
        );
    }

    #[test]
    fn waiting_without_a_reason_is_not_attention() {
        let s = card_state(true, Some(CardState::Waiting));
        assert_eq!(s, CardState::Waiting);
        assert!(!s.wants_attention());
    }

    #[test]
    fn nothing_running_is_stopped_whatever_it_last_said() {
        assert_eq!(
            card_state(false, Some(CardState::Blocked)),
            CardState::Stopped
        );
    }

    #[test]
    fn an_agent_the_daemon_has_not_described_is_working() {
        assert_eq!(card_state(true, None), CardState::Working);
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
