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
