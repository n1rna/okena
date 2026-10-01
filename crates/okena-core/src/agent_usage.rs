//! What an agent session has used: its tokens, and its cost where the agent
//! CLI reports one.
//!
//! The daemon reads both from what the agent's own CLI writes — okena prices
//! nothing — and keeps them on the session, so a client only formats them.
//! Lives in `okena-core` because the figure is saved with the session, sent to
//! every client, and worded the same in the Info tab and on the sidebar card.

use serde::{Deserialize, Serialize};

/// One session's usage so far.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentUsage {
    /// Every token the conversation has used: input (cache writes and reads
    /// included) and output. Zero when only a cost is known yet.
    #[serde(default)]
    pub tokens: u64,
    /// Whether `tokens` is output alone. Copilot records nothing else while
    /// it runs; its input is written when the session shuts down.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub output_only: bool,
    /// The cost the agent CLI itself reports, in US dollars. `None` for a CLI
    /// that reports none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

impl AgentUsage {
    /// The token figure alone, as a sidebar card has room for: `1.2M`, with
    /// no unit. `None` when no tokens are known.
    pub fn tokens_short(&self) -> Option<String> {
        (self.tokens > 0).then(|| format_tokens(self.tokens))
    }

    /// The token figure spelled out, as the Info tab shows it: `1.2M tokens`,
    /// or `45.2k output tokens` while only output is known.
    pub fn tokens_label(&self) -> Option<String> {
        (self.tokens > 0).then(|| {
            let unit = if self.output_only {
                "output tokens"
            } else {
                "tokens"
            };
            format!("{} {unit}", format_tokens(self.tokens))
        })
    }

    /// The cost as the Info tab shows it. `None` when the CLI reported none,
    /// or nothing has been spent yet.
    pub fn cost_label(&self) -> Option<String> {
        self.cost_usd.filter(|c| *c > 0.0).map(format_cost)
    }

    /// Whether `other` reads the same wherever it is shown. Figures move on
    /// every turn; only a change someone can see is worth sending to clients.
    pub fn shown_as(&self, other: &Self) -> bool {
        self.tokens_label() == other.tokens_label() && self.cost_label() == other.cost_label()
    }
}

/// A token count as the UI shows it: exact below a thousand, then `k`, `M`
/// and `B` — to one decimal below a hundred of the unit, whole above.
pub fn format_tokens(tokens: u64) -> String {
    if tokens < 1000 {
        return tokens.to_string();
    }
    let mut value = tokens as f64 / 1000.0;
    for unit in ["k", "M"] {
        if value < 99.95 {
            return format!("{value:.1}{unit}");
        }
        if value < 999.5 {
            return format!("{value:.0}{unit}");
        }
        value /= 1000.0;
    }
    if value < 99.95 {
        format!("{value:.1}B")
    } else {
        format!("{value:.0}B")
    }
}

/// A dollar cost as the UI shows it: to the cent, and `<$0.01` below one.
pub fn format_cost(usd: f64) -> String {
    if usd < 0.005 {
        "<$0.01".to_string()
    } else {
        format!("${usd:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentUsage, format_cost, format_tokens};

    #[test]
    fn tokens_are_shortened_by_magnitude() {
        for (tokens, shown) in [
            (0, "0"),
            (812, "812"),
            (999, "999"),
            (1000, "1.0k"),
            (45_230, "45.2k"),
            (99_949, "99.9k"),
            (99_950, "100k"),
            (812_400, "812k"),
            (999_499, "999k"),
            (999_500, "1.0M"),
            (1_234_567, "1.2M"),
            (29_640_952, "29.6M"),
            (120_000_000, "120M"),
            (999_600_000, "1.0B"),
            (2_500_000_000, "2.5B"),
        ] {
            assert_eq!(format_tokens(tokens), shown, "{tokens}");
        }
    }

    #[test]
    fn cost_is_shown_to_the_cent() {
        assert_eq!(format_cost(21.010909), "$21.01");
        assert_eq!(format_cost(0.4251), "$0.43");
        assert_eq!(format_cost(0.005), "$0.01");
        assert_eq!(format_cost(0.0031), "<$0.01");
    }

    #[test]
    fn nothing_known_is_worded_as_nothing() {
        let none = AgentUsage::default();
        assert_eq!(none.tokens_label(), None);
        assert_eq!(none.tokens_short(), None);
        assert_eq!(none.cost_label(), None);
        // A status line that has spent nothing yet reports a cost of zero.
        let unspent = AgentUsage {
            cost_usd: Some(0.0),
            ..Default::default()
        };
        assert_eq!(unspent.cost_label(), None);
    }

    #[test]
    fn output_alone_is_labelled_as_output() {
        let live = AgentUsage {
            tokens: 45_230,
            output_only: true,
            cost_usd: None,
        };
        assert_eq!(live.tokens_label().as_deref(), Some("45.2k output tokens"));
        assert_eq!(live.tokens_short().as_deref(), Some("45.2k"));
        let full = AgentUsage {
            tokens: 1_234_567,
            output_only: false,
            cost_usd: Some(1.5),
        };
        assert_eq!(full.tokens_label().as_deref(), Some("1.2M tokens"));
        assert_eq!(full.cost_label().as_deref(), Some("$1.50"));
    }

    #[test]
    fn only_a_visible_change_differs() {
        let a = AgentUsage {
            tokens: 1_234_000,
            output_only: false,
            cost_usd: Some(1.501),
        };
        let mut b = a.clone();
        b.tokens += 900;
        b.cost_usd = Some(1.504);
        assert!(a.shown_as(&b));
        b.tokens += 100_000;
        assert!(!a.shown_as(&b));
        let mut c = a.clone();
        c.cost_usd = Some(1.52);
        assert!(!a.shown_as(&c));
        let mut d = a.clone();
        d.output_only = true;
        assert!(!a.shown_as(&d), "the same number, meaning something else");
    }

    #[test]
    fn usage_from_an_older_peer_still_parses() {
        let usage: AgentUsage = serde_json::from_str(r#"{"tokens":5}"#).unwrap();
        assert_eq!(
            usage,
            AgentUsage {
                tokens: 5,
                output_only: false,
                cost_usd: None
            }
        );
        assert_eq!(serde_json::to_string(&usage).unwrap(), r#"{"tokens":5}"#);
    }
}
