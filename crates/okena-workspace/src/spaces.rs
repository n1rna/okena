//! Creating, renaming, deleting and switching spaces.
//!
//! A space's identity and per-space config live in `settings.json`
//! ([`AppSettings::spaces`]); what belongs to it lives in `workspace.json`
//! (each project's `space_id`). Both files are the daemon's, so the rules that
//! span them live here, GPUI-free and pure: every function takes the two
//! pieces of state and returns what changed. The caller — the daemon's command
//! loop, or the desktop over it — is what actually writes them and, for a
//! delete, what tears the projects down.
//!
//! Default is the one space that is always there. It cannot be renamed and it
//! cannot be deleted, so both refuse it rather than quietly doing nothing.

use crate::settings::AppSettings;
use okena_core::spaces::{DEFAULT_SPACE_ID, SpaceData, mint_space_id};
use okena_core::tasks::TaskScope;
use okena_state::WorkspaceData;

/// What a space holds, for the confirmation that names it before a delete.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpaceContents {
    /// Repos and other plain projects, by name, in workspace order.
    pub projects: Vec<String>,
    /// Agent sessions, by name — the closed ones too, since a space's agent
    /// history goes with it.
    pub agents: Vec<String>,
    /// Every project id in the space, agents included: exactly what a delete
    /// removes from okena. Files on disk are not touched.
    pub project_ids: Vec<String>,
}

impl SpaceContents {
    pub fn is_empty(&self) -> bool {
        self.project_ids.is_empty()
    }
}

/// What `space_id` holds.
pub fn contents(data: &WorkspaceData, space_id: &str) -> SpaceContents {
    let mut out = SpaceContents::default();
    for p in data.projects.iter().filter(|p| p.space_id == space_id) {
        out.project_ids.push(p.id.clone());
        if p.is_any_agent_session() {
            out.agents.push(p.name.clone());
        } else {
            out.projects.push(p.name.clone());
        }
    }
    out
}

/// Add a space called `name`, reading `connection` scoped by `tasks`.
///
/// Appended, so the selector's order is the order spaces were added and the
/// numbered shortcuts do not shuffle under anyone. Default stays first.
pub fn create(
    settings: &mut AppSettings,
    name: &str,
    connection: Option<String>,
    tasks: TaskScope,
) -> Result<SpaceData, String> {
    settings.ensure_spaces();
    let name = name.trim();
    if name.is_empty() {
        return Err("A space needs a name.".to_string());
    }
    let taken: Vec<String> = settings.spaces.iter().map(|s| s.id.clone()).collect();
    let mut space = SpaceData::new(mint_space_id(name, &taken), name);
    space.connection = connection.map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
    space.tasks = tasks;
    settings.spaces.push(space.clone());
    Ok(space)
}

/// Rename a space. Refuses Default and blank names.
pub fn rename(settings: &mut AppSettings, id: &str, name: &str) -> Result<(), String> {
    settings.ensure_spaces();
    let name = name.trim();
    if name.is_empty() {
        return Err("A space needs a name.".to_string());
    }
    if id == DEFAULT_SPACE_ID {
        return Err("The Default space cannot be renamed.".to_string());
    }
    let Some(space) = settings.space_mut(id) else {
        return Err(format!("No space called {id}."));
    };
    space.name = name.to_string();
    Ok(())
}

/// Point a space at another task backend connection, or at none.
pub fn set_connection(
    settings: &mut AppSettings,
    id: &str,
    connection: Option<String>,
) -> Result<(), String> {
    settings.ensure_spaces();
    let Some(space) = settings.space_mut(id) else {
        return Err(format!("No space called {id}."));
    };
    space.connection = connection
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty());
    Ok(())
}

/// Replace a space's hard task scope.
pub fn set_task_scope(
    settings: &mut AppSettings,
    id: &str,
    tasks: TaskScope,
) -> Result<(), String> {
    settings.ensure_spaces();
    let Some(space) = settings.space_mut(id) else {
        return Err(format!("No space called {id}."));
    };
    space.tasks = tasks;
    Ok(())
}

/// Remove a space from the list, and say which space is active afterwards.
///
/// Only the settings side: the caller removes the projects
/// [`contents`] named first, so their agents stop and their PTYs close. The
/// connection is left alone — connections outlive the spaces that used them
/// and are managed from Settings.
pub fn remove(settings: &mut AppSettings, id: &str) -> Result<String, String> {
    settings.ensure_spaces();
    if id == DEFAULT_SPACE_ID {
        return Err("The Default space cannot be deleted.".to_string());
    }
    if settings.space(id).is_none() {
        return Err(format!("No space called {id}."));
    }
    settings.spaces.retain(|s| s.id != id);
    if settings.active_space == id {
        settings.active_space = DEFAULT_SPACE_ID.to_string();
    }
    Ok(settings.active_space.clone())
}

/// The spaces reading `connection_id`, by name, in selector order.
pub fn spaces_using_connection(settings: &AppSettings, connection_id: &str) -> Vec<String> {
    settings
        .spaces
        .iter()
        .filter(|s| s.connection.as_deref() == Some(connection_id))
        .map(|s| s.name.clone())
        .collect()
}

/// Whether `connection_id` may be forgotten.
///
/// A connection a space still reads cannot be removed: the space would be left
/// pointing at nothing, and the message has to name which spaces are in the
/// way so the user knows what to repoint. Deleting a *space* does not touch
/// its connection — connections outlive them.
pub fn refuse_removing_connection(
    settings: &AppSettings,
    connection_id: &str,
) -> Result<(), String> {
    let using = spaces_using_connection(settings, connection_id);
    if using.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{connection_id} is still used by {}. Point {} at another connection first.",
        using.join(", "),
        if using.len() == 1 { "it" } else { "them" }
    ))
}

/// Switch to `id`. `Ok(false)` when it was already showing.
pub fn activate(settings: &mut AppSettings, id: &str) -> Result<bool, String> {
    settings.ensure_spaces();
    if settings.space(id).is_none() {
        return Err(format!("No space called {id}."));
    }
    if settings.active_space == id {
        return Ok(false);
    }
    settings.active_space = id.to_string();
    Ok(true)
}

/// Switch to the space `by` positions along, wrapping. `by` is +1 for the next
/// one and -1 for the previous.
pub fn step(settings: &mut AppSettings, by: isize) -> Result<bool, String> {
    settings.ensure_spaces();
    let active = settings.active_space.clone();
    let next = if by >= 0 {
        okena_core::spaces::next_space(&settings.spaces, &active)
    } else {
        okena_core::spaces::previous_space(&settings.spaces, &active)
    };
    let Some(next) = next.map(|s| s.id.clone()) else {
        return Ok(false);
    };
    activate(settings, &next)
}

/// Switch to the `n`th space, counting from 1. `Ok(false)` past the end, so
/// Cmd+5 with four spaces does nothing rather than erroring at the user.
pub fn activate_nth(settings: &mut AppSettings, n: usize) -> Result<bool, String> {
    settings.ensure_spaces();
    let Some(id) = okena_core::spaces::nth_space(&settings.spaces, n).map(|s| s.id.clone()) else {
        return Ok(false);
    };
    activate(settings, &id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_state::ProjectData;

    fn settings() -> AppSettings {
        let mut s = AppSettings::default();
        s.ensure_spaces();
        s
    }

    /// A plain project row. Built from JSON rather than spelled out: every
    /// field but these four has a serde default, and listing thirty of them
    /// here would break on the next one added.
    fn project(id: &str, name: &str, space: &str) -> ProjectData {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "path": format!("/tmp/{id}"),
            "layout": null, "space_id": space,
        }))
        .expect("a minimal project row")
    }

    fn agent(id: &str, name: &str, space: &str) -> ProjectData {
        let mut p = project(id, name, space);
        p.task_ref = Some(okena_core::tasks::TaskRef {
            id: okena_core::tasks::TaskId {
                provider: "linear".into(),
                external_id: "t1".into(),
            },
            display_key: "QBL-1".into(),
            title: "t".into(),
            url: String::new(),
            parent_id: None,
            parent_key: None,
        });
        p
    }

    #[test]
    fn a_new_space_is_appended_after_default_with_no_roots() {
        let mut s = settings();
        let space = create(&mut s, "  Client A  ", None, TaskScope::default()).expect("created");
        assert_eq!(space.name, "Client A");
        assert_eq!(space.id, "client-a");
        assert_eq!(s.spaces.len(), 2);
        assert_eq!(s.spaces[0].id, DEFAULT_SPACE_ID);
        assert_eq!(s.space_position("client-a"), Some(2));
        assert!(space.specs.folders.is_empty());
        // Adding a space does not switch to it; that is the caller's move.
        assert_eq!(s.active_space, DEFAULT_SPACE_ID);
    }

    #[test]
    fn a_space_needs_a_name() {
        let mut s = settings();
        assert!(create(&mut s, "   ", None, TaskScope::default()).is_err());
        assert_eq!(s.spaces.len(), 1);
    }

    #[test]
    fn a_new_space_can_reuse_a_connection_and_carry_filters() {
        let mut s = settings();
        let mut scope = TaskScope::default();
        scope.toggle_group(okena_core::tasks::GroupAxis::Project, "alpha");
        let space = create(&mut s, "Client A", Some("linear".into()), scope).expect("created");
        assert_eq!(space.connection.as_deref(), Some("linear"));
        assert_eq!(space.tasks.selected_count(), 1);
        // Sharing one connection with Default is allowed on purpose.
        assert_eq!(s.spaces[0].connection.as_deref(), Some("linear"));
    }

    #[test]
    fn default_can_be_neither_renamed_nor_deleted() {
        let mut s = settings();
        let renamed = rename(&mut s, DEFAULT_SPACE_ID, "Mine");
        assert!(renamed.is_err(), "{renamed:?}");
        assert_eq!(s.spaces[0].name, "Default");
        let deleted = remove(&mut s, DEFAULT_SPACE_ID);
        assert!(deleted.is_err(), "{deleted:?}");
        assert_eq!(s.spaces.len(), 1);
    }

    #[test]
    fn any_other_space_can_be_renamed() {
        let mut s = settings();
        create(&mut s, "Client A", None, TaskScope::default()).expect("created");
        rename(&mut s, "client-a", "  Acme  ").expect("renamed");
        assert_eq!(s.space("client-a").map(|s| s.name.as_str()), Some("Acme"));
        // The id does not follow the name — projects point at it.
        assert!(s.space("acme").is_none());
        assert!(rename(&mut s, "client-a", " ").is_err());
        assert!(rename(&mut s, "nope", "X").is_err());
    }

    #[test]
    fn deleting_the_space_you_are_in_falls_back_to_default() {
        let mut s = settings();
        create(&mut s, "Client A", None, TaskScope::default()).expect("created");
        activate(&mut s, "client-a").expect("activated");
        let active = remove(&mut s, "client-a").expect("removed");
        assert_eq!(active, DEFAULT_SPACE_ID);
        assert_eq!(s.active_space, DEFAULT_SPACE_ID);
        assert_eq!(s.spaces.len(), 1);
    }

    #[test]
    fn deleting_a_space_you_are_not_in_leaves_you_where_you_are() {
        let mut s = settings();
        create(&mut s, "Client A", None, TaskScope::default()).expect("created");
        create(&mut s, "Client B", None, TaskScope::default()).expect("created");
        activate(&mut s, "client-b").expect("activated");
        remove(&mut s, "client-a").expect("removed");
        assert_eq!(s.active_space, "client-b");
    }

    #[test]
    fn a_delete_names_the_projects_and_agents_it_will_remove() {
        let mut data = WorkspaceData::empty();
        data.projects.push(project("p1", "acme-web", "client-a"));
        data.projects.push(agent("a1", "QBL-1 login", "client-a"));
        data.projects.push(project("p2", "other", DEFAULT_SPACE_ID));

        let held = contents(&data, "client-a");
        assert_eq!(held.projects, ["acme-web"]);
        assert_eq!(held.agents, ["QBL-1 login"]);
        assert_eq!(held.project_ids, ["p1", "a1"]);
        assert!(!held.is_empty());
        // Default's project is untouched by any of it.
        assert!(contents(&data, DEFAULT_SPACE_ID).projects == ["other"]);
    }

    #[test]
    fn an_empty_space_has_nothing_to_confirm() {
        let data = WorkspaceData::empty();
        assert!(contents(&data, "client-a").is_empty());
    }

    #[test]
    fn a_closed_agent_still_counts_as_the_spaces_own() {
        // The closed-agent history belongs to the space and goes with it.
        let mut data = WorkspaceData::empty();
        let mut closed = agent("a1", "QBL-1 login", "client-a");
        closed.closed_at = Some(1);
        data.projects.push(closed);
        assert_eq!(contents(&data, "client-a").agents, ["QBL-1 login"]);
    }

    #[test]
    fn a_connection_no_space_reads_can_be_forgotten() {
        let mut s = settings();
        // Default reads `linear` by default; nothing reads `linear-2`.
        assert_eq!(refuse_removing_connection(&s, "linear-2"), Ok(()));
        create(&mut s, "Client A", Some("linear-2".into()), TaskScope::default())
            .expect("created");
        assert!(refuse_removing_connection(&s, "linear-2").is_err());
    }

    #[test]
    fn removing_a_connection_in_use_names_every_space_using_it() {
        let mut s = settings();
        create(&mut s, "Client A", Some("linear-2".into()), TaskScope::default())
            .expect("created");
        create(&mut s, "Client B", Some("linear-2".into()), TaskScope::default())
            .expect("created");
        let message = refuse_removing_connection(&s, "linear-2").expect_err("refused");
        assert!(message.contains("Client A"), "{message}");
        assert!(message.contains("Client B"), "{message}");
        assert!(message.contains("them"), "{message}");
        assert_eq!(
            spaces_using_connection(&s, "linear-2"),
            ["Client A", "Client B"]
        );
    }

    #[test]
    fn deleting_a_space_leaves_its_connection_alone() {
        // Connections outlive the spaces that used them; they are managed from
        // Settings, not by a delete.
        let mut s = settings();
        create(&mut s, "Client A", Some("linear-2".into()), TaskScope::default())
            .expect("created");
        remove(&mut s, "client-a").expect("removed");
        assert!(spaces_using_connection(&s, "linear-2").is_empty());
        // …and Default still reads its own.
        assert_eq!(s.spaces[0].connection.as_deref(), Some("linear"));
    }

    #[test]
    fn several_spaces_can_share_one_connection() {
        let mut s = settings();
        create(&mut s, "Client A", Some("linear".into()), TaskScope::default()).expect("created");
        assert_eq!(spaces_using_connection(&s, "linear"), ["Default", "Client A"]);
    }

    #[test]
    fn activating_the_space_already_showing_changes_nothing() {
        let mut s = settings();
        assert_eq!(activate(&mut s, DEFAULT_SPACE_ID), Ok(false));
        assert!(activate(&mut s, "nope").is_err());
    }

    #[test]
    fn stepping_cycles_through_the_spaces_and_wraps() {
        let mut s = settings();
        create(&mut s, "A", None, TaskScope::default()).expect("created");
        create(&mut s, "B", None, TaskScope::default()).expect("created");

        assert_eq!(step(&mut s, 1), Ok(true));
        assert_eq!(s.active_space, "a");
        assert_eq!(step(&mut s, 1), Ok(true));
        assert_eq!(s.active_space, "b");
        assert_eq!(step(&mut s, 1), Ok(true));
        assert_eq!(s.active_space, DEFAULT_SPACE_ID, "wraps to the front");
        assert_eq!(step(&mut s, -1), Ok(true));
        assert_eq!(s.active_space, "b", "and backwards off the front");
    }

    #[test]
    fn the_numbered_shortcuts_count_from_one_and_stop_at_the_end() {
        let mut s = settings();
        create(&mut s, "A", None, TaskScope::default()).expect("created");
        create(&mut s, "B", None, TaskScope::default()).expect("created");

        assert_eq!(activate_nth(&mut s, 3), Ok(true));
        assert_eq!(s.active_space, "b");
        assert_eq!(activate_nth(&mut s, 1), Ok(true));
        assert_eq!(s.active_space, DEFAULT_SPACE_ID);
        // Past the end does nothing rather than erroring at the user.
        assert_eq!(activate_nth(&mut s, 9), Ok(false));
        assert_eq!(s.active_space, DEFAULT_SPACE_ID);
    }

    #[test]
    fn a_spaces_connection_and_filters_can_be_edited_at_any_time() {
        let mut s = settings();
        create(&mut s, "Client A", Some("linear".into()), TaskScope::default()).expect("created");
        set_connection(&mut s, "client-a", Some("linear-2".into())).expect("repointed");
        assert_eq!(
            s.space("client-a").and_then(|s| s.connection.as_deref()),
            Some("linear-2")
        );
        let mut scope = TaskScope::default();
        scope.toggle_status("In Review");
        set_task_scope(&mut s, "client-a", scope).expect("refiltered");
        assert_eq!(
            s.space("client-a").map(|s| s.tasks.selected_count()),
            Some(1)
        );
        set_connection(&mut s, "client-a", Some("  ".into())).expect("cleared");
        assert!(s.space("client-a").and_then(|s| s.connection.as_ref()).is_none());
        assert!(set_connection(&mut s, "nope", None).is_err());
    }
}
