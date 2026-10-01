//! The sidebar drawn in a real (test) window, driven by keys and the mouse.
//!
//! In its own file for the reason `from_project_test` gives. These tests go
//! through the window on purpose: what they pin is how a keystroke or a drag
//! reaches the sidebar, which a call to the handler would skip.

use super::{Sidebar, SidebarList};
use crate::{SidebarConfirm, SidebarDown, SidebarEscape, SidebarToggleExpand, SidebarUp};
use gpui::{
    AppContext as _, Entity, KeyBinding, Modifiers, MouseButton, TestAppContext, VisualTestContext,
    point, px,
};
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

// ---- pinning and arranging agents (QBL-438) ----

fn agent(id: &str, pinned: bool) -> ProjectData {
    let mut p = project(serde_json::json!({
        "id": id, "name": id, "path": "/projects", "custom_session": "goal",
    }));
    p.pinned = pinned;
    p
}

/// An agent on the task `task`, whose parent task is `parent`.
fn on_task(id: &str, task: &str, parent: Option<&str>) -> ProjectData {
    project(serde_json::json!({
        "id": id, "name": id, "path": "/projects",
        "task_ref": {
            "id": { "provider": "linear", "external_id": task },
            "display_key": task, "title": "t", "url": "http://x", "parent_id": parent,
        },
    }))
}

/// `p1` and `p2` pinned, in that order; `u1` and `u2` not; an `epic` with a
/// `story` under it, not pinned. A repo sits among them in `project_order`.
fn agents_data() -> WorkspaceData {
    let mut data = WorkspaceData::empty();
    data.projects = vec![
        project(serde_json::json!({ "id": "repo", "name": "repo", "path": "/r" })),
        agent("p1", true),
        agent("u1", false),
        agent("p2", true),
        agent("u2", false),
        on_task("epic", "E", None),
        on_task("story", "S", Some("E")),
    ];
    data.project_order = ["repo", "p1", "u1", "p2", "u2", "epic", "story"]
        .map(String::from)
        .to_vec();
    data
}

fn center(vcx: &mut VisualTestContext, selector: &'static str) -> gpui::Point<gpui::Pixels> {
    vcx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} was painted"))
        .center()
}

/// Press on `from`, carry it to `to`, let go. `to` is looked up once the
/// drag is under way, since the list makes room as one starts.
fn drag(vcx: &mut VisualTestContext, from: &'static str, to: &'static str) {
    let start = center(vcx, from);
    vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_move(
        start + point(px(6.0), px(6.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    vcx.run_until_parked();
    let end = center(vcx, to);
    vcx.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    vcx.run_until_parked();
}

/// What was dispatched, as `move <id> <index>` and `pin <id>`.
fn dispatched(actions: &Actions) -> Vec<String> {
    actions
        .borrow()
        .iter()
        .map(|action| match action {
            ActionRequest::MoveProject {
                project_id,
                new_index,
            } => format!("move {project_id} {new_index}"),
            ActionRequest::ToggleProjectPinned { project_id } => format!("pin {project_id}"),
            other => format!("{other:?}"),
        })
        .collect()
}

#[gpui::test]
fn dragging_an_unpinned_agent_onto_a_pinned_one_pins_it_there(cx: &mut TestAppContext) {
    let (_sidebar, actions, vcx) = draw(cx, agents_data(), SidebarList::Agents);
    drag(vcx, "agent-card-u2", "agent-card-p2");
    // Just above p2: index 3 of the order without u2 (repo, p1, u1, p2, …).
    assert_eq!(dispatched(&actions), ["move u2 3", "pin u2"]);
}

#[gpui::test]
fn dragging_a_pinned_agent_within_the_pinned_ones_only_moves_it(cx: &mut TestAppContext) {
    let (_sidebar, actions, vcx) = draw(cx, agents_data(), SidebarList::Agents);
    drag(vcx, "agent-card-p2", "agent-card-p1");
    assert_eq!(dispatched(&actions), ["move p2 1"]);
}

#[gpui::test]
fn dragging_below_the_last_pinned_agent_pins_at_the_end(cx: &mut TestAppContext) {
    let (_sidebar, actions, vcx) = draw(cx, agents_data(), SidebarList::Agents);
    drag(vcx, "agent-card-u1", "agent-pin-tail");
    // After p2 in the order without u1 (repo, p1, p2, …).
    assert_eq!(dispatched(&actions), ["move u1 3", "pin u1"]);
}

#[gpui::test]
fn dragging_a_pinned_agent_out_among_the_rest_unpins_it(cx: &mut TestAppContext) {
    let (_sidebar, actions, vcx) = draw(cx, agents_data(), SidebarList::Agents);
    drag(vcx, "agent-card-p1", "agent-card-u1");
    // The same action unpins as pins: it is a toggle.
    assert_eq!(dispatched(&actions), ["pin p1"]);
}

#[gpui::test]
fn dragging_an_unpinned_agent_among_the_unpinned_does_nothing(cx: &mut TestAppContext) {
    let (_sidebar, actions, vcx) = draw(cx, agents_data(), SidebarList::Agents);
    drag(vcx, "agent-card-u1", "agent-card-u2");
    assert_eq!(dispatched(&actions), Vec::<String>::new());
}

#[gpui::test]
fn the_first_pin_can_be_made_by_dragging(cx: &mut TestAppContext) {
    let mut data = agents_data();
    for p in &mut data.projects {
        p.pinned = false;
    }
    let (_sidebar, actions, vcx) = draw(cx, data, SidebarList::Agents);
    let resting = vcx.debug_bounds("agent-pin-head").unwrap().size.height;

    let start = center(vcx, "agent-card-u1");
    vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_move(
        start + point(px(6.0), px(6.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    vcx.run_until_parked();
    // With nothing pinned the strip at the top opens up into somewhere to aim.
    let carrying = vcx.debug_bounds("agent-pin-head").unwrap().size.height;
    assert!(
        carrying > resting,
        "the pin strip stayed {resting:?} tall while an agent was carried"
    );
    let end = center(vcx, "agent-pin-head");
    vcx.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    vcx.run_until_parked();
    // Nothing else is pinned, so there is no place to take: it is only pinned.
    assert_eq!(dispatched(&actions), ["pin u1"]);
}

#[gpui::test]
fn a_group_is_dragged_by_its_parent_and_never_by_a_sub_agent(cx: &mut TestAppContext) {
    let (_sidebar, actions, vcx) = draw(cx, agents_data(), SidebarList::Agents);
    // The sub-agent's card is not something that can be picked up.
    let start = center(vcx, "agent-card-story");
    vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_move(
        start + point(px(6.0), px(6.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    vcx.run_until_parked();
    assert!(!vcx.update(|_, cx| cx.has_active_drag()));
    let end = center(vcx, "agent-card-p1");
    vcx.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(dispatched(&actions), Vec::<String>::new());

    // Its parent is, and one move and one pin carry the whole group: the
    // sub-agent is listed under its parent wherever the parent is.
    drag(vcx, "agent-card-epic", "agent-card-p1");
    assert_eq!(dispatched(&actions), ["move epic 1", "pin epic"]);
}

fn right_click(vcx: &mut VisualTestContext, selector: &'static str) {
    let at = center(vcx, selector);
    vcx.simulate_mouse_down(at, MouseButton::Right, Modifiers::default());
    vcx.simulate_mouse_up(at, MouseButton::Right, Modifiers::default());
    vcx.run_until_parked();
}

fn press(vcx: &mut VisualTestContext, selector: &'static str) {
    let at = center(vcx, selector);
    vcx.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    vcx.run_until_parked();
}

#[gpui::test]
fn the_menu_pins_at_the_end_of_the_pinned_agents_and_unpins(cx: &mut TestAppContext) {
    let (_sidebar, actions, vcx) = draw(cx, agents_data(), SidebarList::Agents);
    right_click(vcx, "agent-card-u1");
    press(vcx, "agent-menu-pin");
    // Below p2, so agents sit in the order they were pinned.
    assert_eq!(dispatched(&actions), ["move u1 3", "pin u1"]);
    // And the menu has closed.
    assert!(vcx.debug_bounds("agent-menu-pin").is_none());

    actions.borrow_mut().clear();
    right_click(vcx, "agent-card-p1");
    press(vcx, "agent-menu-pin");
    assert_eq!(dispatched(&actions), ["pin p1"]);
}

#[gpui::test]
fn a_sub_agents_menu_does_not_offer_to_pin_it(cx: &mut TestAppContext) {
    let (_sidebar, _actions, vcx) = draw(cx, agents_data(), SidebarList::Agents);
    right_click(vcx, "agent-card-epic");
    assert!(vcx.debug_bounds("agent-menu-pin").is_some());
    right_click(vcx, "agent-card-story");
    assert!(vcx.debug_bounds("agent-menu-pin").is_none());
}

#[gpui::test]
fn folding_a_group_hides_its_sub_agents_and_nothing_else(cx: &mut TestAppContext) {
    let (_sidebar, actions, vcx) = draw(cx, agents_data(), SidebarList::Agents);
    assert!(vcx.debug_bounds("agent-card-story").is_some());

    press(vcx, "agent-fold-epic");
    assert!(vcx.debug_bounds("agent-card-story").is_none());
    // The parent's card and the fold control stay, and every other agent.
    for still in [
        "agent-card-epic",
        "agent-fold-epic",
        "agent-card-p1",
        "agent-card-u1",
    ] {
        assert!(vcx.debug_bounds(still).is_some(), "{still} went with it");
    }
    // Folding is the sidebar's own: nothing about pins or order is sent.
    assert_eq!(dispatched(&actions), Vec::<String>::new());

    press(vcx, "agent-fold-epic");
    assert!(vcx.debug_bounds("agent-card-story").is_some());
}

/// The agents' cards as painted, top to bottom.
fn painted_order(vcx: &mut VisualTestContext) -> Vec<&'static str> {
    let mut cards: Vec<(f32, &'static str)> = [
        ("p1", "agent-card-p1"),
        ("p2", "agent-card-p2"),
        ("u1", "agent-card-u1"),
        ("u2", "agent-card-u2"),
        ("epic", "agent-card-epic"),
        ("story", "agent-card-story"),
    ]
    .into_iter()
    .map(|(id, selector)| (f32::from(center(vcx, selector).y), id))
    .collect();
    cards.sort_by(|a, b| a.0.total_cmp(&b.0));
    cards.into_iter().map(|(_, id)| id).collect()
}

#[gpui::test]
fn the_list_paints_pinned_agents_first_in_both_sorts(cx: &mut TestAppContext) {
    use okena_workspace::state::AgentSortMode;
    // Arranged p2-then-p1, against both the name and the activity order, and
    // with the unpinned agents the more recently active.
    let mut data = agents_data();
    data.project_order = ["p2", "repo", "u1", "p1", "u2", "epic", "story"]
        .map(String::from)
        .to_vec();
    for (id, at) in [
        ("p1", 5),
        ("p2", 1),
        ("u1", 50),
        ("u2", 60),
        ("epic", 40),
        ("story", 70),
    ] {
        let p = data.projects.iter_mut().find(|p| p.id == id).unwrap();
        p.last_activity_at = Some(at);
    }

    data.main_window.agent_sort_mode = AgentSortMode::Activity;
    let (_sidebar, _actions, vcx) = draw(cx, data.clone(), SidebarList::Agents);
    assert_eq!(
        painted_order(vcx),
        ["p2", "p1", "u2", "u1", "epic", "story"]
    );

    data.main_window.agent_sort_mode = AgentSortMode::Name;
    let (_sidebar, _actions, vcx) = draw(cx, data, SidebarList::Agents);
    assert_eq!(
        painted_order(vcx),
        ["p2", "p1", "epic", "story", "u1", "u2"]
    );
}
