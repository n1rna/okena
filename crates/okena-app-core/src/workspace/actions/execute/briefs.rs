//! Building an agent's opening brief.
//!
//! One place that knows how to turn "this flow, these facts" into the text an
//! agent is launched with, so the four launch routes cannot drift apart in
//! what they tell an agent or in where they let an organisation override it.
//!
//! The prose lives in templates (`okena_knowledge::prompts`); this module's
//! job is the part that cannot: putting the knowledge roots in the order
//! resolution reads them, and composing the variables whose value is a
//! *decision* rather
//! than a field — whether a spec root is a store, whether a task has a
//! description worth including. A renderer with no conditionals pushes those
//! here on purpose, where they are testable.

use crate::workspace::persistence::{AppSettings, get_config_dir};
use okena_knowledge::discover;
use okena_knowledge::prompts::{self, Brief, Flow, Vars};
use std::path::PathBuf;

/// The knowledge roots briefs are read from, in order: each one's key and its
/// checkout, most authoritative first.
///
/// Empty means the built-ins alone. Named because it is threaded through every
/// launch route, and `Vec<(String, PathBuf)>` in six signatures says nothing.
pub(super) type PromptRoots = Vec<(String, PathBuf)>;

/// Every root a brief may be read from, in resolution order.
///
/// Resolved through discovery rather than from settings: nobody picks a root
/// for this any more (QBL-415). Discovery already hands them back in the one
/// saved order (QBL-425) — the roots the user arranged, then any they have not
/// arranged yet — so the order here is simply the order it found them in.
///
/// Two are left out. An unhealthy root is one discovery has already said it
/// cannot read, and launching from it would fail per-template anyway, less
/// legibly. okena's own `okena-defaults` store is left out because it holds a
/// copy of the built-ins: including it would put okena's answer *before* the
/// layers that are meant to override it, and the compiled-in fallback already
/// gives the same text with no way to lose it.
pub(super) fn prompt_roots(
    projects: &[okena_workspace::state::ProjectData],
    settings: &AppSettings,
) -> PromptRoots {
    layers_of(&discover::discover(&super::knowledge::knowledge_sources(
        &okena_knowledge::registry::registry_path(&get_config_dir()),
        &super::knowledge::knowledge_project_sources(projects, settings),
        settings,
    )))
}

/// The roots of `stores` a brief resolves through, in the saved order.
///
/// Split from [`prompt_roots`] so the choice can be tested without a profile
/// on disk; the rule it encodes is the one the doc above describes.
pub(super) fn layers_of(stores: &okena_core::knowledge::KnowledgeStores) -> PromptRoots {
    stores
        .roots
        .iter()
        .filter(|r| r.healthy && !r.builtin)
        .map(|r| (r.key.clone(), PathBuf::from(&r.path)))
        .collect()
}

/// Borrow `roots` in the shape resolution takes.
pub(super) fn layers(roots: &PromptRoots) -> Vec<prompts::Root<'_>> {
    roots
        .iter()
        .map(|(key, path)| (key.as_str(), path.as_path()))
        .collect()
}

/// Render `flow` against the roots, falling back to the built-in.
pub(super) fn build(flow: Flow, roots: &PromptRoots, vars: &Vars<'_>) -> Brief {
    prompts::brief(flow, &layers(roots), vars)
}

/// The model `flow`'s template runs its agent on, with `picked` — the model
/// the person chose for this launch, if any — over it.
pub(super) fn launch_model(
    flow: Flow,
    roots: &PromptRoots,
    picked: Option<String>,
) -> super::agent_options::LaunchModel {
    let models = prompts::template_info(flow, &layers(roots)).models;
    super::agent_options::LaunchModel::new(models, picked)
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
    roots: &PromptRoots,
) -> String {
    if projects.is_empty() {
        return String::new();
    }
    let list = projects
        .iter()
        .map(|(name, path)| format!("- {name} ({path})"))
        .collect::<Vec<_>>()
        .join("\n");
    block(&fragment(heading, roots, &Vars::from([("list", list)])))
}

/// Most bytes the listed context lines of a brief may take.
///
/// A brief is read in full before the agent does anything, so a long pick is
/// listed compactly past this: the rest are named by title, and the agent
/// looks them up through okena's MCP tools.
pub(super) const CONTEXT_BUDGET_BYTES: usize = 4 * 1024;

/// The `context` block of a brief: what was picked at launch, by owner, each
/// with its kind, title and absolute path — the agent reads what it needs;
/// nothing is inlined, and descriptions are left to the lookup tools.
///
/// Lines are listed until the next would take the list past
/// [`CONTEXT_BUDGET_BYTES`]; that item and every one after it are named by
/// title in the `context-more` line instead. With `loaded`, skills and agents
/// were installed into the session itself, so they are named in the
/// `context-installed` line instead of listed. Lines are structure; the
/// heading and those lines are words, so they are partials.
pub(super) fn context_block(
    items: &[okena_core::context::ContextItem],
    loaded: bool,
    roots: &PromptRoots,
) -> String {
    let mut owners: Vec<(&str, Vec<&okena_core::context::ContextItem>)> = Vec::new();
    let mut named: Vec<String> = Vec::new();
    for item in items {
        let kind = item.reference.kind;
        if loaded && kind.is_installable() {
            named.push(format!("{} ({})", item.title, kind.label().to_lowercase()));
            continue;
        }
        match owners
            .iter_mut()
            .find(|(owner, _)| *owner == item.owner_name)
        {
            Some((_, listed)) => listed.push(item),
            None => owners.push((&item.owner_name, vec![item])),
        }
    }
    let mut list: Vec<String> = Vec::new();
    let mut more: Vec<&str> = Vec::new();
    let mut used = 0;
    for (owner, owned) in &owners {
        let heading = format!("- {owner}:");
        let mut headed = false;
        for item in owned {
            let line = context_line(item);
            let cost = line.len() + 1 + if headed { 0 } else { heading.len() + 1 };
            if !more.is_empty() || used + cost > CONTEXT_BUDGET_BYTES {
                more.push(&item.title);
                continue;
            }
            if !headed {
                list.push(heading.clone());
                headed = true;
            }
            list.push(line);
            used += cost;
        }
    }
    let mut parts = Vec::new();
    if !list.is_empty() {
        parts.push(fragment(
            "context",
            roots,
            &Vars::from([("list", list.join("\n"))]),
        ));
    }
    if !more.is_empty() {
        parts.push(fragment(
            "context-more",
            roots,
            &Vars::from([("list", more.join(", "))]),
        ));
    }
    if !named.is_empty() {
        parts.push(fragment(
            "context-installed",
            roots,
            &Vars::from([("list", named.join(", "))]),
        ));
    }
    block(&parts.join("\n\n"))
}

/// One listed item: its kind (with its map id), title and absolute path.
fn context_line(item: &okena_core::context::ContextItem) -> String {
    let kind = item.reference.kind;
    let what = match &item.map_id {
        Some(id) => format!("{} `{id}`", kind.label()),
        None => kind.label().to_string(),
    };
    format!("  - {what}: {} (`{}`)", item.title, item.path)
}

/// The arguments that hand `command` its opening `brief`: a reference to a
/// file holding it, in the position the brief itself would take.
///
/// Never the brief itself in argv, where a session backend has to carry it: a
/// long enough brief made tmux refuse the whole session (`command too long`).
/// The terminal resolves the reference as it spawns (`okena_terminal::
/// brief_file`). One file per launch, in okena's profile directory like the
/// `agent-context/` plugins — a respawned session reads it again. Without a
/// profile, or when writing fails, the brief goes in argv as it used to.
pub(super) fn brief_args(command: &str, brief: &str) -> Vec<String> {
    let written = briefs_dir().and_then(|dir| match write_brief(&dir, brief) {
        Ok(path) => Some(path),
        Err(e) => {
            log::warn!(
                "[agents] could not write the brief under {}: {e}",
                dir.display()
            );
            None
        }
    });
    match written {
        Some(path) => {
            super::specs::prompt_args(command, &okena_terminal::brief_file::reference(&path))
        }
        None => super::specs::prompt_args(command, brief),
    }
}

/// Where launch briefs are written.
fn briefs_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(dir) = test_briefs_dir::get() {
        return Some(dir);
    }
    okena_core::profiles::try_current().map(|p| p.root.join("agent-briefs"))
}

fn write_brief(dir: &std::path::Path, brief: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.md", uuid::Uuid::new_v4()));
    std::fs::write(&path, brief)?;
    Ok(path)
}

/// Tests have no profile: this points the current test thread's launches at a
/// directory of its own, so a route can be checked with its brief in a file.
#[cfg(test)]
pub(super) mod test_briefs_dir {
    use std::cell::RefCell;
    use std::path::PathBuf;

    thread_local! {
        static DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    }

    pub fn set(dir: Option<PathBuf>) {
        DIR.with(|d| *d.borrow_mut() = dir);
    }

    pub fn get() -> Option<PathBuf> {
        DIR.with(|d| d.borrow().clone())
    }
}

/// Render one partial against the configured root.
///
/// Where code has to choose between wordings — a store or a folder, who
/// commits, a fan-out or a group — it picks the partial by name here, and the
/// sentence itself stays editable in knowledge.
pub(super) fn fragment(name: &str, roots: &PromptRoots, vars: &Vars<'_>) -> String {
    prompts::fragment(name, &layers(roots), vars).text
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
    let brief = build(flow, &prompt_roots(projects, settings), &owned);
    let source = match &brief.source {
        prompts::Source::Root { key, path } => serde_json::json!({ "root": key, "path": path }),
        prompts::Source::Builtin => serde_json::json!({ "builtin": true }),
    };
    super::ActionResult::Ok(Some(serde_json::json!({
        "flow": flow.id(),
        "text": brief.rendered.text,
        "unknown": brief.rendered.unknown,
        "source": source,
        // What the launcher calls this brief, and the model its template runs
        // the agent on (`model`, `models`).
        "name": brief.name,
        "models": brief.models,
    })))
}

#[cfg(test)]
mod tests {
    use super::{CONTEXT_BUDGET_BYTES, block, context_block, layers_of, project_block};
    use okena_core::context::{ContextItem, ContextKind, ContextOwner, ContextRef};
    use okena_core::knowledge::{KnowledgeRoot, KnowledgeRootKind, KnowledgeStores};

    fn root(key: &str, kind: KnowledgeRootKind, healthy: bool, builtin: bool) -> KnowledgeRoot {
        KnowledgeRoot {
            key: key.into(),
            kind,
            name: key.into(),
            path: format!("/k/{key}"),
            store_id: None,
            description: None,
            remote: None,
            healthy,
            builtin,
            git: None,
            counts: Default::default(),
            used_by: Vec::new(),
            status: Vec::new(),
        }
    }

    #[test]
    fn briefs_resolve_through_every_healthy_root_but_never_okenas_own() {
        use KnowledgeRootKind::{Project, Store};
        // Discovery's order is resolution's order: stores, then projects.
        let stores = KnowledgeStores {
            roots: vec![
                root("store:acme", Store, true, false),
                root("store:okena-defaults", Store, true, true),
                root("store:broken", Store, false, false),
                root("path:/p/web", Project, true, false),
                root("path:/p/gone", Project, false, false),
            ],
            ..Default::default()
        };
        let layers = layers_of(&stores);
        let keys: Vec<&str> = layers.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["store:acme", "path:/p/web"]);

        // The paths come along, since that is what resolution reads from.
        assert_eq!(layers[0].1, std::path::PathBuf::from("/k/store:acme"));
        // Nothing discovered, nothing to layer — the built-ins stand alone.
        assert!(layers_of(&KnowledgeStores::default()).is_empty());
    }

    fn item(owner: &str, kind: ContextKind, n: usize) -> ContextItem {
        let path = format!("/Users/someone/p/{owner}/docs/reference/topic-number-{n}.md");
        ContextItem {
            reference: ContextRef {
                kind,
                owner: ContextOwner::store(format!("store:{owner}")),
                locator: path.clone(),
            },
            title: format!("Topic {n}"),
            description: format!("What topic {n} is about, at some length."),
            owner_name: owner.into(),
            path,
            map_id: (kind == ContextKind::MapEntry).then(|| format!("area:topic-{n}")),
            chosen: false,
        }
    }

    #[test]
    fn a_context_line_is_kind_title_and_path_without_a_description() {
        let items = [
            item("shop", ContextKind::MapEntry, 1),
            item("acme", ContextKind::Doc, 2),
        ];
        let b = context_block(&items, false, &Vec::new());
        assert!(
            b.contains("- shop:\n  - Map entry `area:topic-1`: Topic 1 (`/Users/someone/p/shop/docs/reference/topic-number-1.md`)"),
            "{b}"
        );
        assert!(b.contains("  - Knowledge doc: Topic 2 (`"), "{b}");
        assert!(!b.contains("What topic"), "{b}");
        // Within the budget nothing is left to look up.
        assert!(!b.contains("named here to keep this brief short"), "{b}");
    }

    #[test]
    fn past_the_budget_the_rest_are_named_with_the_lookup_tools() {
        let items: Vec<ContextItem> = (0..120)
            .map(|n| {
                let owner = ["shop", "acme", "web"][n % 3];
                let kind = [ContextKind::MapEntry, ContextKind::Spec, ContextKind::Doc][n % 3];
                item(owner, kind, n)
            })
            .collect();
        let b = context_block(&items, false, &Vec::new());
        let (listed, rest) = b
            .split_once("Also picked, named here to keep this brief short: ")
            .expect("the overflow line");
        let lines: Vec<&str> = listed
            .lines()
            .filter(|l| l.starts_with('-') || l.starts_with("  -"))
            .collect();
        let bytes: usize = lines.iter().map(|l| l.len() + 1).sum();
        assert!(bytes <= CONTEXT_BUDGET_BYTES, "{bytes} bytes listed");
        // Close to the budget, not far under it.
        assert!(bytes > CONTEXT_BUDGET_BYTES - 200, "{bytes} bytes listed");
        // Every item is either listed with its path or named by title, once.
        for it in &items {
            let listed_here = listed.contains(&format!("{} (`{}`)", it.title, it.path));
            let named = rest.contains(&format!("{}, ", it.title))
                || rest.contains(&format!("{}.", it.title));
            assert!(
                listed_here ^ named,
                "{} listed={listed_here} named={named}",
                it.title
            );
        }
        // The rest are a tail: the last item is named, the first listed.
        assert!(rest.contains("Topic 119."), "{rest}");
        assert!(!rest.contains(&items[119].path));
        assert!(
            rest.contains("`okena_context_search`") && rest.contains("`okena_context_read`"),
            "{rest}"
        );
    }

    #[test]
    fn loaded_skills_stay_named_and_do_not_count() {
        let mut items: Vec<ContextItem> =
            (0..60).map(|n| item("acme", ContextKind::Doc, n)).collect();
        items.push(item("acme", ContextKind::Skill, 999));
        let b = context_block(&items, true, &Vec::new());
        assert!(
            b.contains("Loaded into this session: Topic 999 (skill)"),
            "{b}"
        );
        assert!(!b.contains("topic-number-999"), "{b}");
        // Nothing picked, nothing at all.
        assert_eq!(context_block(&[], false, &Vec::new()), "");
    }

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
        assert_eq!(project_block("given-projects", &[], &Vec::new()), "");
    }

    #[test]
    fn projects_are_listed_under_their_label() {
        let given = [
            ("okena".to_string(), "/p/okena".to_string()),
            ("web".to_string(), "/p/web".to_string()),
        ];
        assert_eq!(
            project_block("given-projects", &given, &Vec::new()),
            "\n\nProjects you were given:\n- okena (/p/okena)\n- web (/p/web)"
        );
    }
}
