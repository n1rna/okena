//! Search, then pick into chips.
//!
//! A search box, the results under it, and what has been picked as removable
//! chips — the one control every launcher uses to choose projects and context
//! (QBL-406). It knows nothing about what it searches: the host listens for
//! [`ChipSearchEvent::QueryChanged`], runs the search however it runs it — a
//! filter over projects, a daemon round trip — and hands the results back with
//! [`ChipSearch::set_results`].
//!
//! Keyboard: ↑/↓ move through results, Enter adds the highlighted one,
//! Backspace in an empty box removes the last chip, Esc closes the list. They
//! are taken in the capture phase, before the text input sees them, and only
//! when they mean something here — an Esc with the list closed still reaches
//! the dialog around it.

use crate::selectable_list::selectable_list_item;
use crate::simple_input::{InputChangedEvent, SimpleInput, SimpleInputState};
use crate::theme::{theme, with_alpha};
use crate::tokens::{ui_text_md, ui_text_ms};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};

/// One pickable thing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChipItem {
    /// Identity: two results with one id are the same item.
    pub id: SharedString,
    pub title: SharedString,
    /// What it is, e.g. "Skill", drawn as a label before the title.
    pub kind: Option<SharedString>,
    /// One line under the title.
    pub description: Option<SharedString>,
    /// Whose it is — a project or a store — drawn on the right.
    pub owner: Option<SharedString>,
}

impl ChipItem {
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            kind: None,
            description: None,
            owner: None,
        }
    }

    pub fn kind(mut self, kind: impl Into<SharedString>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        let description = description.into();
        self.description = (!description.is_empty()).then_some(description);
        self
    }

    pub fn owner(mut self, owner: impl Into<SharedString>) -> Self {
        self.owner = Some(owner.into());
        self
    }
}

/// A row above the results that is not a result: something to say, with one
/// action — "shop isn't mapped yet · Scan".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChipHint {
    /// Handed back in [`ChipSearchEvent::Hint`].
    pub id: SharedString,
    pub text: SharedString,
    pub action: SharedString,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChipSearchEvent {
    /// The text in the box changed; search again.
    QueryChanged(String),
    Added(ChipItem),
    Removed(ChipItem),
    /// A hint's action was clicked.
    Hint(SharedString),
}

/// Rows shown before the list scrolls, at their usual height.
const LIST_MAX_HEIGHT: f32 = 220.0;

pub struct ChipSearch {
    id: SharedString,
    input: Entity<SimpleInputState>,
    results: Vec<ChipItem>,
    hints: Vec<ChipHint>,
    chips: Vec<ChipItem>,
    /// Index into [`Self::visible_results`].
    highlighted: usize,
    open: bool,
    /// Said in the list when a search found nothing.
    empty_text: Option<SharedString>,
    scroll: ScrollHandle,
    _input_changed: Subscription,
}

impl EventEmitter<ChipSearchEvent> for ChipSearch {}

impl ChipSearch {
    pub fn new(
        id: impl Into<SharedString>,
        placeholder: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = placeholder.into();
        let input = cx.new(|cx| SimpleInputState::new(cx).placeholder(placeholder));
        let input_changed = cx.subscribe(&input, |this, input, _: &InputChangedEvent, cx| {
            let query = input.read(cx).value().to_string();
            this.open = true;
            this.highlighted = 0;
            cx.emit(ChipSearchEvent::QueryChanged(query));
            cx.notify();
        });
        Self {
            id: id.into(),
            input,
            results: Vec::new(),
            hints: Vec::new(),
            chips: Vec::new(),
            highlighted: 0,
            open: false,
            empty_text: None,
            scroll: ScrollHandle::new(),
            _input_changed: input_changed,
        }
    }

    /// What the list says when a search finds nothing. Without it the list
    /// simply does not open.
    pub fn empty_text(mut self, text: impl Into<SharedString>) -> Self {
        self.empty_text = Some(text.into());
        self
    }

    pub fn query(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// What has been picked, in the order it was.
    pub fn chips(&self) -> &[ChipItem] {
        &self.chips
    }

    /// Replace the picks, without events: a host preselecting, not a user
    /// picking.
    pub fn set_chips(&mut self, chips: Vec<ChipItem>, cx: &mut Context<Self>) {
        self.chips = chips;
        self.clamp_highlight();
        cx.notify();
    }

    pub fn set_results(&mut self, results: Vec<ChipItem>, cx: &mut Context<Self>) {
        self.results = results;
        self.clamp_highlight();
        cx.notify();
    }

    pub fn set_hints(&mut self, hints: Vec<ChipHint>, cx: &mut Context<Self>) {
        self.hints = hints;
        cx.notify();
    }

    /// Results not already picked: an added item does not show again.
    pub fn visible_results(&self) -> Vec<&ChipItem> {
        self.results
            .iter()
            .filter(|r| !self.chips.iter().any(|c| c.id == r.id))
            .collect()
    }

    pub fn highlighted(&self) -> Option<&ChipItem> {
        self.visible_results().get(self.highlighted).copied()
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Show the results. The host is asked to search for what is typed, so a
    /// list opened later is not left showing what was current earlier.
    pub fn open(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            self.open = true;
            let query = self.query(cx);
            cx.emit(ChipSearchEvent::QueryChanged(query));
            cx.notify();
        }
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            cx.notify();
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Pick `item`, clearing the box for the next search.
    pub fn add(&mut self, item: ChipItem, cx: &mut Context<Self>) {
        if self.chips.iter().any(|c| c.id == item.id) {
            return;
        }
        self.chips.push(item.clone());
        cx.emit(ChipSearchEvent::Added(item));
        self.input.update(cx, |input, cx| input.set_value("", cx));
        self.clamp_highlight();
        cx.notify();
    }

    pub fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(at) = self.chips.iter().position(|c| c.id.as_ref() == id) {
            let item = self.chips.remove(at);
            cx.emit(ChipSearchEvent::Removed(item));
            cx.notify();
        }
    }

    fn clamp_highlight(&mut self) {
        let len = self.visible_results().len();
        self.highlighted = self.highlighted.min(len.saturating_sub(1));
    }

    /// Act on `key`. `true` when it meant something here, so it goes no
    /// further.
    fn handle_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let len = self.visible_results().len();
        match key {
            "down" => {
                if !self.open {
                    self.open(cx);
                } else if self.highlighted + 1 < len {
                    self.highlighted += 1;
                    self.scroll
                        .scroll_to_item(self.hints.len() + self.highlighted);
                }
                cx.notify();
                true
            }
            "up" if self.open => {
                if self.highlighted > 0 {
                    self.highlighted -= 1;
                    self.scroll
                        .scroll_to_item(self.hints.len() + self.highlighted);
                    cx.notify();
                }
                true
            }
            "enter" if self.open && len > 0 => {
                if let Some(item) = self.highlighted().cloned() {
                    self.add(item, cx);
                }
                true
            }
            "escape" if self.open => {
                self.close(cx);
                true
            }
            "backspace" if self.input.read(cx).value().is_empty() && !self.chips.is_empty() => {
                if let Some(last) = self.chips.last().map(|c| c.id.clone()) {
                    self.remove(&last, cx);
                }
                true
            }
            _ => false,
        }
    }

    fn render_chip(&self, item: &ChipItem, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let id = item.id.clone();
        h_flex()
            .id(SharedString::from(format!("{}-chip-{}", self.id, item.id)))
            .flex_shrink_0()
            .max_w_full()
            .min_w_0()
            .items_center()
            .gap(px(4.0))
            .pl(px(8.0))
            .pr(px(4.0))
            .py(px(2.0))
            .rounded(px(4.0))
            .bg(with_alpha(t.button_primary_bg, 0.18))
            .text_size(ui_text_ms(cx))
            .children(item.kind.clone().map(|kind| {
                div()
                    .flex_shrink_0()
                    .text_color(rgb(t.text_muted))
                    .child(kind)
            }))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(rgb(t.text_primary))
                    .child(item.title.clone()),
            )
            .child(
                div()
                    .id(SharedString::from(format!(
                        "{}-chip-{}-remove",
                        self.id, item.id
                    )))
                    .flex_shrink_0()
                    .cursor_pointer()
                    .px(px(3.0))
                    .rounded(px(3.0))
                    .text_color(rgb(t.text_secondary))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .child("✕")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.remove(&id, cx);
                        }),
                    ),
            )
            .into_any_element()
    }

    fn render_list(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let visible: Vec<ChipItem> = self.visible_results().into_iter().cloned().collect();
        let empty = visible.is_empty() && self.hints.is_empty();
        if !self.open || (empty && self.empty_text.is_none()) {
            return None;
        }
        let t = theme(cx);
        let mut list = v_flex()
            .id(SharedString::from(format!("{}-results", self.id)))
            .w_full()
            .max_h(px(LIST_MAX_HEIGHT))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            // The list scrolls on its own: without this the wheel also scrolled
            // the dialog it sits in.
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .py(px(2.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_primary));

        for hint in &self.hints {
            let hint_id = hint.id.clone();
            list = list.child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(12.0))
                    .py(px(6.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_secondary))
                            .child(hint.text.clone()),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("{}-hint-{}", self.id, hint.id)))
                            .flex_shrink_0()
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .bg(rgb(t.bg_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_primary))
                            .child(hint.action.clone())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |_, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.emit(ChipSearchEvent::Hint(hint_id.clone()));
                                }),
                            ),
                    ),
            );
        }

        if empty && let Some(text) = self.empty_text.clone() {
            list = list.child(
                div()
                    .px(px(12.0))
                    .py(px(6.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(text),
            );
        }

        for (i, item) in visible.into_iter().enumerate() {
            let picked = item.clone();
            list = list.child(
                selectable_list_item(
                    SharedString::from(format!("{}-result-{}", self.id, item.id)),
                    i == self.highlighted,
                    &t,
                )
                .w_full()
                .min_w_0()
                .py(px(5.0))
                .child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .gap(px(1.0))
                        .child(
                            h_flex()
                                .w_full()
                                .min_w_0()
                                .items_center()
                                .gap(px(6.0))
                                .children(item.kind.clone().map(|kind| {
                                    div()
                                        .flex_shrink_0()
                                        .px(px(5.0))
                                        .rounded(px(3.0))
                                        .bg(rgb(t.bg_secondary))
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child(kind)
                                }))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(ui_text_md(cx))
                                        .text_color(rgb(t.text_primary))
                                        .child(item.title.clone()),
                                )
                                .children(item.owner.clone().map(|owner| {
                                    div()
                                        .flex_shrink_0()
                                        .max_w(px(140.0))
                                        .truncate()
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child(owner)
                                })),
                        )
                        .children(item.description.clone().map(|description| {
                            div()
                                .w_full()
                                .min_w_0()
                                .truncate()
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_secondary))
                                .child(description)
                        })),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.add(picked.clone(), cx);
                    }),
                ),
            );
        }
        Some(list.into_any_element())
    }
}

impl Render for ChipSearch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        let chips: Vec<AnyElement> = self
            .chips
            .clone()
            .iter()
            .map(|item| self.render_chip(item, cx))
            .collect();
        let list = self.render_list(cx);

        v_flex()
            .id(self.id.clone())
            .w_full()
            .min_w_0()
            .gap(px(4.0))
            // A press anywhere outside the box and its list closes the list.
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close(cx)))
            .child(
                crate::input::input_container(&t, Some(focused))
                    .w_full()
                    .min_w_0()
                    .px(px(6.0))
                    .py(px(4.0))
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .flex_wrap()
                            .items_center()
                            .gap(px(4.0))
                            .children(chips)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(80.0))
                                    .px(px(2.0))
                                    .capture_key_down(cx.listener(
                                        |this, event: &KeyDownEvent, _window, cx| {
                                            if this.handle_key(&event.keystroke.key, cx) {
                                                cx.stop_propagation();
                                            }
                                        },
                                    ))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| this.open(cx)),
                                    )
                                    .child(SimpleInput::new(&self.input).text_size(ui_text_md(cx))),
                            ),
                    ),
            )
            .children(list)
    }
}

#[cfg(test)]
mod tests {
    use super::{ChipHint, ChipItem, ChipSearch, ChipSearchEvent};
    use gpui::prelude::*;
    use gpui::{Context, Entity, TestAppContext, VisualTestContext, Window, div};
    use okena_theme::{DARK_THEME, GlobalThemeProvider};
    use std::cell::RefCell;
    use std::rc::Rc;

    struct TestRoot {
        search: Entity<ChipSearch>,
    }

    impl Render for TestRoot {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(gpui::px(400.0))
                .child(self.search.clone())
                // Somewhere to click that is not the search.
                .child(
                    div()
                        .h(gpui::px(200.0))
                        .debug_selector(|| "outside".to_string()),
                )
        }
    }

    type Events = Rc<RefCell<Vec<ChipSearchEvent>>>;

    fn draw(cx: &mut TestAppContext) -> (Entity<ChipSearch>, Events, &mut VisualTestContext) {
        cx.update(|cx| cx.set_global(GlobalThemeProvider(|_| DARK_THEME)));
        let (root, vcx) = cx.add_window_view(|_window, cx| TestRoot {
            search: cx.new(|cx| ChipSearch::new("s", "Search", cx)),
        });
        let search = root.read_with(vcx, |root, _| root.search.clone());
        let events: Events = Rc::default();
        let sink = events.clone();
        vcx.update(|window, cx| {
            cx.subscribe(&search, move |_, event: &ChipSearchEvent, _| {
                sink.borrow_mut().push(event.clone());
            })
            .detach();
            search.update(cx, |s, cx| s.focus(window, cx));
        });
        vcx.run_until_parked();
        (search, events, vcx)
    }

    fn items() -> Vec<ChipItem> {
        ["shop", "billing", "search"]
            .into_iter()
            .map(|name| ChipItem::new(name, name).kind("Project"))
            .collect()
    }

    fn set_results(
        search: &Entity<ChipSearch>,
        vcx: &mut VisualTestContext,
        results: Vec<ChipItem>,
    ) {
        search.update(vcx, |s, cx| s.set_results(results, cx));
        vcx.run_until_parked();
    }

    fn chip_ids(search: &Entity<ChipSearch>, vcx: &mut VisualTestContext) -> Vec<String> {
        search.read_with(vcx, |s, _| {
            s.chips().iter().map(|c| c.id.to_string()).collect()
        })
    }

    #[gpui::test]
    fn typing_asks_the_host_to_search_and_opens_the_list(cx: &mut TestAppContext) {
        let (search, events, vcx) = draw(cx);
        vcx.simulate_input("sh");
        vcx.run_until_parked();
        assert_eq!(
            events.borrow().last(),
            Some(&ChipSearchEvent::QueryChanged("sh".into()))
        );
        assert!(search.read_with(vcx, |s, _| s.is_open()));
    }

    #[gpui::test]
    fn arrows_move_and_enter_adds_the_highlighted_result(cx: &mut TestAppContext) {
        let (search, events, vcx) = draw(cx);
        vcx.simulate_input("s");
        set_results(&search, vcx, items());

        vcx.simulate_keystrokes("down down up down");
        assert_eq!(
            search.read_with(vcx, |s, _| s.highlighted().map(|h| h.id.to_string())),
            Some("search".into())
        );
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();

        assert_eq!(chip_ids(&search, vcx), ["search"]);
        assert!(
            events
                .borrow()
                .iter()
                .any(|e| matches!(e, ChipSearchEvent::Added(i) if i.id.as_ref() == "search"))
        );
        // The box is cleared for the next search, and the added item is gone
        // from the results.
        assert_eq!(search.read_with(vcx, |s, cx| s.query(cx)), "");
        let visible: Vec<String> = search.read_with(vcx, |s, _| {
            s.visible_results()
                .iter()
                .map(|r| r.id.to_string())
                .collect()
        });
        assert_eq!(visible, ["shop", "billing"]);
    }

    #[gpui::test]
    fn an_added_item_does_not_show_again_in_later_results(cx: &mut TestAppContext) {
        let (search, _, vcx) = draw(cx);
        search.update(vcx, |s, cx| s.set_chips(vec![items()[1].clone()], cx));
        set_results(&search, vcx, items());
        vcx.simulate_keystrokes("down");
        let visible: Vec<String> = search.read_with(vcx, |s, _| {
            s.visible_results()
                .iter()
                .map(|r| r.id.to_string())
                .collect()
        });
        assert_eq!(visible, ["shop", "search"]);
        // Adding an already-picked item twice is a no-op.
        search.update(vcx, |s, cx| s.add(items()[1].clone(), cx));
        assert_eq!(chip_ids(&search, vcx), ["billing"]);
    }

    #[gpui::test]
    fn backspace_removes_the_last_chip_only_from_an_empty_box(cx: &mut TestAppContext) {
        let (search, events, vcx) = draw(cx);
        search.update(vcx, |s, cx| s.set_chips(items()[..2].to_vec(), cx));
        vcx.simulate_input("x");
        vcx.simulate_keystrokes("backspace");
        vcx.run_until_parked();
        // It deleted the character, not a chip.
        assert_eq!(chip_ids(&search, vcx), ["shop", "billing"]);
        vcx.simulate_keystrokes("backspace");
        vcx.run_until_parked();
        assert_eq!(chip_ids(&search, vcx), ["shop"]);
        assert!(
            events
                .borrow()
                .iter()
                .any(|e| matches!(e, ChipSearchEvent::Removed(i) if i.id.as_ref() == "billing"))
        );
    }

    #[gpui::test]
    fn escape_closes_the_list_and_then_lets_escape_through(cx: &mut TestAppContext) {
        let (search, _, vcx) = draw(cx);
        vcx.simulate_input("s");
        set_results(&search, vcx, items());
        assert!(search.read_with(vcx, |s, _| s.is_open()));
        let handled = search.update(vcx, |s, cx| s.handle_key("escape", cx));
        assert!(handled);
        assert!(!search.read_with(vcx, |s, _| s.is_open()));
        // Closed: a second Esc is not ours, so the dialog around can close.
        assert!(!search.update(vcx, |s, cx| s.handle_key("escape", cx)));
        assert!(!search.update(vcx, |s, cx| s.handle_key("enter", cx)));
    }

    #[gpui::test]
    fn a_chips_remove_and_a_hints_action_reach_the_host(cx: &mut TestAppContext) {
        let (search, events, vcx) = draw(cx);
        search.update(vcx, |s, cx| {
            s.set_chips(items(), cx);
            s.set_hints(
                vec![ChipHint {
                    id: "p-shop".into(),
                    text: "shop isn't mapped yet".into(),
                    action: "Scan".into(),
                }],
                cx,
            );
        });
        search.update(vcx, |s, cx| s.remove("shop", cx));
        assert_eq!(chip_ids(&search, vcx), ["billing", "search"]);
        assert!(
            events
                .borrow()
                .iter()
                .any(|e| matches!(e, ChipSearchEvent::Removed(i) if i.id.as_ref() == "shop"))
        );
        // Removing what is not there says nothing.
        let before = events.borrow().len();
        search.update(vcx, |s, cx| s.remove("nope", cx));
        assert_eq!(events.borrow().len(), before);
    }

    #[gpui::test]
    fn a_press_outside_closes_the_list(cx: &mut TestAppContext) {
        let (search, _, vcx) = draw(cx);
        vcx.simulate_input("s");
        set_results(&search, vcx, items());
        assert!(search.read_with(vcx, |s, _| s.is_open()));
        let outside = vcx
            .debug_bounds("outside")
            .expect("outside bounds recorded");
        vcx.simulate_mouse_down(
            outside.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        vcx.run_until_parked();
        assert!(!search.read_with(vcx, |s, _| s.is_open()));
    }

    #[gpui::test]
    fn a_hundred_results_fit_in_a_bounded_scrolling_list(cx: &mut TestAppContext) {
        let (search, _, vcx) = draw(cx);
        let many: Vec<ChipItem> = (0..150)
            .map(|i| ChipItem::new(format!("p{i}"), format!("project {i}")))
            .collect();
        vcx.simulate_input("p");
        set_results(&search, vcx, many);
        // Driven directly rather than as 150 simulated keystrokes: those take
        // long enough for the input's cursor-blink timer to fire on smol's
        // reactor thread, which the test scheduler does not allow.
        search.update(vcx, |s, cx| {
            for _ in 0..150 {
                s.handle_key("down", cx);
            }
        });
        assert_eq!(
            search.read_with(vcx, |s, _| s.highlighted().map(|h| h.id.to_string())),
            Some("p149".into())
        );
        // Past the end it stays on the last.
        search.update(vcx, |s, cx| s.handle_key("down", cx));
        assert_eq!(
            search.read_with(vcx, |s, _| s.highlighted().map(|h| h.id.to_string())),
            Some("p149".into())
        );
    }
}
