//! Keyboard navigation over the agent rows in the Projects list.
//!
//! In its own file for the reason `from_project_test` gives, and because
//! `use gpui::*` would shadow `#[test]`.

use super::{Sidebar, SidebarCursorItem};
use gpui::{AppContext as _, TestAppContext};
use okena_workspace::focus::FocusManager;
use okena_workspace::request_broker::RequestBroker;
use okena_workspace::state::{ProjectData, WindowId, Workspace, WorkspaceData};

fn project(json: serde_json::Value) -> ProjectData {
    serde_json::from_value(json).unwrap()
}

fn task(external: &str) -> serde_json::Value {
    serde_json::json!({
        "id": { "provider": "linear", "external_id": external },
        "display_key": external.to_uppercase(), "title": "t", "url": "http://x",
    })
}

/// A repo with one worktree on task u1; a live and a closed session on u1;
/// a coordinator given the repo with no worktree; a spec session linked to
/// nothing.
fn workspace_data(show_agents: bool) -> WorkspaceData {
    let mut data = WorkspaceData::empty();
    data.projects = vec![
        project(serde_json::json!({
            "id": "repo", "name": "repo", "path": "/r", "worktree_ids": ["wt1"],
        })),
        project(serde_json::json!({
            "id": "wt1", "name": "wt1", "path": "/r/wt1", "task_ref": task("u1"),
            "worktree_info": {
                "parent_project_id": "repo", "main_repo_path": "/r",
                "worktree_path": "/r/wt1", "branch_name": "wt1",
            },
        })),
        project(serde_json::json!({
            "id": "live", "name": "live", "path": "/projects", "task_ref": task("u1"),
        })),
        project(serde_json::json!({
            "id": "closed", "name": "closed", "path": "/projects", "task_ref": task("u1"),
            "closed_at": 5,
        })),
        project(serde_json::json!({
            "id": "coord", "name": "coord", "path": "/projects",
            "custom_session": "goal", "repo_ids": ["repo"],
        })),
        project(serde_json::json!({
            "id": "spec", "name": "spec", "path": "/specs", "spec_change": "add-login",
        })),
    ];
    data.project_order = ["repo", "live", "closed", "coord", "spec"]
        .map(String::from)
        .to_vec();
    data.main_window.projects_show_agents = show_agents;
    data
}

fn sidebar(cx: &mut TestAppContext, show_agents: bool) -> gpui::Entity<Sidebar> {
    sidebar_over(cx, workspace_data(show_agents))
}

fn sidebar_over(cx: &mut TestAppContext, data: WorkspaceData) -> gpui::Entity<Sidebar> {
    cx.update(|cx| {
        let workspace = cx.new(|_| Workspace::new(data));
        let focus = cx.new(|_| FocusManager::new());
        let broker = cx.new(|_| RequestBroker::new());
        cx.new(|cx| {
            Sidebar::new(
                WindowId::Main,
                workspace,
                focus,
                broker,
                Default::default(),
                cx,
            )
        })
    })
}

fn rows(sidebar: &gpui::Entity<Sidebar>, cx: &mut TestAppContext) -> Vec<String> {
    sidebar.update(cx, |s, cx| {
        s.build_cursor_items(cx)
            .into_iter()
            .map(|item| match item {
                SidebarCursorItem::Project { project_id } => format!("project:{project_id}"),
                SidebarCursorItem::WorktreeProject { project_id } => {
                    format!("worktree:{project_id}")
                }
                SidebarCursorItem::Agent { project_id } => format!("agent:{project_id}"),
                other => format!("{other:?}"),
            })
            .collect()
    })
}

#[gpui::test]
fn the_cursor_reaches_agents_in_display_order(cx: &mut TestAppContext) {
    let sidebar = sidebar(cx, true);
    // Live before closed under the worktree; the coordinator directly under
    // the repo after its worktrees; the spec session nowhere.
    assert_eq!(
        rows(&sidebar, cx),
        [
            "project:repo",
            "worktree:repo",
            "worktree:wt1",
            "agent:live",
            "agent:closed",
            "agent:coord",
        ]
    );
}

#[gpui::test]
fn collapsing_a_repos_worktrees_hides_its_agents(cx: &mut TestAppContext) {
    let sidebar = sidebar(cx, true);
    sidebar.update(cx, |s, _| s.toggle_worktrees_collapsed("repo"));
    assert_eq!(rows(&sidebar, cx), ["project:repo"]);
}

#[gpui::test]
fn turning_show_agents_off_leaves_the_worktrees_alone(cx: &mut TestAppContext) {
    let sidebar = sidebar(cx, false);
    assert_eq!(
        rows(&sidebar, cx),
        ["project:repo", "worktree:repo", "worktree:wt1"]
    );
}

#[gpui::test]
fn agents_nest_the_same_way_inside_a_folder(cx: &mut TestAppContext) {
    let mut data = workspace_data(true);
    data.folders = vec![
        serde_json::from_value(serde_json::json!({
            "id": "f", "name": "f", "project_ids": ["repo", "coord", "spec"],
        }))
        .unwrap(),
    ];
    data.project_order = ["f", "live", "closed"].map(String::from).to_vec();
    let sidebar = sidebar_over(cx, data);
    // Sessions filed in the folder are not drawn as its projects either.
    assert_eq!(
        rows(&sidebar, cx),
        [
            "Folder { folder_id: \"f\" }",
            "project:repo",
            "worktree:repo",
            "worktree:wt1",
            "agent:live",
            "agent:closed",
            "agent:coord",
        ]
    );
}

// ---- spaces (QBL-430) ----

/// Two spaces, each with a repo, a worktree on a task, and an agent on it —
/// the same shape as `workspace_data`, so the agent really nests under its
/// repo rather than being dropped for an unrelated reason.
fn two_space_data() -> WorkspaceData {
    let side = |space: &str, tag: &str, task_id: &str| {
        vec![
            project(serde_json::json!({
                "id": format!("{tag}-repo"), "name": format!("{tag}-repo"),
                "path": format!("/{tag}"), "worktree_ids": [format!("{tag}-wt")],
                "space_id": space,
            })),
            project(serde_json::json!({
                "id": format!("{tag}-wt"), "name": format!("{tag}-wt"),
                "path": format!("/{tag}/wt"), "task_ref": task(task_id),
                "space_id": space,
                "worktree_info": {
                    "parent_project_id": format!("{tag}-repo"),
                    "main_repo_path": format!("/{tag}"),
                    "worktree_path": format!("/{tag}/wt"), "branch_name": "wt",
                },
            })),
            project(serde_json::json!({
                "id": format!("{tag}-agent"), "name": format!("{tag}-agent"),
                "path": "/projects", "task_ref": task(task_id), "space_id": space,
            })),
        ]
    };
    let mut data = WorkspaceData::empty();
    data.projects = side("default", "here", "u1");
    data.projects.extend(side("client-a", "there", "u2"));
    data.project_order = ["here-repo", "here-agent", "there-repo", "there-agent"]
        .map(String::from)
        .to_vec();
    data.main_window.projects_show_agents = true;
    data
}

#[gpui::test]
fn the_sidebar_lists_only_the_active_spaces_projects_and_agents(cx: &mut TestAppContext) {
    // The regression this pins: every list in the sidebar used to walk
    // `data().projects`, which holds every space's rows, so switching spaces
    // changed the dots and nothing else.
    let sidebar = sidebar_over(cx, two_space_data());
    assert_eq!(
        rows(&sidebar, cx),
        ["project:here-repo", "worktree:here-repo", "worktree:here-wt", "agent:here-agent"],
        "Default's own, and nothing from Client A"
    );
}

#[gpui::test]
fn switching_spaces_swaps_what_the_sidebar_lists(cx: &mut TestAppContext) {
    let sidebar = sidebar_over(cx, two_space_data());
    sidebar.update(cx, |s, cx| {
        s.workspace.update(cx, |ws, _cx| {
            ws.set_active_space("client-a");
        });
    });
    assert_eq!(
        rows(&sidebar, cx),
        ["project:there-repo", "worktree:there-repo", "worktree:there-wt", "agent:there-agent"],
        "…and back the other way, with neither space leaking into the other"
    );
}

#[gpui::test]
fn a_folder_in_another_space_is_not_listed(cx: &mut TestAppContext) {
    let mut data = two_space_data();
    data.folders = vec![serde_json::from_value(serde_json::json!({
        "id": "f-theirs", "name": "Theirs", "project_ids": ["there-repo"],
        "space_id": "client-a",
    }))
    .unwrap()];
    data.project_order = ["here-repo", "f-theirs"].map(String::from).to_vec();
    let sidebar = sidebar_over(cx, data);
    assert_eq!(rows(&sidebar, cx), ["project:here-repo", "worktree:here-repo", "worktree:here-wt", "agent:here-agent"]);
}

/// What the *render* walk would draw, resolved the way it resolves it.
///
/// The cursor walk and the render walk are two separate passes over
/// `project_order`; `rows` above covers the first. This covers the second —
/// the one that was still showing every space's projects after the cursor
/// walk had been fixed.
fn rendered(sidebar: &gpui::Entity<Sidebar>, cx: &mut TestAppContext) -> Vec<String> {
    sidebar.update(cx, |s, cx| {
        let workspace = s.workspace.read(cx);
        let (by_id, _) = super::visible_projects_by_id(workspace);
        let mut out = Vec::new();
        for id in &workspace.data().project_order {
            if let Some(folder) = super::visible_folder(workspace, id) {
                out.push(format!("folder:{}", folder.id));
                for pid in &folder.project_ids {
                    if let Some(p) = by_id.get(pid.as_str())
                        && p.agent_role().is_none()
                    {
                        out.push(format!("project:{}", p.id));
                    }
                }
                continue;
            }
            // Agent sessions belong to the Agents list, as in the render walk.
            if let Some(p) = by_id.get(id.as_str())
                && p.agent_role().is_none()
            {
                out.push(format!("project:{}", p.id));
            }
        }
        out
    })
}

#[gpui::test]
fn the_rendered_project_list_holds_only_the_active_space(cx: &mut TestAppContext) {
    // The bug this pins: the manual render path built its own unfiltered
    // lookup, so switching spaces moved the dots and left the list alone.
    let sidebar = sidebar_over(cx, two_space_data());
    assert_eq!(rendered(&sidebar, cx), ["project:here-repo"]);

    sidebar.update(cx, |s, cx| {
        s.workspace.update(cx, |ws, _cx| {
            ws.set_active_space("client-a");
        });
    });
    assert_eq!(rendered(&sidebar, cx), ["project:there-repo"]);
}

#[gpui::test]
fn the_rendered_list_and_the_cursor_agree_about_what_is_on_screen(cx: &mut TestAppContext) {
    // The two walks drifting apart is what let the cursor be fixed while the
    // list was not. Whatever the render walk draws, the cursor must reach.
    let sidebar = sidebar_over(cx, two_space_data());
    for space in ["default", "client-a"] {
        sidebar.update(cx, |s, cx| {
            s.workspace.update(cx, |ws, _cx| {
                ws.set_active_space(space);
            });
        });
        let drawn: Vec<String> = rendered(&sidebar, cx)
            .into_iter()
            .filter(|r| r.starts_with("project:"))
            .collect();
        let reachable: Vec<String> = rows(&sidebar, cx)
            .into_iter()
            .filter(|r| r.starts_with("project:"))
            .collect();
        assert_eq!(drawn, reachable, "in {space}");
    }
}

#[gpui::test]
fn a_folder_from_another_space_does_not_render_at_all(cx: &mut TestAppContext) {
    // Not even as an empty row: its projects resolve to nothing, and a folder
    // with no visible reason to be empty reads as a bug.
    let mut data = two_space_data();
    data.folders = vec![serde_json::from_value(serde_json::json!({
        "id": "f-theirs", "name": "Theirs", "project_ids": ["there-repo"],
        "space_id": "client-a",
    }))
    .unwrap()];
    data.project_order = ["here-repo", "f-theirs"].map(String::from).to_vec();
    let sidebar = sidebar_over(cx, data);
    assert_eq!(rendered(&sidebar, cx), ["project:here-repo"]);
}
