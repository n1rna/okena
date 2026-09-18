//! Which model an agent CLI runs a launch on.
//!
//! A brief template names the model its flow is worth (QBL-419): a break-down
//! can run on something cheaper than the task it breaks down. The template may
//! name one model for everyone and a model per CLI; this module decides which
//! of those, if any, a given CLI is handed.
//!
//! The rule is "no guessing": a name is only passed to a CLI that is known to
//! mean something by it. A Claude alias is not translated into a Codex model,
//! and a CLI with nothing of its own runs its default. Shared by the daemon,
//! which passes the flag, and the clients, which show what will be passed.

use crate::agents::command_name;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a brief template's frontmatter says about models.
///
/// ```yaml
/// model: sonnet
/// models:
///   codex: gpt-5-codex
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentModels {
    /// One model for every CLI it fits (see [`fits`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// A model per CLI, keyed by its command name. Wins over `model` for that
    /// CLI, and is passed whatever it names.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, String>,
}

impl AgentModels {
    /// Read from a template's `model` and `models` fields. Blank values count
    /// as unset; `models` keys are matched case-insensitively.
    pub fn new(model: Option<String>, models: BTreeMap<String, String>) -> Self {
        let clean = |s: String| {
            let s = s.trim().to_string();
            (!s.is_empty()).then_some(s)
        };
        Self {
            model: model.and_then(clean),
            models: models
                .into_iter()
                .filter_map(|(k, v)| Some((command_name(k.trim()), clean(v)?)))
                .filter(|(k, _)| !k.is_empty())
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.models.is_empty()
    }

    /// The model `command` runs on, when the template names one for it.
    ///
    /// `models.<cli>` first; then `model`, but only if it [`fits`] that CLI;
    /// otherwise none, and the CLI picks its own default.
    pub fn for_agent(&self, command: &str) -> Option<&str> {
        let agent = command_name(command);
        if let Some(m) = self.models.get(&agent) {
            return Some(m);
        }
        self.model.as_deref().filter(|m| fits(m, &agent))
    }
}

/// The model a launch of `command` uses: the person's pick for this launch
/// when there is one, else the template's.
///
/// `picked` is `None` when nothing was picked, `Some("")` for "the CLI's
/// default" and `Some(model)` for a model.
pub fn resolve(models: &AgentModels, command: &str, picked: Option<&str>) -> Option<String> {
    match picked.map(str::trim) {
        Some("") => None,
        Some(m) => Some(m.to_string()),
        None => models.for_agent(command).map(str::to_string),
    }
}

/// Whether the shared `model` name belongs to `agent`'s family.
///
/// Claude's aliases and `claude-*` ids go to `claude`; OpenAI's `gpt-*`,
/// `o<n>` and `codex-*` names to `codex`. Nothing goes to `copilot` or an
/// agent okena does not know: those take a model only from `models`, because
/// their names for the same model differ.
pub fn fits(model: &str, agent: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    match command_name(agent).as_str() {
        "claude" => is_claude(&m),
        "codex" => is_openai(&m),
        _ => false,
    }
}

fn is_claude(m: &str) -> bool {
    let base = m.strip_suffix("[1m]").unwrap_or(m);
    matches!(base, "haiku" | "sonnet" | "opus" | "opusplan") || base.starts_with("claude-")
}

fn is_openai(m: &str) -> bool {
    m.starts_with("gpt-")
        || m.starts_with("codex-")
        || (m.starts_with('o') && m[1..].starts_with(|c: char| c.is_ascii_digit()))
}

/// The models the launcher offers for one launch of `command`. Fixed here on
/// purpose: a template can name any id, the picker only these.
pub fn choices(command: &str) -> &'static [&'static str] {
    match command_name(command).as_str() {
        "claude" => &["haiku", "sonnet", "opus"],
        "codex" => &["gpt-5-codex", "gpt-5", "gpt-5-mini"],
        "copilot" => &["claude-sonnet-4.5", "gpt-5", "claude-haiku-4.5"],
        _ => &[],
    }
}

/// The flag `command` takes a model by. `None` for a CLI okena does not know,
/// which is never handed one.
pub fn flag(command: &str) -> Option<&'static str> {
    match command_name(command).as_str() {
        "claude" | "codex" | "copilot" => Some("--model"),
        _ => None,
    }
}

/// The model `args` already set for `command`, e.g. through the extra
/// arguments in Settings, or a launch that is being resumed.
///
/// Reads `--model <m>` and `--model=<m>`, and for Codex also `-m <m>` and a
/// `-c model=<m>` config override.
pub fn model_in(command: &str, args: &[String]) -> Option<String> {
    let codex = command_name(command) == "codex";
    let mut it = args.iter().map(|a| a.trim());
    while let Some(arg) = it.next() {
        if arg == "--model" || (codex && arg == "-m") {
            return it.next().map(str::to_string);
        }
        if let Some(m) = arg.strip_prefix("--model=") {
            return Some(m.to_string());
        }
        if codex
            && (arg == "-c" || arg == "--config")
            && let Some(m) = it.next().and_then(|v| v.strip_prefix("model="))
        {
            return Some(m.trim_matches('"').to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{AgentModels, choices, fits, flag, model_in, resolve};
    use std::collections::BTreeMap;

    fn models(model: Option<&str>, per: &[(&str, &str)]) -> AgentModels {
        AgentModels::new(
            model.map(str::to_string),
            per.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_cli_entry_wins_over_the_shared_model() {
        let m = models(Some("sonnet"), &[("codex", "gpt-5-codex")]);
        assert_eq!(m.for_agent("claude"), Some("sonnet"));
        assert_eq!(m.for_agent("/usr/local/bin/codex"), Some("gpt-5-codex"));
        let m = models(Some("sonnet"), &[("claude", "opus")]);
        assert_eq!(m.for_agent("claude"), Some("opus"));
    }

    #[test]
    fn a_claude_name_goes_only_to_claude() {
        for name in ["opus", "Sonnet", "haiku", "opus[1m]", "claude-opus-4-1"] {
            let m = models(Some(name), &[]);
            assert_eq!(m.for_agent("claude"), Some(name), "{name}");
            assert_eq!(m.for_agent("codex"), None, "{name}");
            assert_eq!(m.for_agent("copilot"), None, "{name}");
        }
    }

    #[test]
    fn an_openai_name_goes_only_to_codex() {
        for name in ["gpt-5", "o3", "o4-mini", "codex-mini-latest"] {
            let m = models(Some(name), &[]);
            assert_eq!(m.for_agent("codex"), Some(name), "{name}");
            assert_eq!(m.for_agent("claude"), None, "{name}");
            assert_eq!(m.for_agent("copilot"), None, "{name}");
        }
        // Not an o-series name just for starting with an `o`.
        assert!(!fits("opus", "codex"));
    }

    #[test]
    fn copilot_takes_a_model_only_from_its_own_entry() {
        assert_eq!(models(Some("gpt-5"), &[]).for_agent("copilot"), None);
        let m = models(None, &[("Copilot", " gpt-5 ")]);
        assert_eq!(m.for_agent("copilot"), Some("gpt-5"));
    }

    #[test]
    fn nothing_set_is_no_model() {
        let m = models(None, &[]);
        assert!(m.is_empty());
        for agent in ["claude", "codex", "copilot", "aider"] {
            assert_eq!(m.for_agent(agent), None, "{agent}");
        }
        // Blank values are unset, not an empty model name.
        let m = models(Some("  "), &[("codex", "")]);
        assert!(m.is_empty());
    }

    #[test]
    fn a_pick_overrides_the_template_for_one_launch() {
        let m = models(Some("opus"), &[]);
        assert_eq!(resolve(&m, "claude", None).as_deref(), Some("opus"));
        assert_eq!(
            resolve(&m, "claude", Some("haiku")).as_deref(),
            Some("haiku")
        );
        // "CLI default" drops the template's model too.
        assert_eq!(resolve(&m, "claude", Some("")), None);
        assert_eq!(resolve(&m, "codex", None), None);
    }

    #[test]
    fn known_clis_have_a_flag_and_choices() {
        for agent in ["claude", "codex", "copilot"] {
            assert_eq!(flag(agent), Some("--model"));
            assert!(!choices(agent).is_empty());
        }
        assert_eq!(choices("claude"), ["haiku", "sonnet", "opus"]);
        assert_eq!(flag("aider"), None);
        assert!(choices("aider").is_empty());
    }

    #[test]
    fn a_model_already_in_the_args_is_found() {
        assert_eq!(
            model_in("claude", &strings(&["--verbose", "--model", "opus"])).as_deref(),
            Some("opus")
        );
        assert_eq!(
            model_in("copilot", &strings(&["--model=gpt-5"])).as_deref(),
            Some("gpt-5")
        );
        assert_eq!(
            model_in("codex", &strings(&["-m", "o3"])).as_deref(),
            Some("o3")
        );
        assert_eq!(
            model_in("codex", &strings(&["-c", "model=\"o3\""])).as_deref(),
            Some("o3")
        );
        // `-m` means something else to other CLIs.
        assert_eq!(model_in("claude", &strings(&["-m", "x"])), None);
        assert_eq!(model_in("claude", &strings(&["--verbose"])), None);
    }
}
