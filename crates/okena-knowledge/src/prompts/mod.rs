//! Where an agent's opening brief comes from.
//!
//! Every brief okena sends used to be a `format!` in the function that sent
//! it, which meant an organisation could not change how its agents are briefed
//! without a release. A brief is now a [`Flow`] — a named launch point with a
//! fixed set of variables — resolved to a template, rendered, and sent.
//!
//! Resolution walks every knowledge root okena can see, in order, and ends in
//! a guarantee:
//!
//! 1. each root in turn, and the first one holding the file wins;
//! 2. okena's built-in template, which is compiled in.
//!
//! Nobody picks a root for this. Knowledge is layered the way a search path is,
//! and the order is one saved list the user arranges ([`crate::order`],
//! QBL-425): the roots they have placed, in their order, then any root nobody
//! has placed yet, at the bottom in discovery order. Stores and projects' own
//! folders are one list, so either can sit above the other. An empty file is
//! not an answer and falls through, so a root can hold a placeholder without
//! silencing the layer below.
//!
//! The guarantee is that step 2 always exists, for every flow. There is no
//! state in which okena has nothing to say to an agent, so an organisation can
//! override one flow without having to supply the other five, and a store that
//! goes missing degrades to the defaults instead of breaking every launch.
//!
//! The built-ins are the same files okena writes into its own `okena-defaults`
//! store (see [`defaults`]), so "read the default and override it" starts from
//! the exact text that would otherwise have run. That store is okena's, always
//! rewritten to match, and never one of the layers: it is consulted last, as
//! the compiled-in step 2, and it is not part of the order — it cannot be
//! moved above the very roots meant to override it. Overriding a default means
//! putting a copy in a root of your own.
//!
//! Skills resolve the same way (see [`skill`]), but whole: a skill is handed to
//! an agent as a file, not rendered into a brief.

pub mod defaults;
pub mod flows;
pub mod render;

pub use flows::Flow;
pub use render::{Rendered, Vars, placeholders, render, render_with};

use crate::frontmatter;
use okena_core::agent_model::AgentModels;
use std::path::Path;

/// One root a file can be resolved from: the key naming it, and its checkout.
pub type Root<'a> = (&'a str, &'a Path);

/// The roots a file is looked for in, in the user's saved order; the first one
/// holding it wins.
///
/// okena's own defaults are never in here: they are the compiled-in last
/// resort, which is what makes them impossible to lose.
pub type Layers<'a> = &'a [Root<'a>];

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
    /// The template's frontmatter `name`, else the flow's id: what the
    /// launcher calls this brief.
    pub name: String,
    /// The model the template runs its agent on. Comes with the template that
    /// won, whole: an override that names no model runs the CLI's default,
    /// just as its text replaces the built-in's rather than merging with it.
    pub models: AgentModels,
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

/// Build the brief for `flow` from the first root that has a template for it,
/// falling back to the built-in.
///
/// `roots` is every knowledge root okena can see, in order; empty means the
/// built-ins. A root that exists but has no template for this flow falls
/// through rather than erroring: overriding one flow must not mean supplying
/// all of them, in any layer.
pub fn brief(flow: Flow, roots: Layers<'_>, vars: &Vars<'_>) -> Brief {
    let partial = |name: &str| partial_from(roots, name);
    let (source, template) = resolve(flow, roots);
    Brief {
        flow,
        source,
        name: template.name,
        models: template.models,
        rendered: render_with(&template.body, vars, &partial),
    }
}

/// Which template `flow` launches with, without rendering it: where it comes
/// from, what it is called, and the model it runs on. What a launcher shows
/// before anything starts, and what a launch passes as the model flag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateInfo {
    pub source: Source,
    pub name: String,
    pub models: AgentModels,
}

/// [`TemplateInfo`] for `flow`, resolved exactly as [`brief`] resolves it.
pub fn template_info(flow: Flow, roots: Layers<'_>) -> TemplateInfo {
    let (source, template) = resolve(flow, roots);
    TemplateInfo {
        source,
        name: template.name,
        models: template.models,
    }
}

/// The first root with a non-empty template for `flow`, else the built-in.
fn resolve(flow: Flow, roots: Layers<'_>) -> (Source, Template) {
    let rel = flow.template_path();
    for (key, dir) in roots {
        if let Ok(content) = std::fs::read_to_string(dir.join(&rel))
            && let Some(template) = Template::read(flow, &content)
        {
            let source = Source::Root {
                key: (*key).to_string(),
                path: rel,
            };
            return (source, template);
        }
    }
    let template = Template::read(flow, defaults::file(flow))
        .expect("every flow has a non-empty built-in template");
    (Source::Builtin, template)
}

/// A flow's template file, read: what it is called, what it runs on, and the
/// body to render.
struct Template {
    name: String,
    models: AgentModels,
    body: String,
}

impl Template {
    /// `None` for an empty body, which is not an answer and falls through.
    fn read(flow: Flow, content: &str) -> Option<Self> {
        let (fm, body) = frontmatter::parse(content);
        let body = body.trim_end();
        (!body.is_empty()).then(|| Template {
            name: fm.string("name").unwrap_or_else(|| flow.id().to_string()),
            models: fm.agent_models(),
            body: body.to_string(),
        })
    }
}

/// Render partial `name` on its own.
///
/// For the places code has to choose between wordings — a store or a folder,
/// who commits — so the choice stays in code and the words stay in knowledge.
/// Resolved like any include: each root in turn, then okena's.
pub fn fragment(name: &str, roots: Layers<'_>, vars: &Vars<'_>) -> Rendered {
    let partial = |n: &str| partial_from(roots, n);
    render_with(&format!("{{>{name}}}"), vars, &partial)
}

/// A skill's `SKILL.md`, and where it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub source: Source,
    /// The whole file, frontmatter included: the frontmatter is what makes it
    /// a skill to the agent it is handed to.
    pub content: String,
}

/// Skill `name`: the first root with a `skills/<name>/SKILL.md`, else okena's
/// built-in. `None` when nobody has it.
///
/// Per skill, like templates are per flow. An override replaces the whole
/// file; an empty one falls through, as an empty template does.
pub fn skill(name: &str, roots: Layers<'_>) -> Option<Skill> {
    // The name is composed into a path, so only a plain kebab-case name is one.
    if !okena_core::specs::is_kebab_id(name) {
        return None;
    }
    let rel = defaults::skill_path(name);
    for (key, dir) in roots {
        if let Ok(content) = std::fs::read_to_string(dir.join(&rel))
            && !content.trim().is_empty()
        {
            return Some(Skill {
                name: name.to_string(),
                source: Source::Root {
                    key: (*key).to_string(),
                    path: rel,
                },
                content,
            });
        }
    }
    defaults::skill_file(name).map(|content| Skill {
        name: name.to_string(),
        source: Source::Builtin,
        content: content.to_string(),
    })
}

/// Is `rel` a file the layers resolve — a flow template, a partial or a skill?
///
/// Docs and agents are listed from every root but read from the one you opened
/// them in, so nothing overrides them and asking is a category error.
pub fn is_layered(rel: &str) -> bool {
    is_skill(rel) || rel.starts_with("templates/")
}

/// Which root supplies `rel`, in layer order — for a default, the override
/// that is already beating it.
///
/// Uses the same emptiness rule resolution uses, so "supplied" here means what
/// it will mean at launch rather than merely "a file exists".
pub fn supplied_by<'a>(roots: Layers<'a>, rel: &str) -> Option<Root<'a>> {
    roots.iter().copied().find(|(_, dir)| supplies(dir, rel))
}

/// Does the root at `dir` supply `rel`?
pub fn supplies(dir: &Path, rel: &str) -> bool {
    if is_skill(rel) {
        // A skill is handed over whole, so its frontmatter is content.
        std::fs::read_to_string(dir.join(rel)).is_ok_and(|c| !c.trim().is_empty())
    } else {
        body_at(dir, rel).is_some()
    }
}

fn is_skill(rel: &str) -> bool {
    rel.starts_with("skills/") && rel.ends_with("/SKILL.md")
}

/// A partial's body: the first root that has it, else okena's.
///
/// Per partial, like templates are per flow: overriding the reporting
/// instruction must not mean supplying every other shared sentence too.
fn partial_from(roots: Layers<'_>, name: &str) -> Option<String> {
    let rel = defaults::partial_path(name);
    roots
        .iter()
        .find_map(|(_, dir)| body_at(dir, &rel))
        .or_else(|| defaults::partial_body(name))
}

/// The body of `rel` in the root at `dir`, when it has one worth using.
///
/// Unreadable is treated as absent, deliberately: a permissions problem on one
/// template should cost that flow its override, not the launch. The path comes
/// from a closed set of flow ids and partial names, so it cannot escape the
/// root — but the root itself may be a symlink farm, and reading outside it is
/// the one thing that would be a surprise.
fn body_at(dir: &Path, rel: &str) -> Option<String> {
    let content = std::fs::read_to_string(dir.join(rel)).ok()?;
    let body = body_of(&content);
    (!body.is_empty()).then_some(body)
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
            let out = brief(*flow, &[], &filled).rendered;
            assert!(out.is_complete(), "{flow} leaves {:?}", out.unknown);
        }
    }

    #[test]
    fn every_agent_is_told_how_to_report_that_it_is_waiting() {
        // The instruction lives once, in a partial; this pins that no flow
        // quietly stops including it.
        let reporting = super::defaults::partial_body("reporting").expect("reporting partial");
        for flow in Flow::all().iter().filter(|f| f.sent_within().is_none()) {
            let filled: Vars = flow
                .variables()
                .iter()
                .map(|v| (*v, "x".to_string()))
                .collect();
            let text = brief(*flow, &[], &filled).rendered.text;
            assert!(
                text.contains(&reporting),
                "{flow} does not tell the agent to report"
            );
        }
    }

    #[test]
    fn a_flow_sent_within_another_leaves_reporting_to_its_host() {
        // Its host tells the agent once; the embedded brief must not repeat it.
        let reporting = super::defaults::partial_body("reporting").expect("reporting partial");
        for flow in Flow::all() {
            let Some(host) = flow.sent_within() else {
                continue;
            };
            assert!(!builtin(*flow).contains("{>reporting}"), "{flow}");
            assert!(builtin(host).contains("{>reporting}"), "{host}");
            assert!(!builtin(*flow).contains(&reporting), "{flow}");
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
        let b = brief(Flow::AgentSession, &[("acme", dir.path())], &vars);
        // The flow's own template is still the built-in; only the partial is theirs.
        assert_eq!(b.source, Source::Builtin);
        assert!(
            b.text().ends_with("Ping us in #eng when stuck."),
            "{}",
            b.text()
        );
    }

    #[test]
    fn the_extension_build_brief_is_okenas_own_until_a_root_overrides_it() {
        // What the Extensions page's "Build an extension" launches with: the
        // built-in points the agent at the docs, the template and the target,
        // and stops short of installing.
        let b = brief(
            Flow::ExtensionBuild,
            &[],
            &vars(&[
                ("summary", "\n\na widget that shows the build queue"),
                ("projects", ""),
                ("context", ""),
            ]),
        );
        assert_eq!(b.source, Source::Builtin);
        assert_eq!(b.name, "extension-build");
        assert!(b.rendered.is_complete(), "{:?}", b.rendered.unknown);
        for expected in [
            "a widget that shows the build queue",
            "docs/reference/extensions.md",
            "examples/extension-template",
            "examples/extension-library",
            "wasm32-wasip2",
            "Settings → Extensions",
            "Do not install it",
        ] {
            assert!(
                b.text().contains(expected),
                "missing {expected}: {}",
                b.text()
            );
        }

        // No summary: still a whole brief, with nothing left dangling.
        let empty = brief(
            Flow::ExtensionBuild,
            &[],
            &vars(&[("summary", ""), ("projects", ""), ("context", "")]),
        );
        assert!(empty.rendered.is_complete(), "{:?}", empty.rendered.unknown);
        assert!(
            empty.text().starts_with("Build an okena extension."),
            "{}",
            empty.text()
        );
        assert!(empty.text().contains("examples/extension-template"));

        // A knowledge root's own copy replaces it, and says so, which is what
        // the launcher chip's tooltip names.
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "templates/briefs/extension-build.md",
            "---\nname: acme-extension\n---\nBuild it the acme way: {summary}",
        );
        let b = brief(
            Flow::ExtensionBuild,
            &[("store:acme", dir.path())],
            &vars(&[
                ("summary", "a queue widget"),
                ("projects", ""),
                ("context", ""),
            ]),
        );
        assert_eq!(b.text(), "Build it the acme way: a queue widget");
        assert_eq!(b.name, "acme-extension");
        assert_eq!(
            b.source,
            Source::Root {
                key: "store:acme".into(),
                path: "templates/briefs/extension-build.md".into()
            }
        );
    }

    #[test]
    fn a_fragment_renders_one_partial_with_its_values() {
        let mut vars = Vars::new();
        vars.insert("also", "QBL-2, QBL-3".to_string());
        let out = super::fragment("group-note", &[], &vars);
        assert!(out.is_complete(), "{:?}", out.unknown);
        assert!(out.text.contains("QBL-2, QBL-3"), "{}", out.text);
    }

    #[test]
    fn builtins_run_work_on_opus_and_helpers_on_sonnet() {
        use Flow::*;
        let expected = |flow: Flow| match flow {
            TaskStart | TasksStart | TaskCoordinate | TasksCoordinate | TaskVerify => Some("opus"),
            TaskBreakDown | TaskCreate | TaskRefine | SpecDraft | DocumentRefine
            | KnowledgeDraft | ProjectScan | ProjectsScan => Some("sonnet"),
            // Building an extension is implementation work, not a helper.
            ExtensionBuild => Some("opus"),
            AgentSession => None,
        };
        for flow in Flow::all() {
            let b = brief(*flow, &[], &Vars::new());
            assert_eq!(b.models.model.as_deref(), expected(*flow), "{flow}");
            assert!(b.models.models.is_empty(), "{flow}");
            // Every built-in is named for its flow.
            assert_eq!(b.name, flow.id());
        }
    }

    #[test]
    fn a_roots_template_replaces_the_builtins_model_along_with_its_text() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layers = [("store:acme", dir.path())];
        write(
            dir.path(),
            "templates/briefs/task-start.md",
            "---\nname: acme-start\nmodels:\n  codex: gpt-5-codex\n---\nWork on {key}",
        );
        let b = brief(Flow::TaskStart, &layers, &Vars::new());
        assert_eq!(b.name, "acme-start");
        assert_eq!(b.models.for_agent("codex"), Some("gpt-5-codex"));
        // Not merged with the built-in's `opus`: the override names none.
        assert_eq!(b.models.for_agent("claude"), None);

        write(
            dir.path(),
            "templates/briefs/task-start.md",
            "---\nmodel: haiku\n---\nx",
        );
        let b = brief(Flow::TaskStart, &layers, &Vars::new());
        assert_eq!(b.models.for_agent("claude"), Some("haiku"));
        // No `name`: called after its flow.
        assert_eq!(b.name, "task-start");

        // An empty override falls through, model and all.
        write(
            dir.path(),
            "templates/briefs/task-start.md",
            "---\nmodel: haiku\n---\n\n",
        );
        let b = brief(Flow::TaskStart, &layers, &Vars::new());
        assert_eq!(b.source, Source::Builtin);
        assert_eq!(b.models.for_agent("claude"), Some("opus"));
    }

    #[test]
    fn the_first_root_with_the_file_wins_and_the_rest_fall_through_to_it() {
        // The layering, on one template: a store holds it, a project root
        // holds it too, and the store is listed first, so the store wins.
        let store = tempfile::tempdir().expect("tempdir");
        let project = tempfile::tempdir().expect("tempdir");
        write(store.path(), "templates/briefs/agent-session.md", "Store: {goal}");
        write(
            project.path(),
            "templates/briefs/agent-session.md",
            "Project: {goal}",
        );
        let layers = [("store:acme", store.path()), ("path:/p/web", project.path())];

        let b = brief(Flow::AgentSession, &layers, &vars(&[("goal", "ship it")]));
        assert_eq!(b.text(), "Store: ship it");
        assert_eq!(
            b.source,
            Source::Root {
                key: "store:acme".into(),
                path: "templates/briefs/agent-session.md".into()
            }
        );

        // Take it out of the store and the project root's copy takes over —
        // the acceptance case for removing an override one layer up.
        std::fs::remove_file(store.path().join("templates/briefs/agent-session.md")).expect("rm");
        let b = brief(Flow::AgentSession, &layers, &vars(&[("goal", "ship it")]));
        assert_eq!(b.text(), "Project: ship it");

        // With neither, the built-in is still there.
        std::fs::remove_file(project.path().join("templates/briefs/agent-session.md")).expect("rm");
        assert_eq!(
            brief(
                Flow::AgentSession,
                &layers,
                &vars(&[("goal", "g"), ("projects", "")])
            )
            .source,
            Source::Builtin
        );
    }

    #[test]
    fn an_override_at_the_old_flat_brief_path_no_longer_applies() {
        // The briefs moved under `templates/briefs/` and there is no
        // migration (QBL-427): a copy left where they used to live is read by
        // nothing, and the built-in is what launches.
        let store = tempfile::tempdir().expect("tempdir");
        let layers = [("store:acme", store.path())];
        write(store.path(), "templates/agent-session.md", "Old: {goal}");

        let b = brief(Flow::AgentSession, &layers, &vars(&[("goal", "g")]));
        assert_eq!(b.source, Source::Builtin);
        assert!(!b.text().starts_with("Old:"), "{}", b.text());

        // The same words at the new path do apply.
        write(store.path(), "templates/briefs/agent-session.md", "New: {goal}");
        let b = brief(Flow::AgentSession, &layers, &vars(&[("goal", "g")]));
        assert_eq!(b.text(), "New: g");
    }

    #[test]
    fn an_empty_file_in_an_earlier_root_does_not_silence_a_later_one() {
        // An empty override is a placeholder, not an answer: it must not
        // shadow the layer that does have something to say.
        let first = tempfile::tempdir().expect("tempdir");
        let second = tempfile::tempdir().expect("tempdir");
        write(
            first.path(),
            "templates/briefs/agent-session.md",
            "---\nfor: agent-session\n---\n\n",
        );
        write(first.path(), "templates/partials/reporting.md", "   \n");
        write(second.path(), "templates/briefs/agent-session.md", "Second: {goal}");
        write(
            second.path(),
            "templates/partials/reporting.md",
            "Ping #eng.",
        );
        let layers = [("store:a", first.path()), ("store:b", second.path())];

        let b = brief(Flow::AgentSession, &layers, &vars(&[("goal", "g")]));
        assert_eq!(b.text(), "Second: g");
        assert_eq!(super::partial_from(&layers, "reporting").as_deref(), Some("Ping #eng."));
    }

    #[test]
    fn each_layer_supplies_only_what_it_has() {
        // A store overrides one partial, a project root overrides a different
        // template, and neither has to supply the other's file.
        let store = tempfile::tempdir().expect("tempdir");
        let project = tempfile::tempdir().expect("tempdir");
        write(
            store.path(),
            "templates/partials/reporting.md",
            "Ping us in #eng when stuck.",
        );
        write(project.path(), "templates/briefs/spec-draft.md", "Draft it: {change}");
        let layers = [("store:acme", store.path()), ("path:/p/web", project.path())];

        let session: Vars = Flow::AgentSession
            .variables()
            .iter()
            .map(|v| (*v, "x".to_string()))
            .collect();
        let b = brief(Flow::AgentSession, &layers, &session);
        // Its template is still okena's; only the store's partial is theirs.
        assert_eq!(b.source, Source::Builtin);
        assert!(b.text().ends_with("Ping us in #eng when stuck."), "{}", b.text());

        let d = brief(Flow::SpecDraft, &layers, &vars(&[("change", "add-login")]));
        assert_eq!(
            d.source,
            Source::Root {
                key: "path:/p/web".into(),
                path: "templates/briefs/spec-draft.md".into()
            }
        );
    }

    #[test]
    fn a_skill_resolves_through_the_layers_like_a_template() {
        let store = tempfile::tempdir().expect("tempdir");
        let project = tempfile::tempdir().expect("tempdir");
        let rel = super::defaults::skill_path("project-map");
        write(project.path(), &rel, "---\nname: project-map\n---\nTheirs.\n");
        let layers = [("store:acme", store.path()), ("path:/p/web", project.path())];

        // Only the project root has it, so it wins over the built-in.
        let s = super::skill("project-map", &layers).expect("skill");
        assert_eq!(
            s.source,
            Source::Root {
                key: "path:/p/web".into(),
                path: rel.clone()
            }
        );

        // The store gets one too, and being listed first it takes over.
        write(store.path(), &rel, "---\nname: project-map\n---\nOurs.\n");
        let s = super::skill("project-map", &layers).expect("skill");
        assert!(s.content.contains("Ours."), "{}", s.content);
    }

    #[test]
    fn supplied_by_names_the_root_an_override_would_have_to_beat() {
        let store = tempfile::tempdir().expect("tempdir");
        let project = tempfile::tempdir().expect("tempdir");
        let skill = super::defaults::skill_path("project-map");
        write(project.path(), "templates/briefs/spec-draft.md", "Theirs");
        write(store.path(), &skill, "---\nname: project-map\n---\nOurs\n");
        // An empty file is not an override, here as at launch.
        write(store.path(), "templates/briefs/spec-draft.md", "---\nfor: x\n---\n");
        let layers = [("store:acme", store.path()), ("path:/p/web", project.path())];

        assert_eq!(
            super::supplied_by(&layers, "templates/briefs/spec-draft.md").map(|(k, _)| k),
            Some("path:/p/web")
        );
        assert_eq!(
            super::supplied_by(&layers, &skill).map(|(k, _)| k),
            Some("store:acme")
        );
        assert_eq!(super::supplied_by(&layers, "templates/briefs/task-start.md"), None);

        // Only the kinds that have defaults are layered at all.
        assert!(super::is_layered("templates/partials/reporting.md"));
        assert!(super::is_layered(&skill));
        assert!(!super::is_layered("docs/ci/pipeline.md"));
        assert!(!super::is_layered("agents/reviewer.md"));
        assert!(!super::is_layered("README.md"));
    }

    #[test]
    fn with_no_root_the_builtin_is_used() {
        let b = brief(
            Flow::AgentSession,
            &[],
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
            "templates/briefs/agent-session.md",
            "---\nfor: agent-session\n---\nOur way: {goal}\n",
        );
        let b = brief(
            Flow::AgentSession,
            &[("acme-eng", dir.path())],
            &vars(&[("goal", "ship it")]),
        );
        assert_eq!(
            b.source,
            Source::Root {
                key: "acme-eng".into(),
                path: "templates/briefs/agent-session.md".into()
            }
        );
        assert_eq!(b.text(), "Our way: ship it");
    }

    #[test]
    fn a_flow_the_root_does_not_override_falls_through() {
        // Overriding one flow must not mean supplying all six.
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "templates/briefs/agent-session.md", "Ours: {goal}");
        let b = brief(Flow::SpecDraft, &[("acme-eng", dir.path())], &Vars::new());
        assert_eq!(b.source, Source::Builtin);
    }

    #[test]
    fn an_empty_template_file_falls_through_rather_than_briefing_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "templates/briefs/agent-session.md",
            "---\nfor: agent-session\n---\n\n",
        );
        let b = brief(Flow::AgentSession, &[("acme-eng", dir.path())], &Vars::new());
        assert_eq!(b.source, Source::Builtin);
    }

    #[test]
    fn a_missing_root_directory_degrades_to_the_builtin() {
        // A store unregistered or moved out from under us must not break every
        // launch.
        let b = brief(
            Flow::AgentSession,
            &[("gone", Path::new("/nonexistent/knowledge/root"))],
            &vars(&[("goal", "g"), ("projects", "")]),
        );
        assert_eq!(b.source, Source::Builtin);
    }

    #[test]
    fn an_override_reports_its_own_unknown_placeholders() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "templates/briefs/agent-session.md",
            "{goal} and {mystery}",
        );
        let b = brief(
            Flow::AgentSession,
            &[("acme-eng", dir.path())],
            &vars(&[("goal", "g")]),
        );
        assert_eq!(b.rendered.unknown, ["mystery"]);
        assert!(b.text().contains("{mystery}"), "{}", b.text());
    }

    #[test]
    fn with_no_root_a_skill_is_the_builtin_file_whole() {
        let s = super::skill(super::defaults::PROJECT_MAP_SKILL, &[]).expect("built in");
        assert_eq!(s.source, Source::Builtin);
        assert!(
            s.content.starts_with("---\nname: project-map\n"),
            "{}",
            s.content
        );
    }

    #[test]
    fn a_roots_skill_replaces_the_builtin() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "skills/project-map/SKILL.md",
            "---\nname: project-map\ndescription: Ours\n---\nMap it our way.\n",
        );
        let s = super::skill("project-map", &[("acme-eng", dir.path())]).expect("skill");
        assert_eq!(
            s.source,
            Source::Root {
                key: "acme-eng".into(),
                path: "skills/project-map/SKILL.md".into()
            }
        );
        assert!(s.content.contains("Map it our way."), "{}", s.content);
        assert!(
            !s.content.contains("Where to start"),
            "not merged with the built-in"
        );
    }

    #[test]
    fn a_missing_or_empty_skill_override_falls_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source =
            || super::skill("project-map", &[("acme-eng", dir.path())]).map(|s| s.source);
        assert_eq!(source(), Some(Source::Builtin));
        write(dir.path(), "skills/project-map/SKILL.md", "\n\n");
        assert_eq!(source(), Some(Source::Builtin));
    }

    #[test]
    fn a_skill_name_is_never_a_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "SKILL.md", "escaped");
        assert!(super::skill("..", &[("acme-eng", dir.path())]).is_none());
        assert!(super::skill("../project-map", &[("acme-eng", dir.path())]).is_none());
        assert!(super::skill("no-such-skill", &[]).is_none());
    }

    #[test]
    fn one_agent_on_several_tasks_is_told_to_keep_each_in_its_own_worktrees() {
        let b = brief(
            Flow::TasksStart,
            &[],
            &vars(&[
                ("key", "QBL-1 and QBL-2"),
                ("tasks", "\n\n## QBL-1: a\n\n## QBL-2: b"),
                ("note", ""),
                ("verify", ""),
            ]),
        );
        let text = b.text();
        assert!(text.contains("QBL-1 and QBL-2"), "{text}");
        assert!(text.contains("## QBL-2: b"), "{text}");
        assert!(text.contains("that task's worktrees"), "{text}");
        // There is no one branch to be on.
        assert!(!text.contains("that branch"), "{text}");
    }

    #[test]
    fn a_coordinator_over_picked_tasks_is_told_it_has_no_worktree() {
        let text = builtin(Flow::TasksCoordinate);
        assert!(!text.contains("{branch}"), "{text}");
        assert!(text.contains("no worktree of your own"), "{text}");
        assert!(text.contains("`okena_start_work` creates the worktrees"), "{text}");
    }

    #[test]
    fn a_task_in_a_group_and_a_picked_sibling_name_their_branch_and_worktrees() {
        let group = super::fragment(
            "task-in-group",
            &[],
            &vars(&[
                ("key", "QBL-2"),
                ("title", "Fix login"),
                ("url", "http://x/2"),
                ("branch", "fix/qbl-2-login"),
                ("worktrees", "- okena (/wt/qbl-2)"),
                ("description", ""),
            ]),
        );
        assert!(group.is_complete(), "{:?}", group.unknown);
        for needle in ["QBL-2: Fix login", "fix/qbl-2-login", "- okena (/wt/qbl-2)"] {
            assert!(group.text.contains(needle), "{needle}: {}", group.text);
        }

        let sibling = super::fragment(
            "picked-sibling",
            &[],
            &vars(&[
                ("key", "QBL-3"),
                ("branch", "feat/qbl-3"),
                ("worktrees", "/wt/qbl-3"),
            ]),
        );
        assert!(sibling.is_complete(), "{:?}", sibling.unknown);
        assert_eq!(sibling.text, "- QBL-3 on `feat/qbl-3`: /wt/qbl-3");

        let note = super::fragment(
            "picked-fan-out-note",
            &[],
            &vars(&[("siblings", sibling.text.as_str())]),
        );
        assert!(note.text.contains("/wt/qbl-3"), "{}", note.text);
    }

    #[test]
    fn frontmatter_is_not_part_of_the_brief() {
        assert_eq!(body_of("---\nfor: x\n---\nBody here\n"), "Body here");
        assert_eq!(body_of("No frontmatter\n\n"), "No frontmatter");
    }
}
