//! Building an agent's opening brief.
//!
//! One place that knows how to turn "this flow, these facts" into the text an
//! agent is launched with, so the four launch routes cannot drift apart in
//! what they tell an agent or in where they let an organisation override it.
//!
//! The prose lives in templates (`okena_knowledge::prompts`); this module's
//! job is the part that cannot: reading the configured knowledge root off
//! settings, and composing the variables whose value is a *decision* rather
//! than a field — whether a spec root is a store, whether a task has a
//! description worth including. A renderer with no conditionals pushes those
//! here on purpose, where they are testable.

use crate::workspace::persistence::{AppSettings, get_config_dir};
use okena_knowledge::discover::{self, Sources};
use okena_knowledge::prompts::{self, Brief, Flow, Vars};
use std::path::PathBuf;

/// The knowledge root briefs are read from: its key and its checkout.
///
/// `None` is the ordinary case — no store configured — and means the
/// built-ins. Named because it is threaded through every launch route, and
/// `Option<(String, PathBuf)>` in six signatures says nothing.
pub(super) type PromptRoot = Option<(String, PathBuf)>;

/// Where the configured prompt root lives on disk, if there is one.
///
/// Resolved through discovery rather than trusting a path from settings: the
/// key names a store okena knows about, and a store that has since been
/// unregistered or moved should fall back to the built-ins rather than read
/// whatever is at a stale path now.
pub(super) fn prompt_root(
    projects: &[okena_workspace::state::ProjectData],
    settings: &AppSettings,
) -> PromptRoot {
    let key = settings.harness.knowledge.prompt_root()?;
    let stores = discover::discover(&Sources {
        registry_path: okena_knowledge::registry::registry_path(&get_config_dir()),
        projects: super::knowledge::knowledge_project_sources(projects, settings),
    });
    let root = stores.root(key)?;
    // An unhealthy root is one discovery has already said it cannot read.
    // Launching from it would fail per-template anyway, less legibly.
    root.healthy
        .then(|| (root.key.clone(), PathBuf::from(&root.path)))
}

/// Render `flow` against the configured root, falling back to the built-in.
pub(super) fn build(flow: Flow, root: Option<&(String, PathBuf)>, vars: &Vars<'_>) -> Brief {
    prompts::brief(
        flow,
        root.map(|(key, path)| (key.as_str(), path.as_path())),
        vars,
    )
}

/// A block that is either absent or set off by a blank line.
///
/// The shape half the flow variables need: a description, a project list, a
/// store note. Templates place `{description}` on its own line, so an empty
/// value has to bring its own separators or leave a hole in the prose.
pub(super) fn block(body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        String::new()
    } else {
        format!("\n\n{body}")
    }
}

/// A list of `- name (path)` lines under the heading partial `heading`, or
/// nothing. The lines are structure; the heading is words, so it is a partial.
pub(super) fn project_block(
    heading: &str,
    projects: &[(String, String)],
    root: Option<&(String, PathBuf)>,
) -> String {
    if projects.is_empty() {
        return String::new();
    }
    let list = projects
        .iter()
        .map(|(name, path)| format!("- {name} ({path})"))
        .collect::<Vec<_>>()
        .join("\n");
    block(&fragment(heading, root, &Vars::from([("list", list)])))
}

/// Render one partial against the configured root.
///
/// Where code has to choose between wordings — a store or a folder, who
/// commits, a fan-out or a group — it picks the partial by name here, and the
/// sentence itself stays editable in knowledge.
pub(super) fn fragment(name: &str, root: Option<&(String, PathBuf)>, vars: &Vars<'_>) -> String {
    prompts::fragment(
        name,
        root.map(|(key, path)| (key.as_str(), path.as_path())),
        vars,
    )
    .text
}

/// Handle [`ActionRequest::PromptRender`].
///
/// Returns the rendered text, where it came from, and any placeholder the
/// template asked for that the flow does not fill — so a client can show a
/// brief that is visibly incomplete rather than sending it and wondering.
pub(super) fn render_action(
    flow: &str,
    vars: &std::collections::BTreeMap<String, String>,
    projects: &[okena_workspace::state::ProjectData],
    settings: &AppSettings,
) -> super::ActionResult {
    let Some(flow) = Flow::from_id(flow) else {
        return super::ActionResult::Err(format!(
            "unknown prompt flow `{flow}` — okena has {}",
            Flow::all()
                .iter()
                .map(|f| f.id())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    };
    let owned: Vars = vars.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    let brief = build(flow, prompt_root(projects, settings).as_ref(), &owned);
    let source = match &brief.source {
        prompts::Source::Root { key, path } => serde_json::json!({ "root": key, "path": path }),
        prompts::Source::Builtin => serde_json::json!({ "builtin": true }),
    };
    super::ActionResult::Ok(Some(serde_json::json!({
        "flow": flow.id(),
        "text": brief.rendered.text,
        "unknown": brief.rendered.unknown,
        "source": source,
    })))
}

#[cfg(test)]
mod tests {
    use super::{block, project_block};

    #[test]
    fn an_empty_block_contributes_nothing() {
        // Not "\n\n": the template already separates the paragraphs around it,
        // and an empty optional block must not open a hole in them.
        assert_eq!(block(""), "");
        assert_eq!(block("   \n  "), "");
    }

    #[test]
    fn a_block_brings_its_own_separation() {
        assert_eq!(block("why it matters"), "\n\nwhy it matters");
    }

    #[test]
    fn a_block_is_trimmed_so_a_ragged_field_does_not_show() {
        assert_eq!(block("  padded  \n"), "\n\npadded");
    }

    #[test]
    fn no_projects_means_no_list_and_no_label() {
        assert_eq!(project_block("given-projects", &[], None), "");
    }

    #[test]
    fn projects_are_listed_under_their_label() {
        let given = [
            ("okena".to_string(), "/p/okena".to_string()),
            ("web".to_string(), "/p/web".to_string()),
        ];
        assert_eq!(
            project_block("given-projects", &given, None),
            "\n\nProjects you were given:\n- okena (/p/okena)\n- web (/p/web)"
        );
    }
}
