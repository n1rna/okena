//! Links between projects: the read, and the multi-project scan (ADR-0006).
//!
//! The matching is `okena_knowledge::project_links`; this module finds the
//! projects to read, and opens the session that looks for links the maps do
//! not show yet.

use super::ActionResult;
use super::briefs::{self, PromptRoots};
use crate::workspace::persistence::AppSettings;
use crate::workspace::state::ProjectData;
use okena_core::project_map::ProjectMapState;
use okena_knowledge::project_links::{MapInput, match_links};
use okena_knowledge::project_scan::{self, ScanPlan, ScanStart};
use okena_knowledge::prompts::{Flow, Vars};
use std::path::Path;

/// A project that can have a map, copied out of the workspace so the daemon
/// can read maps off its lock.
#[derive(Clone, Debug)]
pub struct MapSource {
    pub id: String,
    pub name: String,
    pub path: String,
}

/// The projects links are looked for among: repositories, never worktrees (a
/// second checkout of one) or agent sessions.
pub fn map_sources(projects: &[ProjectData]) -> Vec<MapSource> {
    projects
        .iter()
        .filter(|p| p.worktree_info.is_none() && !p.is_any_agent_session())
        .map(|p| MapSource {
            id: p.id.clone(),
            name: p.name.clone(),
            path: p.path.clone(),
        })
        .collect()
}

/// Handle [`okena_core::api::ActionRequest::ProjectLinks`]: read every map and
/// match links across them.
pub fn read_project_links(sources: &[MapSource]) -> ActionResult {
    let states: Vec<ProjectMapState> = sources
        .iter()
        .map(|s| okena_knowledge::project_map::load_for_repo(&okena_core::fs::expand_home(&s.path)))
        .collect();
    let inputs: Vec<MapInput<'_>> = sources
        .iter()
        .zip(&states)
        .map(|(source, state)| MapInput {
            project_id: &source.id,
            name: &source.name,
            state,
        })
        .collect();
    ActionResult::Ok(Some(
        serde_json::to_value(match_links(&inputs)).expect("BUG: project links must serialize"),
    ))
}

/// Open one agent session over several repositories, briefed to find the
/// links between them and write each into both maps.
#[allow(clippy::too_many_arguments)]
pub(super) fn scan_links(
    ws: &mut crate::workspace::state::Workspace,
    window_id: crate::workspace::state::WindowId,
    project_ids: Vec<String>,
    agent_command: Option<String>,
    backend: &dyn okena_terminal::backend::TerminalBackend,
    terminals: &okena_terminal::TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl okena_workspace::context::WorkspaceCx,
) -> ActionResult {
    if project_ids.len() < 2 {
        return ActionResult::Err(
            "pick at least two repositories to look for links between".into(),
        );
    }
    let mut listed: Vec<(String, String, ScanPlan)> = Vec::new();
    for id in &project_ids {
        let Some(project) = ws.project(id) else {
            return ActionResult::Err(format!("project not found: {id}"));
        };
        if let Err(e) = super::project_scan::scannable(project) {
            return ActionResult::Err(e);
        }
        let repo = okena_core::fs::expand_home(&project.path);
        if !repo.is_dir() {
            return ActionResult::Err(format!(
                "`{}` is not on disk at {}",
                project.name,
                repo.display()
            ));
        }
        let plan = match project_scan::plan(&repo) {
            Ok(plan) => plan,
            Err(e) => return ActionResult::Err(format!("`{}`: {e}", project.name)),
        };
        listed.push((
            project.name.clone(),
            repo.to_string_lossy().into_owned(),
            plan,
        ));
    }

    let context: Vec<(String, String)> = listed
        .iter()
        .map(|(name, path, _)| (name.clone(), path.clone()))
        .collect();
    let Some(root) = super::tasks::resolve_session_root("", &context, settings) else {
        return ActionResult::Err(
            "no folder to run in — set a projects root in Settings → Harness".into(),
        );
    };
    let prompts = briefs::prompt_roots(&ws.data.projects, settings);
    let skill = match super::project_scan::skill_file(&prompts, &super::knowledge::defaults_store())
    {
        Ok(path) => path,
        Err(e) => return ActionResult::Err(e),
    };
    let brief = links_brief(&listed, &skill, &prompts);
    let Some(shell) = super::specs::spec_agent_shell(
        settings,
        agent_command.as_deref(),
        &brief,
        // A scan is handed no picked context.
        &Default::default(),
    ) else {
        return ActionResult::Err(
            "no agent to start — pick one, or set the agent command in Settings → Harness".into(),
        );
    };

    let names = listed
        .iter()
        .map(|(name, _, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let session_name = format!("{} projects (links)", listed.len());
    let session_id = match ws.add_project(
        session_name.clone(),
        root.clone(),
        true,
        &settings.hooks,
        window_id,
        cx,
    ) {
        Ok(id) => id,
        Err(e) => return ActionResult::Err(format!("could not open the session: {e}")),
    };
    if let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == session_id) {
        p.custom_session = Some(format!("Link {names}"));
        p.project_scan = Some(names.clone());
        p.default_shell = Some(shell);
    }
    if let ActionResult::Err(e) = super::spawn_uninitialized_terminals(
        ws,
        &session_id,
        backend,
        terminals,
        settings,
        None,
        cx,
    ) {
        log::warn!("[project-links] links session terminal failed to spawn: {e}");
    }
    ws.notify_data(cx);
    ActionResult::Ok(Some(serde_json::json!({
        "project_id": session_id,
        "name": session_name,
        "root": root,
    })))
}

fn links_brief(
    listed: &[(String, String, ScanPlan)],
    skill: &Path,
    prompts: &PromptRoots,
) -> String {
    let list = listed
        .iter()
        .map(|(name, path, plan)| {
            format!(
                "- {name} ({path}), map `{}`: {}",
                plan.manifest.display(),
                map_state(&plan.start)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut vars = Vars::new();
    vars.insert(
        "projects",
        briefs::block(&briefs::fragment(
            "given-projects",
            prompts,
            &Vars::from([("list", list)]),
        )),
    );
    vars.insert("skill", skill.to_string_lossy().into_owned());
    briefs::build(Flow::ProjectsScan, prompts, &vars)
        .rendered
        .text
}

/// A repository's map, in the words the list uses.
fn map_state(start: &ScanStart) -> &'static str {
    match start {
        ScanStart::Update => "mapped",
        ScanStart::Repair { .. } => "invalid",
        ScanStart::FromDocs { .. } | ScanStart::FromCode => "not mapped yet",
    }
}

#[cfg(test)]
mod tests {
    use super::{MapSource, links_brief, map_sources, read_project_links};
    use crate::workspace::actions::execute::ActionResult;
    use crate::workspace::state::ProjectData;
    use okena_knowledge::project_scan::{ScanPlan, ScanStart};
    use std::path::{Path, PathBuf};

    fn tmpdir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "okena-project-links-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn plan(repo: &str, start: ScanStart) -> ScanPlan {
        ScanPlan {
            map_root: PathBuf::from(format!("{repo}/.okena/knowledge")),
            manifest: PathBuf::from(format!("{repo}/.okena/knowledge/project-map.yaml")),
            start,
        }
    }

    #[test]
    fn the_brief_names_every_repository_its_map_and_the_both_sides_rule() {
        let listed = vec![
            (
                "api".to_string(),
                "/p/api".to_string(),
                plan("/p/api", ScanStart::Update),
            ),
            (
                "worker".to_string(),
                "/p/worker".to_string(),
                plan("/p/worker", ScanStart::FromCode),
            ),
        ];
        let text = links_brief(
            &listed,
            Path::new("/cfg/skills/project-map/SKILL.md"),
            &Vec::new(),
        );
        for needle in [
            "- api (/p/api), map `/p/api/.okena/knowledge/project-map.yaml`: mapped",
            "- worker (/p/worker), map `/p/worker/.okena/knowledge/project-map.yaml`: not mapped yet",
            "`/cfg/skills/project-map/SKILL.md`",
            "in both maps",
            "`direction: uses`",
            "`direction: used_by`",
            "Do not commit or push",
        ] {
            assert!(
                text.contains(needle),
                "brief is missing {needle:?}:\n{text}"
            );
        }
        for placeholder in ["{projects}", "{skill}", "{list}"] {
            assert!(
                !text.contains(placeholder),
                "unfilled {placeholder}:\n{text}"
            );
        }
    }

    #[test]
    fn links_are_looked_for_among_repositories_only() {
        let projects: Vec<ProjectData> = [
            serde_json::json!({ "id": "r", "name": "api", "path": "/p/api" }),
            serde_json::json!({
                "id": "w", "name": "api (QBL-1)", "path": "/p/wt",
                "worktree_info": {
                    "parent_project_id": "r", "main_repo_path": "/p/api",
                    "worktree_path": "/p/wt", "branch_name": "feat/x",
                },
            }),
            serde_json::json!({ "id": "s", "name": "goal", "path": "/p", "custom_session": "goal" }),
        ]
        .into_iter()
        .map(|v| serde_json::from_value(v).unwrap())
        .collect();
        let ids: Vec<String> = map_sources(&projects).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["r"]);
    }

    #[test]
    fn the_read_matches_links_between_the_repositories_on_disk() {
        let base = tmpdir("read");
        write(
            &base.join("billing/.okena/knowledge/project-map.yaml"),
            "version: 1\nproject:\n  name: billing\n  description: d\nexposes:\n  - type: topic\n    name: invoice-issued\n",
        );
        write(
            &base.join("worker/.okena/knowledge/project-map.yaml"),
            "version: 1\nproject:\n  name: worker\n  description: d\nconsumes:\n  - type: topic\n    name: invoice-issued\n",
        );
        std::fs::create_dir_all(base.join("docs-only")).unwrap();
        let source = |id: &str| MapSource {
            id: id.to_string(),
            name: id.to_string(),
            path: base.join(id).to_string_lossy().into_owned(),
        };
        let result =
            read_project_links(&[source("billing"), source("worker"), source("docs-only")]);
        let ActionResult::Ok(Some(v)) = result else {
            panic!("expected links");
        };
        assert_eq!(v["links"][0]["consumer"], "worker");
        assert_eq!(v["links"][0]["provider"], "billing");
        assert_eq!(v["links"][0]["source"], "matched");
        assert_eq!(v["projects"][2]["status"], "not_scanned");
    }
}
