use gpui::*;
use okena_ui::tokens::ui_text_md;

/// Drag payload for project reordering
#[derive(Clone)]
pub struct ProjectDrag {
    pub project_id: String,
    pub project_name: String,
}

/// Drag preview view
pub struct ProjectDragView {
    pub name: String,
}

impl Render for ProjectDragView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .bg(rgb(0x2d2d2d))
            .border_1()
            .border_color(rgb(0x404040))
            .rounded(px(4.0))
            .shadow_lg()
            .text_size(ui_text_md(cx))
            .text_color(rgb(0xffffff))
            .child(self.name.clone())
    }
}

/// Drag payload for worktree reordering within a parent
#[derive(Clone)]
pub struct WorktreeDrag {
    pub worktree_id: String,
    pub parent_id: String,
    pub worktree_name: String,
}

/// Drag preview view for worktree items
pub struct WorktreeDragView {
    pub name: String,
}

impl Render for WorktreeDragView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .bg(rgb(0x2d2d2d))
            .border_1()
            .border_color(rgb(0x404040))
            .rounded(px(4.0))
            .shadow_lg()
            .text_size(ui_text_md(cx))
            .text_color(rgb(0xffffff))
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                svg()
                    .path("icons/git-branch.svg")
                    .size(px(12.0))
                    .text_color(rgb(0xcccccc)),
            )
            .child(self.name.clone())
    }
}

/// Drag payload for folder reordering
#[derive(Clone)]
pub struct FolderDrag {
    pub folder_id: String,
    pub folder_name: String,
}

/// Drag preview view for folders
pub struct FolderDragView {
    pub name: String,
}

impl Render for FolderDragView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .bg(rgb(0x2d2d2d))
            .border_1()
            .border_color(rgb(0x404040))
            .rounded(px(4.0))
            .shadow_lg()
            .text_size(ui_text_md(cx))
            .text_color(rgb(0xffffff))
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                svg()
                    .path("icons/folder.svg")
                    .size(px(12.0))
                    .text_color(rgb(0xcccccc)),
            )
            .child(self.name.clone())
    }
}

/// Drag payload for arranging an agent in the Agents list.
///
/// Only a top-level agent is dragged: its sub-agents come with it.
#[derive(Clone)]
pub struct AgentDrag {
    pub project_id: String,
    pub name: String,
    /// Whether it is pinned as the drag starts, so a drop target can tell a
    /// drop that would change something from one that would not.
    pub pinned: bool,
}

/// Drag preview view for agents
pub struct AgentDragView {
    pub name: String,
}

impl Render for AgentDragView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .bg(rgb(0x2d2d2d))
            .border_1()
            .border_color(rgb(0x404040))
            .rounded(px(4.0))
            .shadow_lg()
            .text_size(ui_text_md(cx))
            .text_color(rgb(0xffffff))
            .child(self.name.clone())
    }
}
