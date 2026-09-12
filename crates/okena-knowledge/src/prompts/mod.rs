//! Where an agent's opening brief comes from.
//!
//! Every brief okena sends used to be a `format!` in the function that sent
//! it, which meant an organisation could not change how its agents are briefed
//! without a release. A brief is now a [`Flow`] — a named launch point with a
//! fixed set of variables — resolved to a template, rendered, and sent.
//!
//! Resolution has two steps and a guarantee:
//!
//! 1. the knowledge root the user pointed okena at, if it has a template for
//!    the flow;
//! 2. okena's built-in template, which is compiled in.
//!
//! The guarantee is that step 2 always exists, for every flow. There is no
//! state in which okena has nothing to say to an agent, so an organisation can
//! override one flow without having to supply the other five, and a store that
//! goes missing degrades to the defaults instead of breaking every launch.
//!
//! The built-ins are the same files okena writes when it materializes its own
//! knowledge store (see [`defaults`]), so "read the default and edit it" is
//! the same text that would otherwise have run.

pub mod defaults;
pub mod flows;
pub mod render;

pub use flows::Flow;
pub use render::{Rendered, Vars, placeholders, render, render_with};

use crate::frontmatter;
use std::path::Path;

/// Where a rendered brief came from, so the UI can say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A template in a knowledge root, at this path inside it.
    Root { key: String, path: String },
    /// okena's compiled-in default.
    Builtin,
}

impl Source {
    pub fn is_builtin(&self) -> bool {
        matches!(self, Source::Builtin)
    }
}

/// A brief, ready to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Brief {
    pub flow: Flow,
    pub source: Source,
    pub rendered: Rendered,
}

impl Brief {
    pub fn text(&self) -> &str {
        &self.rendered.text
    }
}

/// The built-in template body for `flow`, frontmatter stripped.
pub fn builtin(flow: Flow) -> &'static str {
    defaults::body(flow)
}

/// Build the brief for `flow`, preferring `root`'s template over the built-in.
///
/// `root` is the checkout directory of the knowledge root okena was pointed
/// at, or `None` to use the built-ins. A root that exists but has no template
/// for this flow falls through rather than erroring: overriding one flow must
/// not mean supplying all of them.
pub fn brief(flow: Flow, root: Option<(&str, &Path)>, vars: &Vars<'_>) -> Brief {
    let partial = |name: &str| partial_from(root, name);
    if let Some((key, dir)) = root
        && let Some((path, body)) = read_template(dir, flow)
    {
        return Brief {
            flow,
            source: Source::Root {
                key: key.to_string(),
                path,
            },
            rendered: render_with(&body, vars, &partial),
        };
    }
    Brief {
        flow,
        source: Source::Builtin,
        rendered: render_with(builtin(flow), vars, &partial),
    }
}

/// Render partial `name` on its own.
///
/// For the places code has to choose between wordings — a store or a folder,
/// who commits — so the choice stays in code and the words stay in knowledge.
/// Resolved like any include: the root's partial, then okena's.
pub fn fragment(name: &str, root: Option<(&str, &Path)>, vars: &Vars<'_>) -> Rendered {
    let partial = |n: &str| partial_from(root, n);
    render_with(&format!("{{>{name}}}"), vars, &partial)
}

/// A partial's body: the root's, when it has one, else okena's.
///
/// Per partial, like templates are per flow: overriding the reporting
/// instruction must not mean supplying every other shared sentence too.
fn partial_from(root: Option<(&str, &Path)>, name: &str) -> Option<String> {
    root.and_then(|(_, dir)| {
        let content = std::fs::read_to_string(dir.join(defaults::partial_path(name))).ok()?;
        let body = body_of(&content);
        (!body.is_empty()).then_some(body)
    })
    .or_else(|| defaults::partial_body(name))
}

/// Read `flow`'s template out of the root at `dir`.
///
/// Unreadable is treated as absent, deliberately: a permissions problem on one
/// template should cost that flow its override, not the launch.
fn read_template(dir: &Path, flow: Flow) -> Option<(String, String)> {
    let rel = flow.template_path();
    let path = dir.join(&rel);
    // The path is composed from a closed set of flow ids, so it cannot escape
    // the root — but the root itself may be a symlink farm, and reading
    // outside it is the one thing that would be a surprise.
    let content = std::fs::read_to_string(&path).ok()?;
    let body = body_of(&content);
    (!body.is_empty()).then_some((rel, body))
}

/// A template file's body: frontmatter removed, trailing blank space trimmed.
///
/// Trailing whitespace goes because a file ends with a newline and a brief
/// does not, and an agent should not receive a ragged prompt because of how
/// text editors save.
pub(crate) fn body_of(content: &str) -> String {
    let (_, body) = frontmatter::parse(content);
    body.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::{Flow, Source, Vars, body_of, brief, builtin};
    use std::path::Path;

    fn vars(pairs: &[(&'static str, &str)]) -> Vars<'static> {
        pairs.iter().map(|(k, v)| (*k, v.to_string())).collect()
    }

    fn write(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, content).expect("write");
    }

    #[test]
    fn every_flow_has_a_builtin() {
        // The guarantee the whole module rests on: there is no state in which
        // okena has nothing to say to an agent.
        for flow in Flow::all() {
            assert!(!builtin(*flow).is_empty(), "{flow} has no built-in");
        }
    }

    #[test]
    fn a_builtin_only_uses_variables_its_flow_fills() {
        // Otherwise okena ships a default that renders with a visible
        // `{placeholder}` in it — including through the partials it includes.
        for flow in Flow::all() {
            let filled: Vars = flow
                .variables()
                .iter()
                .map(|v| (*v, String::new()))
                .collect();
            let out = brief(*flow, None, &filled).rendered;
            assert!(out.is_complete(), "{flow} leaves {:?}", out.unknown);
        }
    }

    #[test]
    fn every_agent_is_told_how_to_report_that_it_is_waiting() {
        // The instruction lives once, in a partial; this pins that no flow
        // quietly stops including it.
        let reporting = super::defaults::partial_body("reporting").expect("reporting partial");
        for flow in Flow::all() {
            let filled: Vars = flow
                .variables()
                .iter()
                .map(|v| (*v, "x".to_string()))
                .collect();
            let text = brief(*flow, None, &filled).rendered.text;
            assert!(
                text.contains(&reporting),
                "{flow} does not tell the agent to report"
            );
        }
    }

    #[test]
    fn a_roots_partial_overrides_only_that_partial() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "templates/partials/reporting.md",
            "Ping us in #eng when stuck.",
        );
        let vars: Vars = Flow::AgentSession
            .variables()
            .iter()
            .map(|v| (*v, "x".to_string()))
            .collect();
        let b = brief(Flow::AgentSession, Some(("acme", dir.path())), &vars);
        // The flow's own template is still the built-in; only the partial is theirs.
        assert_eq!(b.source, Source::Builtin);
        assert!(
            b.text().ends_with("Ping us in #eng when stuck."),
            "{}",
            b.text()
        );
    }

    #[test]
    fn a_fragment_renders_one_partial_with_its_values() {
        let mut vars = Vars::new();
        vars.insert("also", "QBL-2, QBL-3".to_string());
        let out = super::fragment("group-note", None, &vars);
        assert!(out.is_complete(), "{:?}", out.unknown);
        assert!(out.text.contains("QBL-2, QBL-3"), "{}", out.text);
    }

    #[test]
    fn with_no_root_the_builtin_is_used() {
        let b = brief(
            Flow::AgentSession,
            None,
            &vars(&[("goal", "ship it"), ("projects", "")]),
        );
        assert_eq!(b.source, Source::Builtin);
        assert!(b.text().starts_with("ship it"), "{}", b.text());
    }

    #[test]
    fn a_roots_template_wins_over_the_builtin() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "templates/agent-session.md",
            "---\nfor: agent-session\n---\nOur way: {goal}\n",
        );
        let b = brief(
            Flow::AgentSession,
            Some(("acme-eng", dir.path())),
            &vars(&[("goal", "ship it")]),
        );
        assert_eq!(
            b.source,
            Source::Root {
                key: "acme-eng".into(),
                path: "templates/agent-session.md".into()
            }
        );
        assert_eq!(b.text(), "Our way: ship it");
    }

    #[test]
    fn a_flow_the_root_does_not_override_falls_through() {
        // Overriding one flow must not mean supplying all six.
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "templates/agent-session.md", "Ours: {goal}");
        let b = brief(
            Flow::SpecDraft,
            Some(("acme-eng", dir.path())),
            &Vars::new(),
        );
        assert_eq!(b.source, Source::Builtin);
    }

    #[test]
    fn an_empty_template_file_falls_through_rather_than_briefing_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "templates/agent-session.md",
            "---\nfor: agent-session\n---\n\n",
        );
        let b = brief(
            Flow::AgentSession,
            Some(("acme-eng", dir.path())),
            &Vars::new(),
        );
        assert_eq!(b.source, Source::Builtin);
    }

    #[test]
    fn a_missing_root_directory_degrades_to_the_builtin() {
        // A store unregistered or moved out from under us must not break every
        // launch.
        let b = brief(
            Flow::AgentSession,
            Some(("gone", Path::new("/nonexistent/knowledge/root"))),
            &vars(&[("goal", "g"), ("projects", "")]),
        );
        assert_eq!(b.source, Source::Builtin);
    }

    #[test]
    fn an_override_reports_its_own_unknown_placeholders() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "templates/agent-session.md",
            "{goal} and {mystery}",
        );
        let b = brief(
            Flow::AgentSession,
            Some(("acme-eng", dir.path())),
            &vars(&[("goal", "g")]),
        );
        assert_eq!(b.rendered.unknown, ["mystery"]);
        assert!(b.text().contains("{mystery}"), "{}", b.text());
    }

    #[test]
    fn frontmatter_is_not_part_of_the_brief() {
        assert_eq!(body_of("---\nfor: x\n---\nBody here\n"), "Body here");
        assert_eq!(body_of("No frontmatter\n\n"), "No frontmatter");
    }
}
