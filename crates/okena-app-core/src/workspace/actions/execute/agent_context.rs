//! Handing an agent the skills and subagents picked for its launch.
//!
//! The per-agent table beside `agent_mcp`'s. An agent whose CLI can load
//! skills and subagents for one session gets them that way; any other gets
//! their paths in the brief instead (`briefs::context_block`). Like
//! `agent_mcp`, everything is written into okena's profile directory — never
//! into the checkout, and never into the agent's own configuration, which
//! would hand them to every session the user runs.

use crate::workspace::persistence::AppSettings;
use okena_core::context::{ContextItem, ContextKind};
use serde_json::json;
use std::collections::HashSet;
use std::path::Path;

/// The plugin a session's skills and agents are loaded as.
const PLUGIN_NAME: &str = "okena-context";
/// Most files copied for one skill, like the knowledge tree's cap.
const MAX_SKILL_FILES: usize = 200;

/// How an agent CLI takes skills and subagents for one session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Delivery {
    /// A plugin directory: `.claude-plugin/plugin.json`, `skills/<name>/`,
    /// `agents/<name>.md`.
    PluginDir,
}

/// Only agents whose mechanism is known are listed. The ticket settles codex
/// and copilot on paths in the brief.
fn delivery(agent: &str) -> Option<Delivery> {
    match agent {
        // Verified against `claude --help` (`--plugin-dir <path>`, "Load a
        // plugin from a directory") and `claude plugin validate`.
        "claude" => Some(Delivery::PluginDir),
        _ => None,
    }
}

/// The agent a launch runs: the override, else the configured one. Empty
/// means none.
pub(super) fn launch_command(settings: &AppSettings, override_command: Option<&str>) -> String {
    match override_command {
        Some(c) => c,
        None => settings.harness.agent_command.as_deref().unwrap_or(""),
    }
    .trim()
    .to_string()
}

/// What was installed for a launch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Install {
    /// Arguments that load it. Empty when nothing was.
    pub args: Vec<String>,
}

impl Install {
    /// The skills and agents are in the session itself, so the brief names
    /// them rather than listing their paths.
    pub fn loaded(&self) -> bool {
        !self.args.is_empty()
    }
}

/// Install `items`' skills and agents for `command`, where its CLI can take
/// them. Nothing when there are none, the agent has no mechanism, or writing
/// failed — the brief then lists their paths, so nothing is lost.
pub(super) fn install(command: &str, items: &[ContextItem]) -> Install {
    let base = okena_core::profiles::try_current().map(|p| p.root.join("agent-context"));
    install_in(base.as_deref(), command, items)
}

/// [`install`], writing under `base`.
fn install_in(base: Option<&Path>, command: &str, items: &[ContextItem]) -> Install {
    let installable: Vec<&ContextItem> = items
        .iter()
        .filter(|i| i.reference.kind.is_installable())
        .collect();
    if installable.is_empty() || command.is_empty() {
        return Install::default();
    }
    match delivery(&okena_core::agents::command_name(command)) {
        Some(Delivery::PluginDir) => {
            let Some(base) = base else {
                return Install::default();
            };
            // One per launch: a restart resumes with the same arguments, so
            // the directory must outlive any later launch's.
            let dir = base.join(uuid::Uuid::new_v4().to_string());
            match write_plugin(&dir, &installable) {
                Ok(()) => Install {
                    args: vec!["--plugin-dir".into(), dir.to_string_lossy().into_owned()],
                },
                Err(e) => {
                    log::warn!("[agents] could not install context skills: {e}");
                    Install::default()
                }
            }
        }
        None => Install::default(),
    }
}

/// Write a plugin holding copies of the skills and agents in `items`.
///
/// Copies, not links: a store that is pulled or edited while the session runs
/// must not change what the session loaded, and a link to a directory does
/// not survive every platform.
fn write_plugin(dir: &Path, items: &[&ContextItem]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir.join(".claude-plugin"))?;
    let manifest = json!({
        "name": PLUGIN_NAME,
        "version": "1.0.0",
        "description": "Skills and agents okena handed this session",
    });
    std::fs::write(
        dir.join(".claude-plugin").join("plugin.json"),
        serde_json::to_string_pretty(&manifest).map_err(std::io::Error::other)?,
    )?;
    let mut taken: HashSet<String> = HashSet::new();
    for item in items {
        let source = Path::new(&item.path);
        match item.reference.kind {
            ContextKind::Skill => {
                let Some(skill_dir) = source.parent() else {
                    continue;
                };
                let name = unique_name(&mut taken, "skills", &file_name(skill_dir));
                let mut budget = MAX_SKILL_FILES;
                copy_dir(skill_dir, &dir.join("skills").join(name), &mut budget)?;
            }
            ContextKind::Agent => {
                let stem = source
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "agent".into());
                let name = unique_name(&mut taken, "agents", &stem);
                std::fs::create_dir_all(dir.join("agents"))?;
                std::fs::copy(source, dir.join("agents").join(format!("{name}.md")))?;
            }
            ContextKind::MapEntry | ContextKind::Spec | ContextKind::Doc => {}
        }
    }
    Ok(())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "skill".into())
}

/// `name`, or `name-2`, `name-3`… when two stores ship one of that name.
fn unique_name(taken: &mut HashSet<String>, folder: &str, name: &str) -> String {
    let mut candidate = name.to_string();
    let mut n = 2;
    while !taken.insert(format!("{folder}/{candidate}")) {
        candidate = format!("{name}-{n}");
        n += 1;
    }
    candidate
}

/// Copy a skill's directory: regular files only, no dot-entries, no links.
fn copy_dir(from: &Path, to: &Path, budget: &mut usize) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    let mut entries: Vec<_> = std::fs::read_dir(from)?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_dir(&entry.path(), &to.join(entry.file_name()), budget)?;
        } else if kind.is_file() {
            if *budget == 0 {
                return Ok(());
            }
            *budget -= 1;
            std::fs::copy(entry.path(), to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_core::context::{ContextOwner, ContextRef};

    pub(crate) fn item(kind: ContextKind, path: &Path, title: &str) -> ContextItem {
        ContextItem {
            reference: ContextRef {
                kind,
                owner: ContextOwner::store("store:acme"),
                locator: path.to_string_lossy().into_owned(),
            },
            title: title.into(),
            description: String::new(),
            owner_name: "acme".into(),
            path: path.to_string_lossy().into_owned(),
            map_id: None,
            chosen: false,
        }
    }

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn only_claude_loads_skills_itself() {
        assert!(delivery("claude").is_some());
        assert!(delivery(&okena_core::agents::command_name("/opt/bin/claude")).is_some());
        assert!(delivery("codex").is_none());
        assert!(delivery("copilot").is_none());
        assert!(delivery("").is_none());
    }

    #[test]
    fn nothing_is_installed_without_a_skill_or_an_agent() {
        let dir = tempfile::tempdir().unwrap();
        let doc = dir.path().join("docs/x.md");
        write(&doc, "# x");
        assert!(!install("claude", &[item(ContextKind::Doc, &doc, "x")]).loaded());
        let skill = dir.path().join("skills/release/SKILL.md");
        write(&skill, "# r");
        // An agent with no mechanism gets nothing here; its brief lists paths.
        assert!(!install("codex", &[item(ContextKind::Skill, &skill, "r")]).loaded());
    }

    #[test]
    fn a_plugin_holds_each_skill_directory_and_agent() {
        let src = tempfile::tempdir().unwrap();
        let skill = src.path().join("acme/skills/release/SKILL.md");
        write(&skill, "---\nname: release\n---\n# Release\n");
        write(
            &src.path().join("acme/skills/release/scripts/tag.sh"),
            "git tag",
        );
        write(&src.path().join("acme/skills/release/.cache"), "skip");
        // Another store's skill of the same name.
        let other = src.path().join("beta/skills/release/SKILL.md");
        write(&other, "# Other release\n");
        let agent = src.path().join("acme/agents/reviewer.md");
        write(&agent, "---\nname: reviewer\n---\nYou review.\n");

        let out = tempfile::tempdir().unwrap();
        let plugin = out.path().join("p");
        write_plugin(
            &plugin,
            &[
                &item(ContextKind::Skill, &skill, "Release"),
                &item(ContextKind::Skill, &other, "Other release"),
                &item(ContextKind::Agent, &agent, "reviewer"),
            ],
        )
        .unwrap();

        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(plugin.join(".claude-plugin/plugin.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["name"], PLUGIN_NAME);
        assert!(plugin.join("skills/release/SKILL.md").is_file());
        assert!(plugin.join("skills/release/scripts/tag.sh").is_file());
        assert!(!plugin.join("skills/release/.cache").exists());
        assert_eq!(
            std::fs::read_to_string(plugin.join("skills/release-2/SKILL.md")).unwrap(),
            "# Other release\n"
        );
        assert!(plugin.join("agents/reviewer.md").is_file());
    }

    /// A project's map entry, a store's doc and a store's skill.
    fn picked(root: &Path) -> Vec<ContextItem> {
        let doc = root.join("acme/docs/principles.md");
        write(&doc, "# Principles");
        let skill = root.join("acme/skills/release/SKILL.md");
        write(&skill, "# Release");
        let map = root.join("shop/.okena/knowledge/docs/project/checkout.md");
        write(&map, "# Checkout");
        let mut area = item(ContextKind::MapEntry, &map, "Checkout");
        area.owner_name = "shop".into();
        area.reference.owner = ContextOwner::project("p-shop");
        area.map_id = Some("area:checkout".into());
        area.description = "Takes payment for a basket.".into();
        let mut principles = item(ContextKind::Doc, &doc, "Engineering principles");
        principles.description = "How we build things.".into();
        vec![
            area,
            principles,
            item(ContextKind::Skill, &skill, "Release"),
        ]
    }

    fn args_of(shell: okena_terminal::shell_config::ShellType) -> Vec<String> {
        match shell {
            okena_terminal::shell_config::ShellType::Custom { args, .. } => args,
            other => panic!("not a custom shell: {other:?}"),
        }
    }

    #[test]
    fn a_started_sessions_brief_names_every_picked_item_by_owner() {
        let dir = tempfile::tempdir().unwrap();
        let items = picked(dir.path());
        let brief = super::super::tasks::custom_brief("Fix checkout", &[], &items, false, None);
        let shop = brief.find("- shop:").expect("grouped under the project");
        let acme = brief.find("- acme:").expect("grouped under the store");
        assert!(shop < acme, "{brief}");
        assert!(
            brief.contains(&format!(
                "Map entry `area:checkout`: Checkout (`{}`)",
                items[0].path
            )),
            "{brief}"
        );
        assert!(
            brief.contains(&format!(
                "Knowledge doc: Engineering principles (`{}`)",
                items[1].path
            )),
            "{brief}"
        );
        // Descriptions are left to the lookup tools.
        assert!(!brief.contains("Takes payment for a basket."), "{brief}");
        // Nothing loaded it, so the skill is listed where it is.
        assert!(brief.contains(&format!("Skill: Release (`{}`)", items[2].path)));
        // Still the goal first and the reporting rule last.
        assert!(brief.starts_with("Fix checkout"));
        assert!(
            brief
                .trim_end()
                .ends_with("Report `working` again when you carry on.")
        );
    }

    #[test]
    fn a_claude_launch_loads_the_skills_and_its_brief_names_them() {
        let dir = tempfile::tempdir().unwrap();
        let items = picked(dir.path());
        let base = tempfile::tempdir().unwrap();
        let install = install_in(Some(base.path()), "claude", &items);
        assert!(install.loaded());
        assert_eq!(install.args[0], "--plugin-dir");
        let plugin = Path::new(&install.args[1]);
        assert!(plugin.starts_with(base.path()));
        assert!(plugin.join("skills/release/SKILL.md").is_file());
        assert!(plugin.join(".claude-plugin/plugin.json").is_file());

        let brief =
            super::super::tasks::custom_brief("Ship it", &[], &items, install.loaded(), None);
        assert!(
            brief.contains("Loaded into this session: Release (skill)"),
            "{brief}"
        );
        assert!(!brief.contains(&items[2].path), "{brief}");
        // The doc and map entry are still paths to read.
        assert!(brief.contains(&items[1].path));

        let settings = AppSettings::default();
        let args = args_of(
            super::super::tasks::custom_agent_shell(&settings, Some("claude"), &brief, &install)
                .expect("an agent"),
        );
        let at = args
            .iter()
            .position(|a| a == "--plugin-dir")
            .expect("plugin dir");
        assert_eq!(args[at + 1], install.args[1]);
    }

    #[test]
    fn a_codex_launch_gets_the_skill_path_in_its_brief() {
        let dir = tempfile::tempdir().unwrap();
        let items = picked(dir.path());
        let base = tempfile::tempdir().unwrap();
        let install = install_in(Some(base.path()), "codex", &items);
        assert!(!install.loaded());
        assert_eq!(std::fs::read_dir(base.path()).unwrap().count(), 0);
        let brief = super::super::tasks::custom_brief("Ship it", &[], &items, false, None);
        assert!(brief.contains(&items[2].path), "{brief}");
        let settings = AppSettings::default();
        let args = args_of(
            super::super::tasks::custom_agent_shell(&settings, Some("codex"), &brief, &install)
                .expect("an agent"),
        );
        assert!(!args.iter().any(|a| a == "--plugin-dir"));
        assert!(args.iter().any(|a| a.contains("skills/release/SKILL.md")));
    }

    #[test]
    fn a_stores_context_partial_changes_the_next_brief() {
        let dir = tempfile::tempdir().unwrap();
        let items = picked(dir.path());
        let store = tempfile::tempdir().unwrap();
        write(
            &store.path().join("templates/partials/context.md"),
            "Acme house rule — read these first:\n{list}",
        );
        let root = Some(("store:acme".to_string(), store.path().to_path_buf()));
        let brief = super::super::tasks::custom_brief("Ship it", &[], &items, false, root);
        assert!(
            brief.contains("Acme house rule — read these first:\n- shop:"),
            "{brief}"
        );
        assert!(!brief.contains("Nothing here is inlined"), "{brief}");
        // Without the store, okena's own words.
        let plain = super::super::tasks::custom_brief("Ship it", &[], &items, false, None);
        assert!(plain.contains("Nothing here is inlined"));
        // No items, no block at all: a launch without context is unchanged.
        let none = super::super::tasks::custom_brief("Ship it", &[], &[], false, None);
        assert!(!none.contains("Context picked"));
    }
}
