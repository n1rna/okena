//! Project maps: the Scan action and the map read (ADR-0005).
//!
//! An agent writes the map. okena decides where it goes and what the agent
//! starts from (`okena_knowledge::project_scan`), points it at the
//! `project-map` skill resolved the way templates are, and opens the session.

use super::ActionResult;
use super::briefs::{self, PromptRoots};
use crate::workspace::persistence::AppSettings;
use crate::workspace::state::ProjectData;
use okena_knowledge::project_scan::{self, ScanPlan, ScanStart};
use okena_knowledge::prompts::{self, Flow, Source, Vars, defaults};
use std::path::{Path, PathBuf};

/// Handle [`okena_core::api::ActionRequest::ProjectMapRead`] for the project
/// at `path`.
///
/// Takes the path rather than the workspace so the daemon can run it off the
/// workspace lock, like the knowledge reads.
pub fn read_project_map(path: Option<&str>, project_id: &str) -> ActionResult {
    let Some(path) = path else {
        return ActionResult::Err(format!("project not found: {project_id}"));
    };
    let repo = okena_core::fs::expand_home(path);
    let report = okena_knowledge::project_map::report_for_repo(&repo);
    ActionResult::Ok(Some(
        serde_json::to_value(report).expect("BUG: a project map report must serialize"),
    ))
}

/// Open an agent session in a repository, briefed to write or update its map.
#[allow(clippy::too_many_arguments)]
pub(super) fn scan(
    ws: &mut crate::workspace::state::Workspace,
    window_id: crate::workspace::state::WindowId,
    project_id: String,
    agent_command: Option<String>,
    model: Option<String>,
    backend: &dyn okena_terminal::backend::TerminalBackend,
    terminals: &okena_terminal::TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl okena_workspace::context::WorkspaceCx,
) -> ActionResult {
    let Some(project) = ws.project(&project_id) else {
        return ActionResult::Err(format!("project not found: {project_id}"));
    };
    if let Err(e) = scannable(project) {
        return ActionResult::Err(e);
    }
    let name = project.name.clone();
    let repo = okena_core::fs::expand_home(&project.path);
    if !repo.is_dir() {
        return ActionResult::Err(format!("`{name}` is not on disk at {}", repo.display()));
    }
    let plan = match project_scan::plan(&repo) {
        Ok(plan) => plan,
        Err(e) => return ActionResult::Err(e.to_string()),
    };
    let prompts = briefs::prompt_roots(&ws.data.projects, settings);
    let skill = match skill_file(&prompts, &super::knowledge::defaults_store()) {
        Ok(path) => path,
        Err(e) => return ActionResult::Err(e),
    };
    let repo_path = repo.to_string_lossy().into_owned();
    let brief = scan_brief(&name, &repo_path, &plan, &skill, &prompts);
    let Some(shell) = super::specs::spec_agent_shell(
        settings,
        agent_command.as_deref(),
        &brief,
        // A scan is handed no picked context.
        &Default::default(),
        &briefs::launch_model(Flow::ProjectScan, &prompts, model),
    ) else {
        // Nothing is scaffolded, so a session without an agent has no purpose.
        return ActionResult::Err(
            "no agent to start — pick one, or set the agent command in Settings → Harness".into(),
        );
    };

    let session_name = format!("{name} (map)");
    let session_id = match ws.add_project(
        session_name.clone(),
        repo_path,
        true,
        &settings.hooks,
        window_id,
        cx,
    ) {
        Ok(id) => id,
        Err(e) => return ActionResult::Err(format!("could not open a session in `{name}`: {e}")),
    };
    // Marked and given its agent before the terminal spawns: the terminal
    // reads the project's shell as it starts, and the marker keeps the session
    // out of project and knowledge discovery from the first snapshot.
    if let Some(p) = ws.data.projects.iter_mut().find(|p| p.id == session_id) {
        p.custom_session = Some(format!("Map {name}"));
        p.project_scan = Some(name.clone());
        p.default_shell = Some(shell);
    }
    if let ActionResult::Err(e) =
        super::spawn_session_terminals(ws, &session_id, backend, terminals, settings, cx)
    {
        log::warn!("[project-map] scan session terminal failed to spawn: {e}");
    }
    ws.notify_data(cx);
    ActionResult::Ok(Some(serde_json::json!({
        "project_id": session_id,
        "name": session_name,
        "start": plan.start.id(),
        "map_root": plan.map_root.to_string_lossy(),
    })))
}

/// Only a repository is mapped: a worktree is a checkout of one, and the map
/// is committed in the repository.
pub(super) fn scannable(project: &ProjectData) -> Result<(), String> {
    if project.worktree_info.is_some() {
        return Err(format!(
            "`{}` is a worktree — scan the repository it was created from, where the map is committed",
            project.name
        ));
    }
    if project.is_any_agent_session() {
        return Err(format!(
            "`{}` is an agent session, not a repository",
            project.name
        ));
    }
    Ok(())
}

/// The `project-map` SKILL.md the agent is pointed at: the first root that
/// overrides it, else okena's own in the defaults store at `defaults_dir`.
///
/// A path rather than the text inlined, so the agent reads the skill as the
/// file it is, and a team can open the exact file its scans follow. Resolution
/// names the winning root by key, so the directory is looked up again here —
/// the layer that answered is not necessarily the first one.
pub(super) fn skill_file(prompts: &PromptRoots, defaults_dir: &Path) -> Result<PathBuf, String> {
    let layers = briefs::layers(prompts);
    if let Some(Source::Root { key, path }) =
        prompts::skill(defaults::PROJECT_MAP_SKILL, &layers).map(|s| s.source)
        && let Some((_, dir)) = layers.iter().find(|(k, _)| *k == key)
    {
        return Ok(dir.join(path));
    }
    let file = defaults_dir.join(defaults::skill_path(defaults::PROJECT_MAP_SKILL));
    if file.is_file() {
        Ok(file)
    } else {
        Err(format!(
            "okena's project-map skill is not on disk at {} — check that okena's config folder is writable",
            file.display()
        ))
    }
}

fn scan_brief(
    project: &str,
    path: &str,
    plan: &ScanPlan,
    skill: &Path,
    prompts: &PromptRoots,
) -> String {
    let mut vars = Vars::new();
    vars.insert("project", project.to_string());
    vars.insert("path", path.to_string());
    vars.insert("map_root", plan.map_root.to_string_lossy().into_owned());
    vars.insert("skill", skill.to_string_lossy().into_owned());
    vars.insert("start", start_note(plan, prompts));
    briefs::build(Flow::ProjectScan, prompts, &vars)
        .rendered
        .text
}

/// What the agent starts from. The decision is `plan.start`; the words are
/// the `scan-*` partial it names.
fn start_note(plan: &ScanPlan, prompts: &PromptRoots) -> String {
    let list = |items: &[String]| {
        items
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut vars = Vars::new();
    vars.insert("manifest", plan.manifest.to_string_lossy().into_owned());
    match &plan.start {
        ScanStart::Repair { problems } => {
            vars.insert("list", list(problems));
        }
        ScanStart::FromDocs { docs } => {
            vars.insert("list", list(docs));
        }
        ScanStart::Update | ScanStart::FromCode => {}
    }
    briefs::block(&briefs::fragment(
        plan.start.partial(),
        prompts,
        &vars,
    ))
}

#[cfg(test)]
mod tests {
    use super::{read_project_map, scan_brief, scannable, skill_file};
    use crate::workspace::actions::execute::ActionResult;
    use crate::workspace::state::ProjectData;
    use okena_knowledge::project_scan::{ScanPlan, ScanStart};
    use std::path::{Path, PathBuf};

    fn tmpdir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "okena-project-scan-{tag}-{}-{}",
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

    fn plan(start: ScanStart) -> ScanPlan {
        ScanPlan {
            map_root: PathBuf::from("/p/api/.okena/knowledge"),
            manifest: PathBuf::from("/p/api/.okena/knowledge/project-map.yaml"),
            start,
        }
    }

    fn brief(start: ScanStart) -> String {
        scan_brief(
            "api",
            "/p/api",
            &plan(start),
            Path::new("/cfg/knowledge/okena-defaults/skills/project-map/SKILL.md"),
            &Vec::new(),
        )
    }

    #[test]
    fn the_brief_names_the_skill_the_project_and_where_the_map_goes() {
        let text = brief(ScanStart::FromCode);
        for needle in [
            "`/cfg/knowledge/okena-defaults/skills/project-map/SKILL.md`",
            "api at `/p/api`",
            "`/p/api/.okena/knowledge`",
            "project-map.yaml",
            "Do not commit or push",
            "okena_report_status",
        ] {
            assert!(
                text.contains(needle),
                "brief is missing {needle:?}:\n{text}"
            );
        }
        // Not any `{`: the shared reporting partial quotes JSON. Only this
        // flow's own values, and the partial's, must all be filled.
        for placeholder in [
            "{project}",
            "{path}",
            "{map_root}",
            "{skill}",
            "{start}",
            "{manifest}",
            "{list}",
        ] {
            assert!(
                !text.contains(placeholder),
                "unfilled {placeholder}:\n{text}"
            );
        }
    }

    #[test]
    fn the_brief_says_which_starting_point_applies() {
        let update = brief(ScanStart::Update);
        assert!(
            update.contains("Bring it up to date rather than rebuilding it"),
            "{update}"
        );
        assert!(
            update.contains("`/p/api/.okena/knowledge/project-map.yaml`"),
            "{update}"
        );

        let repair = brief(ScanStart::Repair {
            problems: vec!["`areas[x].id` is `X`, which is not kebab-case.".into()],
        });
        assert!(repair.contains("does not pass okena's checks"), "{repair}");
        assert!(repair.contains("- `areas[x].id` is `X`"), "{repair}");

        let docs = brief(ScanStart::FromDocs {
            docs: vec!["CLAUDE.md".into(), "docs/".into()],
        });
        assert!(
            docs.contains("describes itself in:\n- CLAUDE.md\n- docs/"),
            "{docs}"
        );

        let code = brief(ScanStart::FromCode);
        assert!(code.contains("Work it out from the code"), "{code}");
        for other in [&update, &repair, &docs] {
            assert!(!other.contains("Work it out from the code"), "{other}");
        }
    }

    fn project(json: serde_json::Value) -> ProjectData {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn only_a_repository_can_be_scanned() {
        let repo = project(serde_json::json!({ "id": "r", "name": "api", "path": "/p/api" }));
        assert!(scannable(&repo).is_ok());

        let worktree = project(serde_json::json!({
            "id": "w", "name": "api (QBL-1)", "path": "/p/wt",
            "worktree_info": {
                "parent_project_id": "r",
                "main_repo_path": "/p/api",
                "worktree_path": "/p/wt",
                "branch_name": "feat/x",
            },
        }));
        assert!(scannable(&worktree).is_err_and(|e| e.contains("worktree")));

        let session = project(serde_json::json!({
            "id": "s", "name": "goal (agent)", "path": "/p", "custom_session": "goal",
        }));
        assert!(scannable(&session).is_err_and(|e| e.contains("agent session")));
    }

    #[test]
    fn the_skill_is_the_prompt_roots_copy_else_okenas() {
        let defaults = tmpdir("defaults");
        let builtin = defaults.join("skills/project-map/SKILL.md");
        let none = Vec::new();
        assert!(
            skill_file(&none, &defaults).is_err_and(|e| e.contains("not on disk")),
            "a missing defaults copy is said, not pointed at"
        );
        write(&builtin, "---\nname: project-map\n---\n");
        assert_eq!(skill_file(&none, &defaults), Ok(builtin.clone()));

        let store = tmpdir("store");
        let project = tmpdir("project");
        let prompts = vec![
            ("store:acme-eng".to_string(), store.clone()),
            ("path:/p/web".to_string(), project.clone()),
        ];
        assert_eq!(
            skill_file(&prompts, &defaults),
            Ok(builtin),
            "roots without their own copy fall back"
        );
        // The second layer answers, so the path must be its copy, not the
        // first root's — the bug a key-only `Source` would hide.
        let theirs = project.join("skills/project-map/SKILL.md");
        write(&theirs, "---\nname: project-map\n---\nTheir way.\n");
        assert_eq!(skill_file(&prompts, &defaults), Ok(theirs));
        // The store is listed first, so its copy takes over.
        let ours = store.join("skills/project-map/SKILL.md");
        write(&ours, "---\nname: project-map\n---\nOur way.\n");
        assert_eq!(skill_file(&prompts, &defaults), Ok(ours));
    }

    #[test]
    fn a_map_read_reports_the_state_of_the_repository_at_the_path() {
        let repo = tmpdir("read");
        let state = |r: ActionResult| match r {
            ActionResult::Ok(Some(v)) => v["state"].as_str().unwrap().to_string(),
            _ => panic!("expected a state"),
        };
        assert_eq!(state(read_project_map(repo.to_str(), "p")), "not_scanned");
        write(
            &repo.join(".okena/knowledge/project-map.yaml"),
            "version: 1\nproject: [\n",
        );
        assert_eq!(state(read_project_map(repo.to_str(), "p")), "invalid");
        match read_project_map(repo.to_str(), "p") {
            ActionResult::Ok(Some(v)) => assert_eq!(
                v["root_key"],
                format!("path:{}", repo.join(".okena/knowledge").display())
            ),
            _ => panic!("expected a report"),
        }
        assert!(matches!(
            read_project_map(None, "gone"),
            ActionResult::Err(e) if e.contains("gone")
        ));
    }
}
