//! Chrome shared by every harness view, and the per-section dispatch.
//!
//! Tasks, Specs and Knowledge live in their own modules; this file holds the
//! toolbar, banners and buttons they all wear.

use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms, ui_text_sm};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};

use super::{HarnessPane, HarnessSection};
use crate::views::components::{SimpleInput, SimpleInputState};

impl HarnessPane {
    /// Translate a client-side project/terminal id into the id the daemon knows.
    ///
    /// Harness panes post straight through `RemoteActionClient` rather than the
    /// `ActionDispatcher`, so they never pass through `strip_remote_ids`. The
    /// client mirror prefixes every id as `remote:<connection>:<uuid>` while the
    /// daemon only knows the bare uuid — sending the prefixed form gets a
    /// "project not found" back. Any harness view putting an id in an action
    /// must route it through here.
    pub(super) fn daemon_id(&self, id: &str) -> String {
        okena_transport::client::strip_prefix(id, self.client.connection_id())
    }

    /// A small coloured label.
    pub(super) fn chip(&self, text: String, color: u32, cx: &Context<Self>) -> AnyElement {
        div()
            .px(px(6.0))
            .py(px(1.0))
            .rounded(px(3.0))
            .bg(with_alpha(color, 0.15))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(color))
            .child(text)
            .into_any_element()
    }

    /// Show what changed in a worktree.
    pub(super) fn open_diff(&self, project_id: &str, cx: &mut App) {
        crate::views::components::project_nav::open_diff(&self.request_broker, project_id, cx);
    }

    /// A small secondary button, used across the harness views.
    pub(super) fn small_button(
        &self,
        id: &'static str,
        label: &str,
        on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
        cx: &Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        div()
            .id(id)
            .cursor_pointer()
            .flex_shrink_0()
            .px(px(10.0))
            .py(px(3.0))
            .rounded(px(4.0))
            .bg(rgb(t.bg_secondary))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .text_size(ui_text_md(cx))
            .text_color(rgb(t.text_primary))
            .child(label.to_string())
            .on_mouse_down(MouseButton::Left, on_click)
            .into_any_element()
    }

    /// A multi-line field, the same in every harness form.
    ///
    /// Three things have to be true together and were not: the input has to be
    /// in `multiline` mode or a long paragraph is laid out as one endless line
    /// and runs off the side; the box has to scroll rather than clip, or the
    /// text you just typed is simply not there; and it has to follow the caret
    /// down, or writing past the fold means typing blind.
    ///
    /// A shared helper because "the same kind of text box" is not a thing you
    /// can keep true by remembering to.
    pub(super) fn multiline_field(
        &self,
        id: &'static str,
        state: &Entity<SimpleInputState>,
        height: f32,
        cx: &Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let scroll = self.field_scroll(id);
        // Following the caret only while it is at the end: someone typing at
        // the bottom wants to see what they are writing, someone editing a
        // line higher up does not want the view yanked off it.
        if state.read(cx).caret_at_end() {
            scroll.set_offset(point(px(0.0), -scroll.max_offset().y));
        }
        okena_ui::input::input_container(&t, None)
            .w_full()
            .h(px(height))
            .py(px(6.0))
            .child(
                div()
                    .id(id)
                    .size_full()
                    .px(px(8.0))
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .child(SimpleInput::new(state).text_size(ui_text(13.0, cx))),
            )
            .into_any_element()
    }

    /// The scroll position of one named field, kept across renders.
    fn field_scroll(&self, id: &'static str) -> ScrollHandle {
        self.field_scrolls
            .borrow_mut()
            .entry(id)
            .or_default()
            .clone()
    }

    /// The one action a harness view leads with: New.
    ///
    /// Extracted because Specs and Knowledge each had their own copy and Tasks
    /// had none, so the three views' primary buttons looked and sat
    /// differently. A shared shape is the only way "the same button" stays
    /// true after the next edit to one of them.
    pub(super) fn primary_button(
        &self,
        id: &'static str,
        label: &'static str,
        on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
        cx: &Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        div()
            .id(id)
            .cursor_pointer()
            .flex_shrink_0()
            .px(px(12.0))
            .py(px(4.0))
            .rounded(px(4.0))
            .bg(rgb(t.button_primary_bg))
            .hover(|s| s.bg(rgb(t.button_primary_hover)))
            .text_size(ui_text_md(cx))
            .text_color(rgb(t.button_primary_fg))
            .child(label)
            .on_mouse_down(MouseButton::Left, on_click)
            .into_any_element()
    }

    /// The agent sessions working on this view's material, listed under the
    /// tree they belong to.
    ///
    /// Deliberately a duplicate of what the main sidebar's AGENTS list already
    /// shows. That list answers "what is running"; this one answers "is
    /// anything writing the thing I am reading", which is a question you have
    /// while looking at the tree and would otherwise have to leave the view to
    /// answer.
    ///
    /// Renders nothing when nothing matches, rather than an empty heading:
    /// most of the time nothing is, and a permanent "Agents — none" would cost
    /// every reader a line to say so.
    pub(super) fn render_related_agents(
        &self,
        project_ids: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let t = theme(cx);
        let sessions: Vec<_> = project_ids
            .into_iter()
            .filter_map(|id| {
                crate::views::agent_session::AgentSessionInfo::collect(
                    self.workspace.read(cx),
                    &self.terminals,
                    &id,
                )
            })
            .collect();
        if sessions.is_empty() {
            return None;
        }
        let mut col = v_flex().pt(px(8.0)).child(self.section_label("Agents", cx));
        for info in sessions {
            let activity = info.activity();
            let id = info.project_id.clone();
            col = col.child(
                h_flex()
                    .id(SharedString::from(format!("harness-agent-{id}")))
                    .cursor_pointer()
                    .w_full()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(6.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .child(
                        div()
                            .flex_shrink_0()
                            .size(px(6.0))
                            .rounded_full()
                            .bg(rgb(activity.color(&t))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_primary))
                            .child(info.name.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_muted))
                            .child(activity.label()),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            crate::views::components::project_nav::focus_project(
                                &this.workspace,
                                &this.focus_manager,
                                this.window_id,
                                &id,
                                cx,
                            );
                            cx.notify();
                        }),
                    ),
            );
        }
        Some(col.into_any_element())
    }

    /// Report the outcome of something the user asked for.
    ///
    /// A toast rather than a banner in the view. These used to be a strip
    /// above the list that nothing could dismiss: it sat there after the work
    /// it described was over, pushed the list down, and gave the reader no way
    /// to be rid of it. okena already has a notification channel with a
    /// lifetime and a close button, and one channel is the whole point of
    /// having one.
    pub(super) fn report(&self, message: impl Into<String>, cx: &App) {
        crate::workspace::toast::ToastManager::success(message.into(), cx);
    }

    /// Report a failure of something the user asked for.
    pub(super) fn report_error(&self, message: impl Into<String>, cx: &App) {
        crate::workspace::toast::ToastManager::error(message.into(), cx);
    }

    pub(super) fn info_banner(&self, message: String, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .px(px(12.0))
            .py(px(6.0))
            .text_size(ui_text_sm(cx))
            .text_color(rgb(t.text_secondary))
            .child(message)
            .into_any_element()
    }

    pub(super) fn error_banner(&self, message: String, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .px(px(12.0))
            .py(px(6.0))
            .bg(with_alpha(t.error, 0.1))
            .text_size(ui_text_sm(cx))
            .text_color(rgb(t.error))
            .child(message)
            .into_any_element()
    }

    /// The toolbar every harness view wears.
    ///
    /// One shape for all of them — the view's name on the left, its own
    /// controls on the right — so moving between Tasks, Specs and Knowledge
    /// does not mean relearning where things are. Views differ only in what
    /// they put in `actions`.
    pub(super) fn render_toolbar(
        &self,
        actions: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        h_flex()
            .w_full()
            .flex_shrink_0()
            .items_center()
            .gap(px(8.0))
            .px(px(12.0))
            .py(px(6.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(ui_text(13.0, cx))
                    .text_color(rgb(t.text_primary))
                    .child(self.section.label()),
            )
            .child(div().flex_1().min_w_0())
            .children(actions)
            .into_any_element()
    }

    /// A square icon button for the toolbar.
    pub(super) fn toolbar_icon(
        &self,
        id: &'static str,
        icon: &'static str,
        tooltip: &'static str,
        on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
        cx: &Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        div()
            .id(id)
            .cursor_pointer()
            .flex_shrink_0()
            .size(px(24.0))
            .rounded(px(4.0))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path(icon)
                    .size(px(13.0))
                    .text_color(rgb(t.text_secondary)),
            )
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(tooltip).build(window, cx)
            })
            .on_mouse_down(MouseButton::Left, on_click)
            .into_any_element()
    }

    /// Open the settings modal on `page`.
    pub(super) fn open_settings(&self, page: &'static str, cx: &mut App) {
        self.request_broker.update(cx, |broker, cx| {
            broker.push_overlay_request(
                okena_workspace::requests::OverlayRequest::Settings {
                    page: Some(page.to_string()),
                },
                cx,
            );
        });
    }
}

impl Render for HarnessPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        // Exhaustive on purpose: a new section must choose its view here rather
        // than silently falling through to a placeholder.
        let body = match self.section {
            HarnessSection::Tasks => self.render_tasks_view(cx),
            HarnessSection::Specs => self.render_specs_view(window, cx),
            HarnessSection::Knowledge => self.render_knowledge_view(window, cx),
        };

        // No title bar and no close button: the sidebar's HARNESS nav already
        // shows which view is open, and every view carries its own toolbar. A
        // second bar on top of that read as a window pasted over the app rather
        // than part of it. Leaving happens by selecting a project or an
        // overview, the same way every other view is left.
        v_flex()
            .size_full()
            .bg(rgb(t.bg_primary))
            .child(div().flex_1().min_h_0().child(body))
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use okena_transport::client::strip_prefix;

    const LOCAL: &str = okena_transport::client::LOCAL_DAEMON_CONNECTION_ID;

    #[test]
    fn client_ids_are_stripped_to_the_daemons_form() {
        // What the client mirror holds vs. what the daemon knows. Sending the
        // prefixed form is exactly the "project not found" bug.
        let client_id = format!("remote:{LOCAL}:8d173ae8-a7f4-4598-b488-5aee654d471e");
        assert_eq!(
            strip_prefix(&client_id, LOCAL),
            "8d173ae8-a7f4-4598-b488-5aee654d471e"
        );
    }

    #[test]
    fn an_already_bare_id_is_unchanged() {
        // Ids that came back from the daemon (e.g. via the MCP path) must
        // survive a second stripping untouched.
        let bare = "8d173ae8-a7f4-4598-b488-5aee654d471e";
        assert_eq!(strip_prefix(bare, LOCAL), bare);
    }

    #[test]
    fn a_different_connections_prefix_is_left_alone() {
        // Stripping must be scoped to this connection, not any `remote:` prefix.
        let other = "remote:some-other-daemon:abc";
        assert_eq!(strip_prefix(other, LOCAL), other);
    }
}
