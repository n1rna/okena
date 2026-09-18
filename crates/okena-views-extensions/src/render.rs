//! Drawing the components of `ui-v1`: text, badges, tables, trees, detail
//! panes, charts, stats, action buttons, and the layout nodes around them.

use std::collections::{HashMap, HashSet};

use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use lyon::tessellation::StrokeOptions;
use okena_core::extension::{
    ColumnKind, ExtBadge, ExtChart, ExtDetail, ExtField, ExtInput, ExtNode, ExtStat, ExtTable,
    ExtTree, ExtView, InputKind, TextStyle, Tone,
};
use okena_ui::button::button;
use okena_ui::expand::expand_toggle;
use okena_ui::input::input_container;
use okena_ui::simple_input::SimpleInput;
use okena_ui::theme::{ThemeColors, theme, with_alpha};
use okena_ui::toggle::toggle_switch;
use okena_ui::tokens::{ui_text, ui_text_md, ui_text_ms, ui_text_sm};
use okena_workspace::extensions_state::ClientExtension;

use crate::AgentBadge;
use crate::pane::{ExtensionPane, Field};
use crate::table::{TableState, arrange, default_sort};

/// Rows beyond this are not drawn; the filter narrows them down.
const MAX_DRAWN_ROWS: usize = 500;

pub fn tone_color(tone: Tone, t: &ThemeColors) -> u32 {
    match tone {
        Tone::Success => t.success,
        Tone::Warning => t.warning,
        Tone::Danger => t.error,
        Tone::Info => t.term_blue,
        Tone::Neutral | Tone::Unknown => t.text_secondary,
    }
}

fn series_color(index: usize, t: &ThemeColors) -> u32 {
    [t.term_blue, t.term_green, t.term_magenta, t.term_yellow, t.term_cyan, t.term_red][index % 6]
}

pub fn badge(b: &ExtBadge, t: &ThemeColors, cx: &App) -> Div {
    let color = tone_color(b.tone, t);
    div()
        .flex_shrink_0()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(px(8.0))
        .bg(with_alpha(color, 0.15))
        .border_1()
        .border_color(with_alpha(color, 0.4))
        .text_size(ui_text_sm(cx))
        .text_color(rgb(color))
        .child(b.label.clone())
}

pub fn code_line(text: String, t: &ThemeColors, cx: &App) -> Div {
    div()
        .px(px(6.0))
        .py(px(2.0))
        .rounded(px(3.0))
        .bg(rgb(t.bg_primary))
        .font_family("monospace")
        .text_size(ui_text_ms(cx))
        .text_color(rgb(t.text_primary))
        .child(text)
}

pub fn loading(text: &str, t: &ThemeColors, cx: &App) -> AnyElement {
    div()
        .py(px(24.0))
        .text_size(ui_text_md(cx))
        .text_color(rgb(t.text_muted))
        .child(text.to_string())
        .into_any_element()
}

pub fn danger_button(id: &'static str, label: String, t: &ThemeColors) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .px(px(12.0))
        .py(px(6.0))
        .rounded(px(4.0))
        .bg(rgb(t.error))
        .hover(|s| s.opacity(0.85))
        .text_size(px(12.0))
        .text_color(rgb(0xffffff))
        .child(label)
}

fn small_button(id: SharedString, label: String, t: &ThemeColors, cx: &App) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .cursor_pointer()
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(rgb(t.border))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .text_size(ui_text_sm(cx))
        .text_color(rgb(t.text_secondary))
        .child(label)
}

/// One field of an action's form.
pub fn form_field(
    index: usize,
    input: &ExtInput,
    field: &Field,
    t: &ThemeColors,
    cx: &mut Context<ExtensionPane>,
) -> AnyElement {
    let label = if input.required {
        format!("{} *", input.label)
    } else {
        input.label.clone()
    };
    let control: AnyElement = match field {
        Field::Text(state) => input_container(t, None)
            .px(px(8.0))
            .py(px(4.0))
            .when(input.kind == InputKind::Multiline, |d| d.min_h(px(72.0)))
            .child(SimpleInput::new(state).text_size(ui_text_md(cx)))
            .into_any_element(),
        Field::Toggle(on) => {
            let on = *on;
            toggle_switch(SharedString::from(format!("ext-form-toggle-{index}")), on, t)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(pending) = this.pending.as_mut()
                        && let Some((_, Field::Toggle(value))) = pending.fields.get_mut(index)
                    {
                        *value = !on;
                        cx.notify();
                    }
                }))
                .into_any_element()
        }
        Field::Select(chosen) => h_flex()
            .flex_wrap()
            .gap(px(4.0))
            .children(input.options.iter().enumerate().map(|(i, option)| {
                let active = i == *chosen;
                div()
                    .id(SharedString::from(format!("ext-form-select-{index}-{i}")))
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(rgb(if active { t.border_active } else { t.border }))
                    .when(active, |d| d.bg(rgb(t.bg_selection)))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_primary))
                    .child(option.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(pending) = this.pending.as_mut()
                            && let Some((_, Field::Select(value))) = pending.fields.get_mut(index)
                        {
                            *value = i;
                            cx.notify();
                        }
                    }))
            }))
            .into_any_element(),
    };
    v_flex()
        .gap(px(4.0))
        .child(
            div()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(label),
        )
        .child(control)
        .into_any_element()
}

/// Draws node `index` and what it contains. `visited` guards against a view
/// that refers back to a node it is inside.
pub fn node(
    pane: &mut ExtensionPane,
    ext: &ClientExtension,
    view: &ExtView,
    index: u32,
    badges: &HashMap<String, AgentBadge>,
    visited: &mut HashSet<u32>,
    cx: &mut Context<ExtensionPane>,
) -> AnyElement {
    let t = theme(cx);
    if !visited.insert(index) {
        return div().into_any_element();
    }
    let Some(current) = view.node(index) else {
        return div().into_any_element();
    };
    let children = |pane: &mut ExtensionPane,
                        ids: &[u32],
                        visited: &mut HashSet<u32>,
                        cx: &mut Context<ExtensionPane>| {
        ids.iter()
            .map(|&child| node(pane, ext, view, child, badges, visited, cx))
            .collect::<Vec<_>>()
    };
    let element = match current {
        ExtNode::Text { text, style, tone } => {
            let color = tone.map(|tone| tone_color(tone, &t)).unwrap_or(match style {
                TextStyle::Muted => t.text_muted,
                TextStyle::Heading => t.text_primary,
                _ => t.text_secondary,
            });
            let d = div().text_color(rgb(color)).child(text.clone());
            match style {
                TextStyle::Heading => d
                    .text_size(ui_text(15.0, cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .pt(px(4.0)),
                TextStyle::Code => code_line(text.clone(), &t, cx),
                TextStyle::Muted => d.text_size(ui_text_ms(cx)),
                _ => d.text_size(ui_text_md(cx)),
            }
            .into_any_element()
        }
        ExtNode::Badge { badge: b } => h_flex().child(badge(b, &t, cx)).into_any_element(),
        ExtNode::Badges { badges: bs } => h_flex()
            .flex_wrap()
            .gap(px(4.0))
            .children(bs.iter().map(|b| badge(b, &t, cx)))
            .into_any_element(),
        ExtNode::Table { table } => render_table(pane, ext, table, badges, cx),
        ExtNode::Detail { detail } => render_detail(detail, &t, cx).into_any_element(),
        ExtNode::Tree { tree } => render_tree(pane, tree, &t, cx),
        ExtNode::BarChart { chart } => render_bar_chart(chart, &t, cx),
        ExtNode::LineChart { chart } => render_line_chart(chart, &t, cx),
        ExtNode::Stats { stats } => render_stats(stats, &t, cx),
        ExtNode::Actions { actions } => {
            let busy = pane.running.is_some();
            h_flex()
                .flex_wrap()
                .gap(px(6.0))
                .children(actions.iter().filter_map(|id| ext.ext.action(id)).map(|action| {
                    let id = action.id.clone();
                    button(SharedString::from(format!("ext-view-action-{id}")), action.label.clone(), &t)
                        .when(busy, |b| b.opacity(0.5))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.start_action(&id, Vec::new(), cx)
                        }))
                }))
                .into_any_element()
        }
        ExtNode::Loading { text } => loading(text, &t, cx),
        ExtNode::Empty { text } => okena_ui::empty_state::empty_state(text.clone(), &t, cx).into_any_element(),
        ExtNode::Error { text } => div()
            .p(px(10.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(rgb(t.error))
            .text_size(ui_text_md(cx))
            .text_color(rgb(t.error))
            .child(text.clone())
            .into_any_element(),
        ExtNode::Stack { children: ids } => v_flex()
            .gap(px(12.0))
            .children(children(pane, ids, visited, cx))
            .into_any_element(),
        ExtNode::Columns { children: ids } => h_flex()
            .items_start()
            .gap(px(16.0))
            .children(
                children(pane, ids, visited, cx)
                    .into_iter()
                    .map(|child| div().flex_1().min_w_0().child(child)),
            )
            .into_any_element(),
        ExtNode::Section {
            title,
            children: ids,
            collapsible,
            collapsed,
        } => {
            let open = !*collapsible || !pane.sections.get(&index).copied().unwrap_or(*collapsed);
            let collapsible = *collapsible;
            let default_collapsed = *collapsed;
            let header = h_flex()
                .id(SharedString::from(format!("ext-section-{index}")))
                .gap(px(4.0))
                .when(collapsible, |h| {
                    h.cursor_pointer()
                        .child(expand_toggle(
                            SharedString::from(format!("ext-section-toggle-{index}")),
                            open,
                            true,
                            &t,
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let collapsed = this.sections.get(&index).copied().unwrap_or(default_collapsed);
                            this.sections.insert(index, !collapsed);
                            cx.notify();
                        }))
                })
                .child(
                    div()
                        .text_size(ui_text_md(cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(t.text_primary))
                        .child(title.clone()),
                );
            v_flex()
                .gap(px(8.0))
                .child(header)
                .when(open, |d| d.children(children(pane, ids, visited, cx)))
                .into_any_element()
        }
        ExtNode::Unknown => div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child("This part of the view needs a newer okena.")
            .into_any_element(),
    };
    visited.remove(&index);
    element
}

fn render_detail(detail: &ExtDetail, t: &ThemeColors, cx: &App) -> Div {
    v_flex()
        .gap(px(6.0))
        .p(px(12.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(rgb(t.border))
        .bg(rgb(t.bg_secondary))
        .when_some(detail.title.clone(), |d, title| {
            d.child(
                div()
                    .text_size(ui_text_md(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_primary))
                    .child(title),
            )
        })
        .children(detail.fields.iter().map(|f| field_row(f, t, cx)))
}

fn field_row(f: &ExtField, t: &ThemeColors, cx: &App) -> Div {
    h_flex()
        .items_start()
        .gap(px(12.0))
        .child(
            div()
                .w(px(140.0))
                .flex_shrink_0()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(f.label.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(ui_text_md(cx))
                .text_color(rgb(f.tone.map_or(t.text_primary, |tone| tone_color(tone, t))))
                .child(f.value.clone()),
        )
}

fn checkbox(id: SharedString, checked: bool, t: &ThemeColors) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .cursor_pointer()
        .size(px(14.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(rgb(if checked { t.border_active } else { t.border }))
        .when(checked, |d| d.bg(rgb(t.border_active)))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.0))
        .text_color(rgb(0xffffff))
        .when(checked, |d| d.child("✓"))
}

fn render_table(
    pane: &mut ExtensionPane,
    ext: &ClientExtension,
    table: &ExtTable,
    badges: &HashMap<String, AgentBadge>,
    cx: &mut Context<ExtensionPane>,
) -> AnyElement {
    let t = theme(cx);
    let tid = table.id.clone();
    let filter = pane.filter_input(
        &tid,
        table.filter_placeholder.as_deref().unwrap_or("Filter rows"),
        cx,
    );
    let state: TableState = pane.tables.get(&tid).cloned().unwrap_or_default();
    let groups = arrange(table, &state);
    let visible: Vec<String> = groups
        .iter()
        .flat_map(|g| g.rows.iter().map(|&i| table.rows[i].id.clone()))
        .collect();
    let group_column = state.group_column(table).map(str::to_string);
    let sort = state.sort_column(table).map(|(c, d)| (c.to_string(), d));
    let busy = pane.running.is_some();
    let has_row_actions = !table.row_actions.is_empty();
    let selectable = !table.bulk_actions.is_empty();

    // ── Toolbar: filter, grouping, bulk actions ──
    let groupable: Vec<_> = table.columns.iter().filter(|c| c.groupable).collect();
    let mut toolbar = h_flex()
        .gap(px(8.0))
        .flex_wrap()
        .child(
            input_container(&t, None)
                .w(px(240.0))
                .px(px(8.0))
                .py(px(3.0))
                .child(SimpleInput::new(&filter).text_size(ui_text_md(cx))),
        );
    if !groupable.is_empty() {
        let mut group_row = h_flex().gap(px(4.0)).child(
            div()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child("Group by"),
        );
        let options = std::iter::once((None, "None".to_string()))
            .chain(groupable.iter().map(|c| (Some(c.key.clone()), c.label.clone())));
        for (key, label) in options {
            let active = group_column == key;
            let table_id = tid.clone();
            group_row = group_row.child(
                small_button(
                    SharedString::from(format!("ext-{tid}-group-{}", key.as_deref().unwrap_or("none"))),
                    label,
                    &t,
                    cx,
                )
                .when(active, |b| b.bg(rgb(t.bg_selection)).text_color(rgb(t.text_primary)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let state = this.tables.entry(table_id.clone()).or_default();
                    state.group_by = Some(key.clone());
                    state.collapsed_groups.clear();
                    cx.notify();
                })),
            );
        }
        toolbar = toolbar.child(group_row);
    }
    if selectable {
        let selected: Vec<String> = state.selected.iter().cloned().collect();
        let count = selected.len();
        let mut bulk = h_flex().gap(px(6.0)).child(
            div()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(format!("{count} selected")),
        );
        for action in table.bulk_actions.iter().filter_map(|id| ext.ext.action(id)) {
            let id = action.id.clone();
            let items = selected.clone();
            let enabled = count > 0 && !busy;
            bulk = bulk.child(
                small_button(
                    SharedString::from(format!("ext-{tid}-bulk-{id}")),
                    action.label.clone(),
                    &t,
                    cx,
                )
                .when(action.destructive, |b| b.text_color(rgb(t.error)))
                .when(!enabled, |b| b.opacity(0.4))
                .when(enabled, |b| {
                    b.on_click(cx.listener(move |this, _, _, cx| {
                        this.start_action(&id, items.clone(), cx)
                    }))
                }),
            );
        }
        toolbar = toolbar.child(div().flex_1()).child(bulk);
    }

    // ── Header row ──
    let all_selected = !visible.is_empty() && visible.iter().all(|id| state.selected.contains(id));
    let mut header = h_flex()
        .px(px(8.0))
        .py(px(6.0))
        .gap(px(8.0))
        .border_b_1()
        .border_color(rgb(t.border))
        .text_size(ui_text_ms(cx))
        .text_color(rgb(t.text_muted));
    if selectable {
        let table_id = tid.clone();
        let visible = visible.clone();
        header = header.child(
            checkbox(SharedString::from(format!("ext-{tid}-select-all")), all_selected, &t)
                .on_click(cx.listener(move |this, _, _, cx| {
                    let state = this.tables.entry(table_id.clone()).or_default();
                    if all_selected {
                        for id in &visible {
                            state.selected.remove(id);
                        }
                    } else {
                        state.selected.extend(visible.iter().cloned());
                    }
                    cx.notify();
                })),
        );
    }
    let table_default = default_sort(table);
    for column in &table.columns {
        let arrow = match &sort {
            Some((c, false)) if c == &column.key => " ▲",
            Some((c, true)) if c == &column.key => " ▼",
            _ => "",
        };
        let cell = div()
            .id(SharedString::from(format!("ext-{tid}-col-{}", column.key)))
            .flex_1()
            .min_w_0()
            .when(column.kind == ColumnKind::Number, |d| d.flex().justify_end())
            .child(format!("{}{arrow}", column.label));
        let cell = if column.sortable {
            let key = column.key.clone();
            let table_id = tid.clone();
            let table_default = table_default.clone();
            cell.cursor_pointer()
                .hover(|s| s.text_color(rgb(t.text_primary)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tables
                        .entry(table_id.clone())
                        .or_default()
                        .cycle_sort(table_default.clone(), &key);
                    cx.notify();
                }))
        } else {
            cell
        };
        header = header.child(cell);
    }
    if !badges.is_empty() {
        header = header.child(div().w(px(90.0)));
    }
    if has_row_actions {
        header = header.child(div().w(px(row_actions_width(table))));
    }

    // ── Rows, grouped ──
    let mut body = v_flex();
    let mut drawn = 0usize;
    let total: usize = groups.iter().map(|g| g.rows.len()).sum();
    for group in &groups {
        if let Some(label) = &group.label {
            let collapsed = state.collapsed_groups.contains(label);
            let table_id = tid.clone();
            let label_key = label.clone();
            body = body.child(
                h_flex()
                    .id(SharedString::from(format!("ext-{tid}-group-row-{label}")))
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(5.0))
                    .gap(px(6.0))
                    .bg(rgb(t.bg_secondary))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .child(expand_toggle(
                        SharedString::from(format!("ext-{tid}-group-toggle-{label}")),
                        !collapsed,
                        true,
                        &t,
                    ))
                    .child(
                        div()
                            .text_size(ui_text_md(cx))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(t.text_primary))
                            .child(if label.is_empty() { "(none)".to_string() } else { label.clone() }),
                    )
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(group.rows.len().to_string()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let state = this.tables.entry(table_id.clone()).or_default();
                        if !state.collapsed_groups.remove(&label_key) {
                            state.collapsed_groups.insert(label_key.clone());
                        }
                        cx.notify();
                    })),
            );
            if collapsed {
                continue;
            }
        }
        for &i in &group.rows {
            if drawn >= MAX_DRAWN_ROWS {
                break;
            }
            drawn += 1;
            let row = &table.rows[i];
            let row_id = row.id.clone();
            let selected = state.selected.contains(&row.id);
            let focused = state.focused.as_deref() == Some(row.id.as_str());
            let mut line = h_flex()
                .id(SharedString::from(format!("ext-{tid}-row-{}", row.id)))
                .px(px(8.0))
                .py(px(5.0))
                .gap(px(8.0))
                .border_b_1()
                .border_color(with_alpha(t.border, 0.5))
                .when(focused, |d| d.bg(rgb(t.bg_selection)))
                .when(!focused, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
                .when(!row.detail.is_empty(), |d| {
                    let table_id = tid.clone();
                    let row_id = row_id.clone();
                    d.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                        let state = this.tables.entry(table_id.clone()).or_default();
                        state.focused = if state.focused.as_deref() == Some(row_id.as_str()) {
                            None
                        } else {
                            Some(row_id.clone())
                        };
                        cx.notify();
                    }))
                });
            if selectable {
                let table_id = tid.clone();
                let row_id = row_id.clone();
                line = line.child(
                    checkbox(SharedString::from(format!("ext-{tid}-check-{}", row.id)), selected, &t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.tables.entry(table_id.clone()).or_default().toggle_selected(&row_id);
                            cx.notify();
                        })),
                );
            }
            for (ci, column) in table.columns.iter().enumerate() {
                let cell = row.cells.get(ci);
                let text = cell.map(|c| c.text.clone()).unwrap_or_default();
                let tone = cell.and_then(|c| c.tone);
                let content: AnyElement = if column.kind == ColumnKind::Badge && !text.is_empty() {
                    badge(
                        &ExtBadge {
                            label: text,
                            tone: tone.unwrap_or_default(),
                        },
                        &t,
                        cx,
                    )
                    .into_any_element()
                } else {
                    div()
                        .truncate()
                        .text_color(rgb(tone.map_or(t.text_primary, |tone| tone_color(tone, &t))))
                        .child(text)
                        .into_any_element()
                };
                line = line.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(ui_text_md(cx))
                        .when(column.kind == ColumnKind::Number, |d| d.flex().justify_end())
                        .child(content),
                );
            }
            if !badges.is_empty() {
                let cell = match badges.get(&row.id) {
                    Some(agent) => {
                        let project_id = agent.project_id.clone();
                        agent_badge(agent, SharedString::from(format!("ext-{tid}-agent-{}", row.id)), cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.open_session(project_id.clone(), cx)
                            }))
                            .into_any_element()
                    }
                    None => div().into_any_element(),
                };
                line = line.child(div().w(px(90.0)).flex().justify_end().child(cell));
            }
            if has_row_actions {
                let mut actions = h_flex().w(px(row_actions_width(table))).gap(px(4.0)).justify_end();
                for action in table.row_actions.iter().filter_map(|id| ext.ext.action(id)) {
                    let id = action.id.clone();
                    let row_id = row_id.clone();
                    actions = actions.child(
                        small_button(
                            SharedString::from(format!("ext-{tid}-row-{}-{id}", row.id)),
                            action.label.clone(),
                            &t,
                            cx,
                        )
                        .when(action.destructive, |b| b.text_color(rgb(t.error)))
                        .when(busy, |b| b.opacity(0.4))
                        .when(!busy, |b| {
                            b.on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.start_action(&id, vec![row_id.clone()], cx)
                            }))
                        }),
                    );
                }
                line = line.child(actions);
            }
            body = body.child(line);
            if focused && !row.detail.is_empty() {
                body = body.child(
                    div().px(px(32.0)).py(px(8.0)).child(render_detail(
                        &ExtDetail {
                            title: None,
                            fields: row.detail.clone(),
                        },
                        &t,
                        cx,
                    )),
                );
            }
        }
    }
    if total == 0 {
        let text = if state.filter.trim().is_empty() {
            table.empty_text.clone().unwrap_or_else(|| "No rows".into())
        } else {
            "No rows match the filter".into()
        };
        body = body.child(okena_ui::empty_state::empty_state(text, &t, cx));
    } else if drawn < total && groups.iter().all(|g| g.label.is_none() || !state.collapsed_groups.contains(g.label.as_deref().unwrap_or(""))) {
        body = body.child(
            div()
                .px(px(8.0))
                .py(px(6.0))
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(format!("Showing {drawn} of {total} rows; filter to narrow them down.")),
        );
    }

    v_flex()
        .gap(px(8.0))
        .child(toolbar)
        .child(
            v_flex()
                .rounded(px(6.0))
                .border_1()
                .border_color(rgb(t.border))
                .child(header)
                .child(body),
        )
        .into_any_element()
}

/// Room for a table's row action buttons.
fn row_actions_width(table: &ExtTable) -> f32 {
    (table.row_actions.len() as f32 * 96.0).clamp(96.0, 320.0)
}

fn agent_badge(agent: &AgentBadge, id: SharedString, cx: &App) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .flex_shrink_0()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(px(8.0))
        .bg(with_alpha(agent.color, 0.18))
        .border_1()
        .border_color(with_alpha(agent.color, 0.5))
        .hover(|s| s.bg(with_alpha(agent.color, 0.3)))
        .text_size(ui_text_sm(cx))
        .text_color(rgb(agent.color))
        .child(format!("● {}", agent.label))
}

fn render_tree(
    pane: &mut ExtensionPane,
    tree: &ExtTree,
    t: &ThemeColors,
    cx: &mut Context<ExtensionPane>,
) -> AnyElement {
    let id = tree.id.clone();
    if tree.roots.is_empty() {
        return okena_ui::empty_state::empty_state(
            tree.empty_text.clone().unwrap_or_else(|| "Nothing here".into()),
            t,
            cx,
        )
        .into_any_element();
    }
    let selected = pane.trees.get(&id).and_then(|s| s.selected.clone());
    let mut rows = Vec::new();
    let mut stack: Vec<(u32, usize)> = tree.roots.iter().rev().map(|&r| (r, 0)).collect();
    let mut seen = HashSet::new();
    while let Some((index, depth)) = stack.pop() {
        if rows.len() >= MAX_DRAWN_ROWS || !seen.insert(index) {
            continue;
        }
        let Some(item) = tree.items.get(index as usize) else {
            continue;
        };
        let open = pane
            .trees
            .get(&id)
            .and_then(|s| s.toggled.get(&item.id).copied())
            .unwrap_or(item.expanded);
        let has_children = !item.children.is_empty();
        let is_selected = selected.as_deref() == Some(item.id.as_str());
        let tree_id = id.clone();
        let item_id = item.id.clone();
        let default_open = item.expanded;
        rows.push(
            h_flex()
                .id(SharedString::from(format!("ext-tree-{id}-{index}")))
                .cursor_pointer()
                .pl(px(8.0 + depth as f32 * 16.0))
                .pr(px(8.0))
                .py(px(3.0))
                .gap(px(6.0))
                .when(is_selected, |d| d.bg(rgb(t.bg_selection)))
                .when(!is_selected, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
                .child(expand_toggle(
                    SharedString::from(format!("ext-tree-{id}-toggle-{index}")),
                    open,
                    has_children,
                    t,
                ))
                .child(
                    div()
                        .text_size(ui_text_md(cx))
                        .text_color(rgb(t.text_primary))
                        .child(item.label.clone()),
                )
                .when_some(item.hint.clone(), |d, hint| {
                    d.child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(hint),
                    )
                })
                .when_some(item.badge.as_ref(), |d, b| d.child(badge(b, t, cx)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let state = this.trees.entry(tree_id.clone()).or_default();
                    if has_children {
                        let now = state.toggled.get(&item_id).copied().unwrap_or(default_open);
                        state.toggled.insert(item_id.clone(), !now);
                    }
                    state.selected = Some(item_id.clone());
                    cx.notify();
                }))
                .into_any_element(),
        );
        if open {
            for &child in item.children.iter().rev() {
                stack.push((child, depth + 1));
            }
        }
    }
    let list = v_flex()
        .py(px(4.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(rgb(t.border))
        .children(rows);
    if !tree.show_detail {
        return list.into_any_element();
    }
    let detail = selected
        .and_then(|sel| tree.items.iter().find(|i| i.id == sel))
        .map(|item| {
            render_detail(
                &ExtDetail {
                    title: Some(item.label.clone()),
                    fields: item.detail.clone(),
                },
                t,
                cx,
            )
            .into_any_element()
        })
        .unwrap_or_else(|| {
            div()
                .p(px(12.0))
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child("Select an item to see its details.")
                .into_any_element()
        });
    h_flex()
        .items_start()
        .gap(px(12.0))
        .child(div().flex_1().min_w_0().child(list))
        .child(div().w(px(340.0)).flex_shrink_0().child(detail))
        .into_any_element()
}

fn chart_frame(title: Option<&str>, body: AnyElement, t: &ThemeColors, cx: &App) -> AnyElement {
    v_flex()
        .gap(px(8.0))
        .p(px(12.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(rgb(t.border))
        .when_some(title.map(str::to_string), |d, title| {
            d.child(
                div()
                    .text_size(ui_text_md(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_primary))
                    .child(title),
            )
        })
        .child(body)
        .into_any_element()
}

fn format_value(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e12 {
        format!("{}", value as i64)
    } else {
        format!("{value:.2}")
    }
}

/// Horizontal bars, one row per point, one group per series.
fn render_bar_chart(chart: &ExtChart, t: &ThemeColors, cx: &App) -> AnyElement {
    let max = chart
        .series
        .iter()
        .flat_map(|s| s.points.iter().map(|p| p.value))
        .fold(0.0_f64, f64::max);
    let mut body = v_flex().gap(px(4.0));
    for (si, series) in chart.series.iter().enumerate() {
        let color = series_color(si, t);
        if chart.series.len() > 1 {
            body = body.child(
                div()
                    .pt(px(4.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(color))
                    .child(series.label.clone()),
            );
        }
        for point in &series.points {
            let fraction = if max > 0.0 { (point.value / max).clamp(0.0, 1.0) } else { 0.0 };
            body = body.child(
                h_flex()
                    .gap(px(8.0))
                    .child(
                        div()
                            .w(px(140.0))
                            .flex_shrink_0()
                            .truncate()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_secondary))
                            .child(point.label.clone()),
                    )
                    .child(
                        div().flex_1().h(px(12.0)).child(
                            div()
                                .h_full()
                                .w(relative(fraction.max(0.005) as f32))
                                .rounded(px(2.0))
                                .bg(rgb(color)),
                        ),
                    )
                    .child(
                        div()
                            .w(px(64.0))
                            .flex()
                            .justify_end()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_primary))
                            .child(format_value(point.value)),
                    ),
            );
        }
    }
    chart_frame(chart.title.as_deref(), body.into_any_element(), t, cx)
}

/// Lines over a shared x axis of point positions, painted on a canvas.
fn render_line_chart(chart: &ExtChart, t: &ThemeColors, cx: &App) -> AnyElement {
    let series: Vec<(u32, Vec<f64>)> = chart
        .series
        .iter()
        .enumerate()
        .map(|(i, s)| (series_color(i, t), s.points.iter().map(|p| p.value).collect()))
        .collect();
    let (min, max) = series
        .iter()
        .flat_map(|(_, values)| values.iter().copied())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
    let (min, max) = if min.is_finite() { (min, max) } else { (0.0, 0.0) };
    let span = if max > min { max - min } else { 1.0 };
    let grid = t.border;
    let plot = canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            for line in 0..=2 {
                let y = oy + h * line as f32 / 2.0;
                stroke(window, grid, 1.0, |b| {
                    b.move_to(point(px(ox), px(y)));
                    b.line_to(point(px(ox + w), px(y)));
                });
            }
            for (color, values) in &series {
                if values.len() < 2 {
                    continue;
                }
                let step = w / (values.len() - 1) as f32;
                stroke(window, *color, 2.0, |b| {
                    for (i, v) in values.iter().enumerate() {
                        let x = ox + step * i as f32;
                        let y = oy + h - (((v - min) / span) as f32) * h;
                        if i == 0 {
                            b.move_to(point(px(x), px(y)));
                        } else {
                            b.line_to(point(px(x), px(y)));
                        }
                    }
                });
            }
        },
    )
    .w_full()
    .h(px(140.0));
    let first = chart.series.first().and_then(|s| s.points.first()).map(|p| p.label.clone());
    let last = chart.series.first().and_then(|s| s.points.last()).map(|p| p.label.clone());
    let legend = h_flex().gap(px(12.0)).children(chart.series.iter().enumerate().map(|(i, s)| {
        h_flex()
            .gap(px(4.0))
            .child(div().size(px(8.0)).rounded_full().bg(rgb(series_color(i, t))))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(s.label.clone()),
            )
    }));
    let body = v_flex()
        .gap(px(4.0))
        .child(
            h_flex()
                .justify_between()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(format!("max {}", format_value(max)))
                .child(format!("min {}", format_value(min))),
        )
        .child(plot)
        .child(
            h_flex()
                .justify_between()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(first.unwrap_or_default())
                .child(last.unwrap_or_default()),
        )
        .child(legend);
    chart_frame(chart.title.as_deref(), body.into_any_element(), t, cx)
}

fn stroke(window: &mut Window, color: u32, width: f32, build: impl FnOnce(&mut PathBuilder)) {
    let options = StrokeOptions::default()
        .with_line_width(width)
        .with_line_join(lyon::path::LineJoin::Round)
        .with_line_cap(lyon::path::LineCap::Round);
    let mut builder = PathBuilder::stroke(px(width)).with_style(PathStyle::Stroke(options));
    build(&mut builder);
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgb(color));
    }
}

fn render_stats(stats: &[ExtStat], t: &ThemeColors, cx: &App) -> AnyElement {
    h_flex()
        .flex_wrap()
        .gap(px(10.0))
        .children(stats.iter().map(|s| {
            v_flex()
                .min_w(px(120.0))
                .p(px(12.0))
                .gap(px(2.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(rgb(t.border))
                .bg(rgb(t.bg_secondary))
                .child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child(s.label.clone()),
                )
                .child(
                    div()
                        .text_size(ui_text(22.0, cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(s.tone.map_or(t.text_primary, |tone| tone_color(tone, t))))
                        .child(s.value.clone()),
                )
                .when_some(s.hint.clone(), |d, hint| {
                    d.child(
                        div()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_muted))
                            .child(hint),
                    )
                })
        }))
        .into_any_element()
}
