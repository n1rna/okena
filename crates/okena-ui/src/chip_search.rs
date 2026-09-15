//! Search, then pick into chips.
//!
//! A search box, the results under it, and what has been picked as removable
//! chips — the one control every launcher uses to choose projects and context
//! (QBL-406). It knows nothing about what it searches: the host listens for
//! [`ChipSearchEvent::QueryChanged`], runs the search however it runs it — a
//! filter over projects, a daemon round trip — and hands the results back with
//! [`ChipSearch::set_results`].
//!
//! The results float beside the box as a launcher panel (QBL-411), drawn over
//! whatever is around it — a dialog's footer, its edge — so opening them never
//! moves the layout. It opens below the box, or above it when the window has
//! more room there, and never grows past a few rows. Results that name a
//! [`ChipGroup`] are listed under a header per group, in the order each group's
//! first result ranks, and with more than one group a row of pills on top
//! narrows the list to one.
//!
//! Built with [`ChipSearch::menu`] it is a menu instead (QBL-414): a button
//! opens the same panel with the search box on top, and picking a result hands
//! it back as [`ChipSearchEvent::Picked`] and closes, keeping no chips.
//!
//! Keyboard: ↑/↓ move through results, Enter adds the highlighted one, Tab and
//! Shift-Tab step through the groups, Backspace in an empty box removes the
//! last chip, Esc closes the list. They are taken in the capture phase, before
//! the text input sees them, and only when they mean something here — an Esc
//! or a Tab with the list closed still reaches the dialog around it.

use crate::simple_input::{InputChangedEvent, SimpleInput, SimpleInputState};
use crate::theme::{theme, with_alpha};
use crate::tokens::{ui_text_md, ui_text_ms, ui_text_sm};
use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::{h_flex, v_flex};

/// Where a result comes from — a project, a store. Results sharing one are
/// listed together under a header naming it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChipGroup {
    /// Identity: results with one group id share a header.
    pub id: SharedString,
    pub name: SharedString,
    /// An icon path drawn before the name, e.g. `icons/folder.svg`.
    pub icon: Option<SharedString>,
}

/// One pickable thing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChipItem {
    /// Identity: two results with one id are the same item.
    pub id: SharedString,
    pub title: SharedString,
    /// What it is, e.g. "Skill": a label on the chip, the icon's tooltip in
    /// the list.
    pub kind: Option<SharedString>,
    /// Muted, after the title in the list.
    pub description: Option<SharedString>,
    /// An icon path drawn at the start of its row, e.g. `icons/map.svg`.
    pub icon: Option<SharedString>,
    /// The header it is listed under.
    pub group: Option<ChipGroup>,
    /// Short labels at the end of its row, e.g. "Spec" or "area:cart".
    pub tags: Vec<SharedString>,
}

impl ChipItem {
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            kind: None,
            description: None,
            icon: None,
            group: None,
            tags: Vec::new(),
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

    pub fn icon(mut self, icon: impl Into<SharedString>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    pub fn group(mut self, group: ChipGroup) -> Self {
        self.group = Some(group);
        self
    }

    /// Add a label at the end of the row. An empty one is skipped.
    pub fn tag(mut self, tag: impl Into<SharedString>) -> Self {
        let tag = tag.into();
        if !tag.is_empty() {
            self.tags.push(tag);
        }
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
    /// A menu's result was picked. The menu has closed.
    Picked(ChipItem),
    /// A hint's action was clicked.
    Hint(SharedString),
}

/// A row of the results: a group's header, or a result under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResultRow<'a> {
    Header(&'a ChipGroup),
    Item(&'a ChipItem),
}

/// The whole panel's height at most: about eight rows with the pills and the
/// footer.
const PANEL_MAX_HEIGHT: f32 = 340.0;
/// The scrolling rows' height at least, however little room the window has.
const MIN_SCROLL_HEIGHT: f32 = 64.0;
/// Between the box and the panel.
const LIST_GAP: f32 = 4.0;
/// Kept clear between the panel and the window's edge.
const WINDOW_MARGIN: f32 = 16.0;
const ROW_HEIGHT: f32 = 32.0;
const HEADER_HEIGHT: f32 = 28.0;
const PILLS_HEIGHT: f32 = 38.0;
const FOOTER_HEIGHT: f32 = 30.0;
/// The panel's border, top and bottom.
const PANEL_BORDER: f32 = 2.0;
/// The icon column every row's text starts after.
const ICON_SLOT: f32 = 16.0;
const ICON_SIZE: f32 = 14.0;
/// A menu's panel width at least: its button is far narrower than a box.
const MENU_WIDTH: f32 = 440.0;
/// The search box on top of a menu's panel.
const MENU_SEARCH_HEIGHT: f32 = 44.0;

/// Whether the panel opens above the box, and how tall its scrolling rows may
/// get. It opens below unless that leaves it short of its full height and
/// above has more room; either way it keeps [`WINDOW_MARGIN`] from the edge.
/// `chrome` is everything in the panel that does not scroll.
fn placement(anchor: Bounds<Pixels>, viewport_height: Pixels, chrome: f32) -> (bool, f32) {
    let below = f32::from(viewport_height - anchor.bottom()) - LIST_GAP - WINDOW_MARGIN;
    let above = f32::from(anchor.top()) - LIST_GAP - WINDOW_MARGIN;
    let up = below < PANEL_MAX_HEIGHT && above > below;
    let room = if up { above } else { below };
    let scroll = (room.min(PANEL_MAX_HEIGHT) - chrome).max(MIN_SCROLL_HEIGHT);
    (up, scroll)
}

pub struct ChipSearch {
    id: SharedString,
    input: Entity<SimpleInputState>,
    results: Vec<ChipItem>,
    hints: Vec<ChipHint>,
    chips: Vec<ChipItem>,
    /// Index into [`Self::visible_results`].
    highlighted: usize,
    open: bool,
    /// The group id the list is narrowed to; `None` for every group.
    origin: Option<SharedString>,
    /// Said in the list when a search found nothing.
    empty_text: Option<SharedString>,
    /// A menu's button label: set, picking opens rather than adds.
    menu: Option<SharedString>,
    /// Where the box (a menu's button) was last painted, which the floating
    /// list hangs from.
    box_bounds: Option<Bounds<Pixels>>,
    /// Where the floating list was last painted: a press there is not outside.
    list_bounds: Option<Bounds<Pixels>>,
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
            // A closed menu emptied its box on a pick: that must not reopen
            // it, and nobody typed into a panel that is not drawn.
            if this.menu.is_some() && !this.open && query.is_empty() {
                cx.emit(ChipSearchEvent::QueryChanged(query));
                return;
            }
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
            origin: None,
            empty_text: None,
            menu: None,
            box_bounds: None,
            list_bounds: None,
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

    /// Make this a menu behind a button labelled `label`: the search box moves
    /// into the panel, and picking a result emits [`ChipSearchEvent::Picked`]
    /// and closes instead of adding a chip.
    pub fn menu(mut self, label: impl Into<SharedString>) -> Self {
        self.menu = Some(label.into());
        self
    }

    /// Relabel a menu's button, e.g. when the count on it changes.
    pub fn set_menu_label(&mut self, label: impl Into<SharedString>, cx: &mut Context<Self>) {
        let label = label.into();
        if self.menu.as_ref() != Some(&label) {
            self.menu = Some(label);
            cx.notify();
        }
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
        self.settle();
        cx.notify();
    }

    pub fn set_results(&mut self, results: Vec<ChipItem>, cx: &mut Context<Self>) {
        self.results = results;
        self.settle();
        cx.notify();
    }

    pub fn set_hints(&mut self, hints: Vec<ChipHint>, cx: &mut Context<Self>) {
        self.hints = hints;
        cx.notify();
    }

    /// Results not already picked, whatever group the list is narrowed to.
    fn unpicked(&self) -> impl Iterator<Item = &ChipItem> {
        self.results
            .iter()
            .filter(|r| !self.chips.iter().any(|c| c.id == r.id))
    }

    /// The groups of the results not already picked, each with how many it
    /// holds, in the order their first result ranks.
    pub fn origins(&self) -> Vec<(&ChipGroup, usize)> {
        let mut origins: Vec<(&ChipGroup, usize)> = Vec::new();
        for group in self.unpicked().filter_map(|item| item.group.as_ref()) {
            match origins.iter_mut().find(|(g, _)| g.id == group.id) {
                Some((_, count)) => *count += 1,
                None => origins.push((group, 1)),
            }
        }
        origins
    }

    /// The group the list is narrowed to.
    pub fn origin(&self) -> Option<&SharedString> {
        self.origin.as_ref()
    }

    /// Narrow the list to one group, or show them all with `None`.
    pub fn set_origin(&mut self, origin: Option<SharedString>, cx: &mut Context<Self>) {
        self.origin = origin;
        self.highlighted = 0;
        self.settle();
        self.scroll.scroll_to_item(self.highlighted_child());
        cx.notify();
    }

    /// Step to the next group, or the previous with `back`, through "every
    /// group" at either end.
    fn cycle_origin(&mut self, back: bool, cx: &mut Context<Self>) {
        let mut stops: Vec<Option<SharedString>> = vec![None];
        stops.extend(self.origins().into_iter().map(|(g, _)| Some(g.id.clone())));
        let at = stops.iter().position(|s| *s == self.origin).unwrap_or(0);
        let next = if back {
            (at + stops.len() - 1) % stops.len()
        } else {
            (at + 1) % stops.len()
        };
        self.set_origin(stops[next].clone(), cx);
    }

    /// Keep the filter and the highlight pointing at something that is still
    /// listed: a group that has left the results goes back to every group.
    fn settle(&mut self) {
        if let Some(origin) = &self.origin
            && !self.origins().iter().any(|(g, _)| &g.id == origin)
        {
            self.origin = None;
        }
        let len = self.visible_results().len();
        self.highlighted = self.highlighted.min(len.saturating_sub(1));
    }

    /// The rows of the list: results not already picked, each group's under
    /// its header. Groups come in the order their first result ranks, and
    /// results keep their order within one. Results with no group are listed
    /// the same way, without a header. Narrowed to one group, only its results
    /// are listed, and the pills name it instead of a header.
    fn rows(&self) -> Vec<ResultRow<'_>> {
        let mut sections: Vec<(Option<&ChipGroup>, Vec<&ChipItem>)> = Vec::new();
        for item in self.unpicked() {
            let group = item.group.as_ref();
            if let Some(origin) = &self.origin
                && group.map(|g| &g.id) != Some(origin)
            {
                continue;
            }
            match sections
                .iter_mut()
                .find(|(g, _)| g.map(|g| &g.id) == group.map(|g| &g.id))
            {
                Some((_, items)) => items.push(item),
                None => sections.push((group, vec![item])),
            }
        }
        let headers = self.origin.is_none();
        sections
            .into_iter()
            .flat_map(|(group, items)| {
                group
                    .filter(|_| headers)
                    .map(ResultRow::Header)
                    .into_iter()
                    .chain(items.into_iter().map(ResultRow::Item))
            })
            .collect()
    }

    /// Results not already picked, in the order the list shows them: an added
    /// item does not show again.
    pub fn visible_results(&self) -> Vec<&ChipItem> {
        self.rows()
            .into_iter()
            .filter_map(|row| match row {
                ResultRow::Item(item) => Some(item),
                ResultRow::Header(_) => None,
            })
            .collect()
    }

    pub fn highlighted(&self) -> Option<&ChipItem> {
        self.visible_results().get(self.highlighted).copied()
    }

    /// The highlighted result's position among the scrolling list's children:
    /// past the hints, and past every header above it.
    fn highlighted_child(&self) -> usize {
        let mut seen = 0;
        let row = self
            .rows()
            .iter()
            .position(|row| {
                if matches!(row, ResultRow::Item(_)) {
                    seen += 1;
                }
                seen == self.highlighted + 1
            })
            .unwrap_or(0);
        self.hints.len() + row
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
        self.settle();
        cx.notify();
    }

    /// What choosing a result does: a menu hands it back and closes, a chip
    /// search adds it.
    fn pick(&mut self, item: ChipItem, cx: &mut Context<Self>) {
        if self.menu.is_none() {
            self.add(item, cx);
            return;
        }
        self.open = false;
        cx.emit(ChipSearchEvent::Picked(item));
        self.input.update(cx, |input, cx| input.set_value("", cx));
        cx.notify();
    }

    pub fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(at) = self.chips.iter().position(|c| c.id.as_ref() == id) {
            let item = self.chips.remove(at);
            cx.emit(ChipSearchEvent::Removed(item));
            cx.notify();
        }
    }

    /// Act on `key`, Shift held or not. `true` when it meant something here,
    /// so it goes no further.
    fn handle_key(&mut self, key: &str, shift: bool, cx: &mut Context<Self>) -> bool {
        let len = self.visible_results().len();
        match key {
            "down" => {
                if !self.open {
                    self.open(cx);
                } else if self.highlighted + 1 < len {
                    self.highlighted += 1;
                    self.scroll.scroll_to_item(self.highlighted_child());
                }
                cx.notify();
                true
            }
            "up" if self.open => {
                if self.highlighted > 0 {
                    self.highlighted -= 1;
                    self.scroll.scroll_to_item(self.highlighted_child());
                    cx.notify();
                }
                true
            }
            "tab" if self.open && self.origins().len() > 1 => {
                self.cycle_origin(shift, cx);
                true
            }
            "enter" if self.open && len > 0 => {
                if let Some(item) = self.highlighted().cloned() {
                    self.pick(item, cx);
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

    /// The fixed column a row's text starts after, with `icon` in it.
    fn icon_slot(icon: Option<SharedString>, color: u32) -> Div {
        div()
            .flex_shrink_0()
            .w(px(ICON_SLOT))
            .h(px(ICON_SLOT))
            .flex()
            .items_center()
            .justify_center()
            .children(icon.map(|path| svg().path(path).size(px(ICON_SIZE)).text_color(rgb(color))))
    }

    /// The pills that narrow the list to one group: every group first, then
    /// each group with how many results it holds.
    fn render_pills(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let total = self.unpicked().count();
        let mut stops: Vec<(
            Option<SharedString>,
            SharedString,
            Option<SharedString>,
            usize,
        )> = vec![(None, "All".into(), None, total)];
        stops.extend(
            self.origins()
                .into_iter()
                .map(|(g, count)| (Some(g.id.clone()), g.name.clone(), g.icon.clone(), count)),
        );
        h_flex()
            .id(SharedString::from(format!("{}-origins", self.id)))
            .flex_shrink_0()
            .w_full()
            .h(px(PILLS_HEIGHT))
            .px(px(8.0))
            .gap(px(4.0))
            .items_center()
            .overflow_x_scroll()
            .border_b_1()
            .border_color(rgb(t.border))
            .children(stops.into_iter().map(|(origin, name, icon, count)| {
                let active = origin == self.origin;
                let selector = format!("{}-origin-{}", self.id, origin.as_deref().unwrap_or("all"));
                h_flex()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .flex_shrink_0()
                    .h(px(24.0))
                    .px(px(8.0))
                    .gap(px(5.0))
                    .items_center()
                    .rounded(px(12.0))
                    .cursor_pointer()
                    .text_size(ui_text_ms(cx))
                    .when(active, |d| {
                        d.bg(rgb(t.bg_selection)).text_color(rgb(t.text_primary))
                    })
                    .when(!active, |d| {
                        d.text_color(rgb(t.text_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                    })
                    .children(icon.map(|path| {
                        svg().path(path).size(px(12.0)).text_color(rgb(if active {
                            t.text_primary
                        } else {
                            t.text_muted
                        }))
                    }))
                    .child(div().max_w(px(180.0)).truncate().child(name))
                    .child(
                        div()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_muted))
                            .child(count.to_string()),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.set_origin(origin.clone(), cx);
                        }),
                    )
            }))
            .into_any_element()
    }

    fn render_header(
        &self,
        group: &ChipGroup,
        with_icons: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        h_flex()
            .w_full()
            .min_w_0()
            .h(px(HEADER_HEIGHT))
            .flex_shrink_0()
            .items_end()
            .pb(px(4.0))
            // Lined up with the rows' icons and text.
            .px(px(14.0))
            .gap(px(10.0))
            .when(with_icons, |d| {
                d.child(Self::icon_slot(group.icon.clone(), t.text_muted))
            })
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_sm(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_muted))
                    .child(group.name.clone()),
            )
            .into_any_element()
    }

    fn render_item(
        &self,
        item: &ChipItem,
        highlighted: bool,
        with_icons: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let picked = item.clone();
        let row_id = format!("{}-result-{}", self.id, item.id);
        let kind = item.kind.clone();
        h_flex()
            .id(SharedString::from(row_id.clone()))
            .debug_selector(move || row_id.clone())
            .flex_shrink_0()
            .h(px(ROW_HEIGHT))
            .mx(px(6.0))
            .px(px(8.0))
            .gap(px(10.0))
            .items_center()
            .min_w_0()
            .rounded(px(6.0))
            .cursor_pointer()
            .when(highlighted, |d| d.bg(rgb(t.bg_selection)))
            .when(!highlighted, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
            .when(with_icons, |d| {
                d.child(
                    Self::icon_slot(
                        item.icon.clone(),
                        if highlighted {
                            t.text_primary
                        } else {
                            t.text_secondary
                        },
                    )
                    .id(SharedString::from(format!(
                        "{}-result-{}-kind",
                        self.id, item.id
                    )))
                    .when_some(kind, |d, kind| {
                        d.tooltip(move |window, cx| Tooltip::new(kind.clone()).build(window, cx))
                    }),
                )
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(8.0))
                    .items_baseline()
                    .child(
                        div()
                            .flex_shrink_0()
                            .max_w(relative(0.6))
                            .truncate()
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(t.text_primary))
                            .child(item.title.clone()),
                    )
                    .children(item.description.clone().map(|description| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(description)
                    })),
            )
            .children(item.tags.iter().map(|tag| {
                div()
                    .flex_shrink_0()
                    .max_w(px(160.0))
                    .truncate()
                    .px(px(6.0))
                    .py(px(1.0))
                    .rounded(px(4.0))
                    .bg(with_alpha(t.text_primary, 0.06))
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(tag.clone())
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.pick(picked.clone(), cx);
                }),
            )
            .into_any_element()
    }

    /// What the keys do, along the panel's bottom.
    fn render_footer(&self, with_origins: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let hint = |key: &'static str, label: &'static str| {
            h_flex()
                .gap(px(5.0))
                .items_center()
                .child(
                    div()
                        .px(px(5.0))
                        .rounded(px(3.0))
                        .bg(with_alpha(t.text_primary, 0.08))
                        .text_color(rgb(t.text_secondary))
                        .child(key),
                )
                .child(label)
        };
        h_flex()
            .flex_shrink_0()
            .w_full()
            .h(px(FOOTER_HEIGHT))
            .px(px(12.0))
            .gap(px(14.0))
            .items_center()
            .border_t_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_secondary))
            .text_size(ui_text_sm(cx))
            .text_color(rgb(t.text_muted))
            .child(hint("↑↓", "Navigate"))
            .child(hint("↵", if self.menu.is_some() { "Open" } else { "Add" }))
            .when(with_origins, |d| d.child(hint("⇥", "Origin")))
            .child(hint("esc", "Close"))
            .into_any_element()
    }

    fn render_list(&mut self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rows: Vec<ResultRow> = self.rows();
        let empty = rows.is_empty() && self.hints.is_empty();
        let anchor = self.box_bounds;
        // A menu opens even on nothing: its search box is in the panel.
        let silent = empty && self.empty_text.is_none() && self.menu.is_none();
        let (true, false, Some(anchor)) = (self.open, silent, anchor) else {
            self.list_bounds = None;
            return None;
        };
        let t = theme(cx);
        let with_icons = rows.iter().any(|row| match row {
            ResultRow::Header(group) => group.icon.is_some(),
            ResultRow::Item(item) => item.icon.is_some(),
        });
        let with_origins = self.origins().len() > 1;
        let search = self.menu.is_some().then(|| {
            div()
                .flex_shrink_0()
                .w_full()
                .h(px(MENU_SEARCH_HEIGHT))
                .px(px(8.0))
                .flex()
                .items_center()
                .border_b_1()
                .border_color(rgb(t.border))
                .child(
                    crate::input::input_container(&t, Some(self.is_input_focused(window, cx)))
                        .w_full()
                        .px(px(6.0))
                        .py(px(4.0))
                        .child(self.render_input(cx)),
                )
        });
        let width = if self.menu.is_some() {
            anchor.size.width.max(px(MENU_WIDTH))
        } else {
            anchor.size.width
        };
        let chrome = PANEL_BORDER
            + FOOTER_HEIGHT
            + if with_origins { PILLS_HEIGHT } else { 0.0 }
            + if search.is_some() { MENU_SEARCH_HEIGHT } else { 0.0 };
        let (up, scroll_height) = placement(anchor, window.viewport_size().height, chrome);

        let mut list = v_flex()
            .id(SharedString::from(format!("{}-results", self.id)))
            .w_full()
            .max_h(px(scroll_height))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .py(px(4.0));

        for hint in &self.hints {
            let hint_id = hint.id.clone();
            list = list.child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(14.0))
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
                    .px(px(14.0))
                    .py(px(8.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(text),
            );
        }

        let mut index = 0;
        for row in rows {
            list = list.child(match row {
                ResultRow::Header(group) => self.render_header(group, with_icons, cx),
                ResultRow::Item(item) => {
                    let highlighted = index == self.highlighted;
                    index += 1;
                    self.render_item(item, highlighted, with_icons, cx)
                }
            });
        }

        let this = cx.entity().downgrade();
        let panel = v_flex()
            .id(SharedString::from(format!("{}-results-panel", self.id)))
            .occlude()
            .w(width)
            .rounded(px(10.0))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_primary))
            .shadow_xl()
            .overflow_hidden()
            // The list scrolls on its own: without this the wheel also scrolled
            // the dialog under it.
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(search)
            .when(with_origins, |d| d.child(self.render_pills(cx)))
            .child(list)
            .child(self.render_footer(with_origins, cx));
        // Measured around the panel, not inside it, so its border counts.
        let panel = div().relative().child(panel).child(
            canvas(
                move |bounds, _, cx| {
                    if let Some(this) = this.upgrade() {
                        this.update(cx, |this, _| this.list_bounds = Some(bounds));
                    }
                },
                |_, _, _, _| {},
            )
            // Pinned to the corner: left to flow, it sat under the panel.
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        );

        let anchored = if up {
            anchored()
                .anchor(Anchor::BottomLeft)
                .position(point(anchor.origin.x, anchor.top() - px(LIST_GAP)))
        } else {
            anchored().position(point(anchor.origin.x, anchor.bottom() + px(LIST_GAP)))
        };
        Some(
            deferred(
                anchored
                    .snap_to_window_with_margin(px(LIST_GAP))
                    .child(panel),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

impl ChipSearch {
    fn is_input_focused(&self, window: &Window, cx: &App) -> bool {
        self.input.read(cx).focus_handle(cx).is_focused(window)
    }

    /// The text input, taking the keys that mean something to the list.
    fn render_input(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex_1()
            .min_w(px(80.0))
            .px(px(2.0))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                let keystroke = &event.keystroke;
                if this.handle_key(&keystroke.key, keystroke.modifiers.shift, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.open(cx)),
            )
            .child(SimpleInput::new(&self.input).text_size(ui_text_md(cx)))
    }

    /// Where the list hangs from, measured around the box (or a menu's
    /// button) so its border counts. Rendered again only when that moved or
    /// resized, which is rare and settles at once.
    fn anchor_canvas(&self, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        canvas(
            move |bounds, _, cx| {
                if let Some(this) = this.upgrade() {
                    this.update(cx, |this, cx| {
                        if this.box_bounds != Some(bounds) {
                            this.box_bounds = Some(bounds);
                            if this.open {
                                cx.notify();
                            }
                        }
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
    }

    /// A press anywhere outside the box and its list closes the list. The
    /// list floats outside the element this is on, so a press on it is let
    /// through.
    fn close_on_press_outside(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
        cx.listener(|this, event: &MouseDownEvent, _, cx| {
            if this
                .list_bounds
                .is_some_and(|bounds| bounds.contains(&event.position))
            {
                return;
            }
            this.close(cx);
        })
    }

    /// A menu's button: it opens the panel, with the search box focused.
    fn render_menu_button(
        &mut self,
        label: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let open = self.open;
        let list = self.render_list(window, cx);
        let selector = format!("{}-trigger", self.id);
        div()
            .id(self.id.clone())
            .flex_shrink_0()
            .on_mouse_down_out(self.close_on_press_outside(cx))
            .child(
                div()
                    .relative()
                    .child(
                        h_flex()
                            .id(SharedString::from(selector.clone()))
                            .debug_selector(move || selector.clone())
                            .cursor_pointer()
                            .h(px(24.0))
                            .px(px(10.0))
                            .gap(px(6.0))
                            .items_center()
                            .rounded(px(4.0))
                            .bg(rgb(if open { t.bg_selection } else { t.bg_secondary }))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_primary))
                            .child(label)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    cx.stop_propagation();
                                    if this.open {
                                        this.close(cx);
                                    } else {
                                        this.open(cx);
                                        this.focus(window, cx);
                                    }
                                }),
                            ),
                    )
                    .child(self.anchor_canvas(cx)),
            )
            .children(list)
            .into_any_element()
    }
}

impl Render for ChipSearch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(label) = self.menu.clone() {
            return self.render_menu_button(label, window, cx);
        }
        let t = theme(cx);
        let focused = self.is_input_focused(window, cx);
        let chips: Vec<AnyElement> = self
            .chips
            .clone()
            .iter()
            .map(|item| self.render_chip(item, cx))
            .collect();
        let list = self.render_list(window, cx);

        v_flex()
            .id(self.id.clone())
            .w_full()
            .min_w_0()
            .on_mouse_down_out(self.close_on_press_outside(cx))
            .child(
                div()
                    .relative()
                    .w_full()
                    .min_w_0()
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
                                    .child(self.render_input(cx)),
                            ),
                    )
                    .child(self.anchor_canvas(cx)),
            )
            .children(list)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChipGroup, ChipHint, ChipItem, ChipSearch, ChipSearchEvent, LIST_GAP, PANEL_MAX_HEIGHT,
        ResultRow, WINDOW_MARGIN, placement,
    };
    use gpui::prelude::*;
    use gpui::{
        Bounds, Context, Entity, TestAppContext, VisualTestContext, Window, div, point, px, size,
    };
    use okena_theme::{DARK_THEME, GlobalThemeProvider};
    use std::cell::RefCell;
    use std::rc::Rc;

    struct TestRoot {
        search: Entity<ChipSearch>,
        /// Height of the space above the search.
        above: f32,
    }

    impl Render for TestRoot {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(gpui::px(400.0))
                // Somewhere to click that is not the search, above it: the
                // list floats over what is below.
                .child(
                    div()
                        .h(gpui::px(self.above))
                        .debug_selector(|| "outside".to_string()),
                )
                .child(self.search.clone())
        }
    }

    type Events = Rc<RefCell<Vec<ChipSearchEvent>>>;

    fn draw(cx: &mut TestAppContext) -> (Entity<ChipSearch>, Events, &mut VisualTestContext) {
        draw_with_space_above(cx, None)
    }

    /// With `Some(from_bottom)`, the search sits that far above the window's
    /// bottom edge; otherwise near the top.
    fn draw_with_space_above(
        cx: &mut TestAppContext,
        from_bottom: Option<f32>,
    ) -> (Entity<ChipSearch>, Events, &mut VisualTestContext) {
        cx.update(|cx| cx.set_global(GlobalThemeProvider(|_| DARK_THEME)));
        let (root, vcx) = cx.add_window_view(|_window, cx| TestRoot {
            search: cx.new(|cx| ChipSearch::new("s", "Search", cx)),
            above: 100.0,
        });
        if let Some(from_bottom) = from_bottom {
            let height = vcx.update(|window, _| f32::from(window.viewport_size().height));
            root.update(vcx, |root, cx| {
                root.above = height - from_bottom;
                cx.notify();
            });
        }
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

    fn group(id: &str) -> ChipGroup {
        ChipGroup {
            id: id.to_string().into(),
            name: id.to_uppercase().into(),
            icon: Some("icons/folder.svg".into()),
        }
    }

    /// Ranked best first, owners interleaved: shop, then the knowledge store,
    /// then shop again, then billing.
    fn grouped_items() -> Vec<ChipItem> {
        [
            ("shop-map", "shop"),
            ("kb-doc", "kb"),
            ("shop-spec", "shop"),
            ("billing-map", "billing"),
            ("kb-skill", "kb"),
        ]
        .into_iter()
        .map(|(id, owner)| {
            ChipItem::new(id, id)
                .icon("icons/map.svg")
                .group(group(owner))
                .tag("Spec")
        })
        .collect()
    }

    fn row_names(search: &Entity<ChipSearch>, vcx: &mut VisualTestContext) -> Vec<String> {
        search.read_with(vcx, |s, _| {
            s.rows()
                .into_iter()
                .map(|row| match row {
                    ResultRow::Header(g) => format!("# {}", g.name),
                    ResultRow::Item(i) => i.id.to_string(),
                })
                .collect()
        })
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

    fn highlighted_id(search: &Entity<ChipSearch>, vcx: &mut VisualTestContext) -> Option<String> {
        search.read_with(vcx, |s, _| s.highlighted().map(|h| h.id.to_string()))
    }

    fn origin(search: &Entity<ChipSearch>, vcx: &mut VisualTestContext) -> Option<String> {
        search.read_with(vcx, |s, _| s.origin().map(|o| o.to_string()))
    }

    fn click(vcx: &mut VisualTestContext, selector: &'static str) {
        let bounds = vcx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} was painted"));
        vcx.simulate_mouse_down(
            bounds.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        vcx.run_until_parked();
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
        assert_eq!(highlighted_id(&search, vcx), Some("search".into()));
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
    fn results_are_grouped_under_headers_in_ranked_order(cx: &mut TestAppContext) {
        let (search, _, vcx) = draw(cx);
        vcx.simulate_input("s");
        set_results(&search, vcx, grouped_items());
        // Owners in the order their first item ranks; items in ranked order
        // within their owner.
        assert_eq!(
            row_names(&search, vcx),
            [
                "# SHOP",
                "shop-map",
                "shop-spec",
                "# KB",
                "kb-doc",
                "kb-skill",
                "# BILLING",
                "billing-map",
            ]
        );
        // Results without a group get no header.
        set_results(&search, vcx, items());
        assert_eq!(row_names(&search, vcx), ["shop", "billing", "search"]);
    }

    #[gpui::test]
    fn arrows_skip_headers_and_enter_adds_the_highlighted_item(cx: &mut TestAppContext) {
        let (search, events, vcx) = draw(cx);
        vcx.simulate_input("s");
        set_results(&search, vcx, grouped_items());
        search.update(vcx, |s, cx| {
            s.set_hints(
                vec![ChipHint {
                    id: "p-shop".into(),
                    text: "shop isn't mapped yet".into(),
                    action: "Scan".into(),
                }],
                cx,
            )
        });
        assert_eq!(highlighted_id(&search, vcx), Some("shop-map".into()));

        // Across the KB header, and back over it.
        vcx.simulate_keystrokes("down down");
        assert_eq!(highlighted_id(&search, vcx), Some("kb-doc".into()));
        // Scrolled to kb-doc's own child: the hint, two headers and two items
        // come before it.
        assert_eq!(search.read_with(vcx, |s, _| s.highlighted_child()), 5);
        vcx.simulate_keystrokes("up");
        assert_eq!(highlighted_id(&search, vcx), Some("shop-spec".into()));
        vcx.simulate_keystrokes("down down down down down");
        // The last item, never the header past kb-skill.
        assert_eq!(highlighted_id(&search, vcx), Some("billing-map".into()));
        assert_eq!(search.read_with(vcx, |s, _| s.highlighted_child()), 8);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        assert_eq!(chip_ids(&search, vcx), ["billing-map"]);
        assert!(
            events
                .borrow()
                .iter()
                .any(|e| matches!(e, ChipSearchEvent::Added(i) if i.id.as_ref() == "billing-map"))
        );
    }

    #[gpui::test]
    fn a_group_goes_when_its_last_visible_item_is_added(cx: &mut TestAppContext) {
        let (search, _, vcx) = draw(cx);
        vcx.simulate_input("s");
        set_results(&search, vcx, grouped_items());
        let billing = grouped_items()[3].clone();
        search.update(vcx, |s, cx| s.add(billing, cx));
        vcx.run_until_parked();
        assert_eq!(
            row_names(&search, vcx),
            [
                "# SHOP",
                "shop-map",
                "shop-spec",
                "# KB",
                "kb-doc",
                "kb-skill"
            ]
        );
        // A group with an item left keeps its header.
        let doc = grouped_items()[1].clone();
        search.update(vcx, |s, cx| s.add(doc, cx));
        assert_eq!(
            row_names(&search, vcx),
            ["# SHOP", "shop-map", "shop-spec", "# KB", "kb-skill"]
        );
    }

    #[gpui::test]
    fn origins_are_counted_and_one_narrows_the_list(cx: &mut TestAppContext) {
        let (search, _, vcx) = draw(cx);
        vcx.simulate_input("s");
        set_results(&search, vcx, grouped_items());
        let origins: Vec<(String, usize)> = search.read_with(vcx, |s, _| {
            s.origins()
                .into_iter()
                .map(|(g, n)| (g.id.to_string(), n))
                .collect()
        });
        assert_eq!(
            origins,
            [("shop".into(), 2), ("kb".into(), 2), ("billing".into(), 1)]
        );

        search.update(vcx, |s, cx| s.set_origin(Some("kb".into()), cx));
        // Only its results, and no header: the pills already name it.
        assert_eq!(row_names(&search, vcx), ["kb-doc", "kb-skill"]);
        assert_eq!(highlighted_id(&search, vcx), Some("kb-doc".into()));
        vcx.simulate_keystrokes("down enter");
        vcx.run_until_parked();
        assert_eq!(chip_ids(&search, vcx), ["kb-skill"]);
        // Counts leave picked items out.
        let kb = search.read_with(vcx, |s, _| {
            s.origins()
                .into_iter()
                .find(|(g, _)| g.id.as_ref() == "kb")
                .map(|(_, n)| n)
        });
        assert_eq!(kb, Some(1));
    }

    #[gpui::test]
    fn tab_steps_through_origins_and_wraps(cx: &mut TestAppContext) {
        let (search, _, vcx) = draw(cx);
        vcx.simulate_input("s");
        set_results(&search, vcx, grouped_items());
        vcx.simulate_keystrokes("tab");
        assert_eq!(origin(&search, vcx), Some("shop".into()));
        vcx.simulate_keystrokes("tab tab");
        assert_eq!(origin(&search, vcx), Some("billing".into()));
        vcx.simulate_keystrokes("tab");
        assert_eq!(origin(&search, vcx), None);
        vcx.simulate_keystrokes("shift-tab");
        assert_eq!(origin(&search, vcx), Some("billing".into()));
        assert_eq!(highlighted_id(&search, vcx), Some("billing-map".into()));

        // Closed, or with one origin, Tab is not ours.
        search.update(vcx, |s, cx| s.close(cx));
        assert!(!search.update(vcx, |s, cx| s.handle_key("tab", false, cx)));
        search.update(vcx, |s, cx| s.open(cx));
        set_results(&search, vcx, items());
        assert!(!search.update(vcx, |s, cx| s.handle_key("tab", false, cx)));
    }

    #[gpui::test]
    fn the_filter_falls_back_to_every_origin_when_its_own_leaves(cx: &mut TestAppContext) {
        let (search, _, vcx) = draw(cx);
        vcx.simulate_input("s");
        set_results(&search, vcx, grouped_items());

        // Its last item picked.
        search.update(vcx, |s, cx| s.set_origin(Some("billing".into()), cx));
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        assert_eq!(chip_ids(&search, vcx), ["billing-map"]);
        assert_eq!(origin(&search, vcx), None);
        assert_eq!(row_names(&search, vcx)[0], "# SHOP");

        // A new search without it.
        search.update(vcx, |s, cx| s.set_origin(Some("kb".into()), cx));
        let shop_only: Vec<ChipItem> = grouped_items()
            .into_iter()
            .filter(|i| i.id.starts_with("shop"))
            .collect();
        set_results(&search, vcx, shop_only);
        assert_eq!(origin(&search, vcx), None);
        assert_eq!(row_names(&search, vcx), ["# SHOP", "shop-map", "shop-spec"]);
    }

    #[gpui::test]
    fn clicking_an_origin_pill_narrows_the_floating_list(cx: &mut TestAppContext) {
        let (search, _, vcx) = draw(cx);
        search.update(vcx, |s, cx| s.open(cx));
        set_results(&search, vcx, grouped_items());
        click(vcx, "s-origin-kb");
        assert_eq!(origin(&search, vcx), Some("kb".into()));
        assert!(search.read_with(vcx, |s, _| s.is_open()));
        click(vcx, "s-origin-all");
        assert_eq!(origin(&search, vcx), None);
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
        let handled = search.update(vcx, |s, cx| s.handle_key("escape", false, cx));
        assert!(handled);
        assert!(!search.read_with(vcx, |s, _| s.is_open()));
        // Closed: a second Esc is not ours, so the dialog around can close.
        assert!(!search.update(vcx, |s, cx| s.handle_key("escape", false, cx)));
        assert!(!search.update(vcx, |s, cx| s.handle_key("enter", false, cx)));
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
    fn a_press_outside_closes_the_floating_list(cx: &mut TestAppContext) {
        let (search, _, vcx) = draw(cx);
        vcx.simulate_input("s");
        set_results(&search, vcx, grouped_items());
        assert!(search.read_with(vcx, |s, _| s.is_open()));
        // It is drawn, and outside the search's own bounds.
        let list = search
            .read_with(vcx, |s, _| s.list_bounds)
            .expect("the floating list was painted");
        let boxed = search
            .read_with(vcx, |s, _| s.box_bounds)
            .expect("the box was painted");
        assert!(list.top() >= boxed.bottom());
        // Hung right under the box, as wide as it.
        assert!(list.top() - boxed.bottom() <= gpui::px(super::LIST_GAP + 1.0));
        assert_eq!(list.size.width, boxed.size.width);
        click(vcx, "outside");
        assert!(!search.read_with(vcx, |s, _| s.is_open()));
        assert!(search.read_with(vcx, |s, _| s.list_bounds.is_none()));
    }

    #[gpui::test]
    fn clicking_a_floating_row_adds_it_and_keeps_the_list_open(cx: &mut TestAppContext) {
        let (search, _, vcx) = draw(cx);
        // Opened with an empty box: typed text would reopen the list when the
        // add clears it, hiding a press on the row closing it.
        search.update(vcx, |s, cx| s.open(cx));
        set_results(&search, vcx, grouped_items());
        click(vcx, "s-result-kb-doc");
        assert_eq!(chip_ids(&search, vcx), ["kb-doc"]);
        assert!(search.read_with(vcx, |s, _| s.is_open()));
    }

    /// A menu behind a button, drawn near the top of the window.
    fn draw_menu(cx: &mut TestAppContext) -> (Entity<ChipSearch>, Events, &mut VisualTestContext) {
        cx.update(|cx| cx.set_global(GlobalThemeProvider(|_| DARK_THEME)));
        let (root, vcx) = cx.add_window_view(|_window, cx| TestRoot {
            search: cx.new(|cx| ChipSearch::new("s", "Search", cx).menu("Browse · 5")),
            above: 100.0,
        });
        let search = root.read_with(vcx, |root, _| root.search.clone());
        let events: Events = Rc::default();
        let sink = events.clone();
        vcx.update(|_, cx| {
            cx.subscribe(&search, move |_, event: &ChipSearchEvent, _| {
                sink.borrow_mut().push(event.clone());
            })
            .detach();
        });
        vcx.run_until_parked();
        (search, events, vcx)
    }

    fn picked(events: &Events) -> Vec<String> {
        events
            .borrow()
            .iter()
            .filter_map(|e| match e {
                ChipSearchEvent::Picked(i) => Some(i.id.to_string()),
                _ => None,
            })
            .collect()
    }

    #[gpui::test]
    fn a_menu_opens_from_its_button_and_enter_emits_the_pick_and_closes(
        cx: &mut TestAppContext,
    ) {
        let (search, events, vcx) = draw_menu(cx);
        assert!(!search.read_with(vcx, |s, _| s.is_open()));
        click(vcx, "s-trigger");
        assert!(search.read_with(vcx, |s, _| s.is_open()));
        set_results(&search, vcx, grouped_items());

        // Typed into the panel's own box, which the button focused.
        vcx.simulate_input("s");
        vcx.run_until_parked();
        assert!(search.read_with(vcx, |s, _| s.is_open()));
        vcx.simulate_keystrokes("down enter");
        vcx.run_until_parked();

        assert_eq!(picked(&events), ["shop-spec"]);
        assert!(chip_ids(&search, vcx).is_empty(), "a menu keeps no chips");
        assert!(
            !events
                .borrow()
                .iter()
                .any(|e| matches!(e, ChipSearchEvent::Added(_))),
            "and adds nothing"
        );
        assert!(!search.read_with(vcx, |s, _| s.is_open()));
        assert!(search.read_with(vcx, |s, _| s.list_bounds.is_none()));
        assert_eq!(search.read_with(vcx, |s, cx| s.query(cx)), "");
        // Picked, the item is still listed the next time it opens.
        click(vcx, "s-trigger");
        let visible = search.read_with(vcx, |s, _| s.visible_results().len());
        assert_eq!(visible, grouped_items().len());
    }

    #[gpui::test]
    fn clicking_a_menu_row_emits_the_pick_and_closes(cx: &mut TestAppContext) {
        let (search, events, vcx) = draw_menu(cx);
        click(vcx, "s-trigger");
        set_results(&search, vcx, grouped_items());
        click(vcx, "s-result-kb-doc");
        assert_eq!(picked(&events), ["kb-doc"]);
        assert!(chip_ids(&search, vcx).is_empty());
        assert!(!search.read_with(vcx, |s, _| s.is_open()));
    }

    #[gpui::test]
    fn a_menus_panel_is_wider_than_its_button_and_stays_in_the_window(
        cx: &mut TestAppContext,
    ) {
        let (search, _, vcx) = draw_menu(cx);
        click(vcx, "s-trigger");
        set_results(&search, vcx, grouped_items());
        let list = search
            .read_with(vcx, |s, _| s.list_bounds)
            .expect("the menu's panel was painted");
        let button = search
            .read_with(vcx, |s, _| s.box_bounds)
            .expect("the button was painted");
        assert!(list.size.width > button.size.width);
        assert!(list.top() >= button.bottom());
        let viewport = vcx.update(|window, _| window.viewport_size());
        assert!(list.right() <= viewport.width, "{list:?} in {viewport:?}");
        assert!(list.bottom() <= viewport.height);
        // Escape closes it; a press on the button again toggles it.
        search.update(vcx, |s, cx| s.handle_key("escape", false, cx));
        assert!(!search.read_with(vcx, |s, _| s.is_open()));
        click(vcx, "s-trigger");
        assert!(search.read_with(vcx, |s, _| s.is_open()));
        click(vcx, "s-trigger");
        assert!(!search.read_with(vcx, |s, _| s.is_open()));
    }

    #[test]
    fn the_panel_opens_below_unless_above_has_more_room() {
        let viewport = px(1000.0);
        let chrome = 70.0;
        let at = |top: f32| Bounds::new(point(px(0.0), px(top)), size(px(400.0), px(32.0)));

        // Plenty of room below: below, at its full height.
        let (up, scroll) = placement(at(100.0), viewport, chrome);
        assert!(!up);
        assert_eq!(scroll, PANEL_MAX_HEIGHT - chrome);

        // Near the bottom, with more room above: above.
        let (up, scroll) = placement(at(850.0), viewport, chrome);
        assert!(up);
        assert_eq!(scroll, PANEL_MAX_HEIGHT - chrome);

        // Short both ways: the roomier side, shrunk to stay off the edge.
        let short = px(300.0);
        let (up, scroll) = placement(at(60.0), short, chrome);
        let below = 300.0 - 92.0 - LIST_GAP - WINDOW_MARGIN;
        assert!(!up);
        assert_eq!(scroll, below - chrome);
    }

    #[gpui::test]
    fn the_list_opens_above_a_box_near_the_windows_bottom(cx: &mut TestAppContext) {
        let (search, _, vcx) = draw_with_space_above(cx, Some(120.0));
        let many: Vec<ChipItem> = (0..40)
            .map(|i| ChipItem::new(format!("p{i}"), format!("project {i}")))
            .collect();
        search.update(vcx, |s, cx| s.open(cx));
        set_results(&search, vcx, many);
        let list = search
            .read_with(vcx, |s, _| s.list_bounds)
            .expect("the floating list was painted");
        let boxed = search
            .read_with(vcx, |s, _| s.box_bounds)
            .expect("the box was painted");
        assert!(list.bottom() <= boxed.top(), "{list:?} above {boxed:?}");
        assert!(f32::from(list.size.height) <= PANEL_MAX_HEIGHT + 0.5);
    }

    #[gpui::test]
    fn a_hundred_results_fit_in_a_bounded_scrolling_list(cx: &mut TestAppContext) {
        let (search, _, vcx) = draw(cx);
        let many: Vec<ChipItem> = (0..150)
            .map(|i| ChipItem::new(format!("p{i}"), format!("project {i}")))
            .collect();
        vcx.simulate_input("p");
        set_results(&search, vcx, many);
        // Never taller than the cap, and clear of the window's bottom.
        let list = search
            .read_with(vcx, |s, _| s.list_bounds)
            .expect("the floating list was painted");
        let height = vcx.update(|window, _| window.viewport_size().height);
        assert!(f32::from(list.size.height) <= PANEL_MAX_HEIGHT + 0.5);
        assert!(f32::from(height - list.bottom()) >= WINDOW_MARGIN - 0.5);
        // Driven directly rather than as 150 simulated keystrokes: those take
        // long enough for the input's cursor-blink timer to fire on smol's
        // reactor thread, which the test scheduler does not allow.
        search.update(vcx, |s, cx| {
            for _ in 0..150 {
                s.handle_key("down", false, cx);
            }
        });
        assert_eq!(highlighted_id(&search, vcx), Some("p149".into()));
        // Past the end it stays on the last.
        search.update(vcx, |s, cx| s.handle_key("down", false, cx));
        assert_eq!(highlighted_id(&search, vcx), Some("p149".into()));
    }
}
