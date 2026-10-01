//! The sidebar drawn in a real (test) window, driven by keys and the mouse.
//!
//! In its own file for the reason `from_project_test` gives. These tests go
//! through the window on purpose: what they pin is how a keystroke or a drag
//! reaches the sidebar, which a call to the handler would skip.

use super::{Sidebar, SidebarList};
use crate::{SidebarConfirm, SidebarDown, SidebarEscape, SidebarToggleExpand, SidebarUp};
use gpui::{AppContext as _, Entity, KeyBinding, TestAppContext, VisualTestContext};
use okena_core::api::ActionRequest;
use okena_ui::theme::{DARK_THEME, GlobalThemeProvider};
use okena_workspace::focus::FocusManager;
use okena_workspace::request_broker::RequestBroker;
use okena_workspace::state::{ProjectData, WindowId, Workspace, WorkspaceData};
use std::cell::RefCell;
use std::rc::Rc;

/// Every action the sidebar dispatched, in order.
type Actions = Rc<RefCell<Vec<ActionRequest>>>;

fn project(json: serde_json::Value) -> ProjectData {
    serde_json::from_value(json).unwrap()
}

/// A repo with a worktree, a repo filed in a folder, and an agent session.
fn workspace_data() -> WorkspaceData {
    let mut data = WorkspaceData::empty();
    data.projects = vec![
        project(serde_json::json!({
            "id": "repo", "name": "repo", "path": "/r", "worktree_ids": ["wt"],
        })),
        project(serde_json::json!({
            "id": "wt", "name": "wt", "path": "/r/wt",
            "worktree_info": {
                "parent_project_id": "repo", "main_repo_path": "/r",
                "worktree_path": "/r/wt", "branch_name": "wt",
            },
        })),
        project(serde_json::json!({ "id": "filed", "name": "filed", "path": "/f" })),
        project(serde_json::json!({
            "id": "agent", "name": "agent", "path": "/projects", "custom_session": "goal",
        })),
    ];
    data.folders = vec![
        serde_json::from_value(serde_json::json!({
            "id": "folder", "name": "folder", "project_ids": ["filed"],
        }))
        .unwrap(),
    ];
    data.project_order = ["repo", "folder", "agent"].map(String::from).to_vec();
    data
}

/// The sidebar's keys as `okena-app` binds them by default. Repeated here
/// because the bindings live in the app crate, above this one.
fn bind_sidebar_keys(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.bind_keys([
            KeyBinding::new("up", SidebarUp, Some("Sidebar")),
            KeyBinding::new("down", SidebarDown, Some("Sidebar")),
            KeyBinding::new("enter", SidebarConfirm, Some("Sidebar")),
            KeyBinding::new("space", SidebarToggleExpand, Some("Sidebar")),
            KeyBinding::new("left", SidebarToggleExpand, Some("Sidebar")),
            KeyBinding::new("right", SidebarToggleExpand, Some("Sidebar")),
            KeyBinding::new("escape", SidebarEscape, Some("Sidebar")),
        ]);
    });
}

/// Draw the sidebar over `data`, showing `list`.
fn draw(
    cx: &mut TestAppContext,
    data: WorkspaceData,
    list: SidebarList,
) -> (Entity<Sidebar>, Actions, &mut VisualTestContext) {
    cx.update(|cx| cx.set_global(GlobalThemeProvider(|_| DARK_THEME)));
    bind_sidebar_keys(cx);
    let actions: Actions = Rc::default();
    let (for_project, for_connection) = (actions.clone(), actions.clone());
    let (sidebar, vcx) = cx.add_window_view(|_window, cx| {
        let workspace = cx.new(|_| Workspace::new(data));
        let focus = cx.new(|_| FocusManager::new());
        let broker = cx.new(|_| RequestBroker::new());
        let mut sidebar = Sidebar::new(
            WindowId::Main,
            workspace,
            focus,
            broker,
            Default::default(),
            cx,
        );
        sidebar.list = list;
        sidebar.set_dispatch_action(Box::new(move |_, action, _| {
            for_project.borrow_mut().push(action);
        }));
        sidebar.set_dispatch_for_connection(Box::new(move |_, action, _| {
            for_connection.borrow_mut().push(action);
        }));
        sidebar
    });
    vcx.run_until_parked();
    (sidebar, actions, vcx)
}

/// Type `fix login bug` over the name the rename box opened on, press Enter.
fn type_a_name_with_spaces(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.simulate_keystrokes("f i x space l o g i n space b u g enter");
    vcx.run_until_parked();
}

fn renamed_to(actions: &Actions) -> Vec<String> {
    actions
        .borrow()
        .iter()
        .filter_map(|action| match action {
            ActionRequest::RenameProject { name, .. }
            | ActionRequest::RenameFolder { name, .. }
            | ActionRequest::RenameTerminal { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

// ---- rename (QBL-438) ----

#[gpui::test]
fn renaming_an_agent_session_keeps_the_spaces_typed(cx: &mut TestAppContext) {
    let (sidebar, actions, vcx) = draw(cx, workspace_data(), SidebarList::Agents);
    sidebar.update_in(vcx, |s, window, cx| {
        s.start_project_rename("agent".into(), "agent".into(), window, cx);
    });
    type_a_name_with_spaces(vcx);
    assert_eq!(renamed_to(&actions), ["fix login bug"]);
}

#[gpui::test]
fn renaming_a_project_keeps_the_spaces_typed(cx: &mut TestAppContext) {
    let (sidebar, actions, vcx) = draw(cx, workspace_data(), SidebarList::Projects);
    sidebar.update_in(vcx, |s, window, cx| {
        s.start_project_rename("repo".into(), "repo".into(), window, cx);
    });
    type_a_name_with_spaces(vcx);
    assert_eq!(renamed_to(&actions), ["fix login bug"]);
}

#[gpui::test]
fn renaming_a_worktree_keeps_the_spaces_typed(cx: &mut TestAppContext) {
    let (sidebar, actions, vcx) = draw(cx, workspace_data(), SidebarList::Projects);
    sidebar.update_in(vcx, |s, window, cx| {
        s.start_project_rename("wt".into(), "wt".into(), window, cx);
    });
    type_a_name_with_spaces(vcx);
    assert_eq!(renamed_to(&actions), ["fix login bug"]);
}

#[gpui::test]
fn renaming_a_folder_keeps_the_spaces_typed(cx: &mut TestAppContext) {
    let (sidebar, actions, vcx) = draw(cx, workspace_data(), SidebarList::Projects);
    sidebar.update_in(vcx, |s, window, cx| {
        s.start_folder_rename("folder".into(), "folder".into(), window, cx);
    });
    type_a_name_with_spaces(vcx);
    assert_eq!(renamed_to(&actions), ["fix login bug"]);
}

#[gpui::test]
fn renaming_a_terminal_keeps_the_spaces_typed(cx: &mut TestAppContext) {
    // A project with one terminal, expanded so the terminal's row — and with
    // it the rename box — is on screen.
    let mut data = workspace_data();
    let mut solo = project(serde_json::json!({ "id": "solo", "name": "solo", "path": "/s" }));
    solo.layout = Some(okena_workspace::state::LayoutNode::Terminal {
        terminal_id: Some("t1".into()),
        minimized: false,
        detached: false,
        shell_type: Default::default(),
        zoom_level: 1.0,
        agent: false,
    });
    data.projects.push(solo);
    data.project_order.push("solo".into());
    let (sidebar, actions, vcx) = draw(cx, data, SidebarList::Projects);
    sidebar.update_in(vcx, |s, window, cx| {
        s.toggle_expanded("solo");
        s.start_rename("solo".into(), "t1".into(), "zsh".into(), window, cx);
    });
    type_a_name_with_spaces(vcx);
    assert_eq!(renamed_to(&actions), ["fix login bug"]);
}

#[gpui::test]
fn the_arrow_keys_move_the_caret_in_a_rename(cx: &mut TestAppContext) {
    // Left and right are bound in the sidebar too, to the same expand toggle
    // as space, and were lost to a rename the same way.
    let (sidebar, actions, vcx) = draw(cx, workspace_data(), SidebarList::Agents);
    sidebar.update_in(vcx, |s, window, cx| {
        s.start_project_rename("agent".into(), "ab".into(), window, cx);
    });
    vcx.run_until_parked();
    // The name opens selected; End drops the selection with the caret after
    // it, and left then steps back one.
    vcx.simulate_keystrokes("end left space enter");
    vcx.run_until_parked();
    assert_eq!(renamed_to(&actions), ["a b"]);
}

#[gpui::test]
fn a_rename_opens_with_the_name_selected(cx: &mut TestAppContext) {
    // Typing straight away replaces the old name rather than adding to it…
    let (sidebar, actions, vcx) = draw(cx, workspace_data(), SidebarList::Agents);
    sidebar.update_in(vcx, |s, window, cx| {
        s.start_project_rename("agent".into(), "agent".into(), window, cx);
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("x enter");
    vcx.run_until_parked();
    assert_eq!(renamed_to(&actions), ["x"]);

    // …while moving the caret first keeps it, to be edited. A folder this
    // time, so in the list that draws folders.
    let (sidebar, actions, vcx) = draw(cx, workspace_data(), SidebarList::Projects);
    sidebar.update_in(vcx, |s, window, cx| {
        s.start_folder_rename("folder".into(), "folder".into(), window, cx);
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("end s enter");
    vcx.run_until_parked();
    assert_eq!(renamed_to(&actions), ["folders"]);
}
