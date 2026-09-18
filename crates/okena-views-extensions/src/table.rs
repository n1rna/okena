//! Grouping, sorting and filtering an extension's table, on the client.
//!
//! The extension only supplies rows; what the user does to arrange them is
//! local to the view and survives refreshes, since rows keep their ids.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashSet};

use okena_core::extension::{ExtRow, ExtTable};

/// How the user has arranged one table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableState {
    /// `None` until the user picks: the extension's own default applies.
    pub group_by: Option<Option<String>>,
    /// `None` until the user picks, as for `group_by`.
    pub sort: Option<Option<(String, bool)>>,
    pub filter: String,
    pub selected: BTreeSet<String>,
    pub collapsed_groups: HashSet<String>,
    /// The row whose detail is showing.
    pub focused: Option<String>,
}

impl TableState {
    pub fn group_column<'a>(&'a self, table: &'a ExtTable) -> Option<&'a str> {
        match &self.group_by {
            Some(choice) => choice.as_deref(),
            None => table.group_by.as_deref(),
        }
    }

    /// `(column, descending)`.
    pub fn sort_column<'a>(&'a self, table: &'a ExtTable) -> Option<(&'a str, bool)> {
        match &self.sort {
            Some(choice) => choice.as_ref().map(|(c, d)| (c.as_str(), *d)),
            None => table.sort.as_ref().map(|s| (s.column.as_str(), s.descending)),
        }
    }

    /// Clicking a header: ascending, then descending, then unsorted.
    /// `table_default` is [`default_sort`] of the table, so a click handler
    /// need not hold the whole table.
    pub fn cycle_sort(&mut self, table_default: Option<(String, bool)>, column: &str) {
        let current = match &self.sort {
            Some(choice) => choice.clone(),
            None => table_default,
        };
        let next = match current {
            Some((c, false)) if c == column => Some((column.to_string(), true)),
            Some((c, true)) if c == column => None,
            _ => Some((column.to_string(), false)),
        };
        self.sort = Some(next);
    }

    /// Drops selections of rows the latest refresh no longer has.
    pub fn retain_rows(&mut self, table: &ExtTable) {
        let ids: HashSet<&str> = table.rows.iter().map(|r| r.id.as_str()).collect();
        self.selected.retain(|id| ids.contains(id.as_str()));
        if self.focused.as_deref().is_some_and(|id| !ids.contains(id)) {
            self.focused = None;
        }
    }

    pub fn toggle_selected(&mut self, id: &str) {
        if !self.selected.remove(id) {
            self.selected.insert(id.to_string());
        }
    }
}

/// The table's own sort, before the user picks one.
pub fn default_sort(table: &ExtTable) -> Option<(String, bool)> {
    table.sort.as_ref().map(|s| (s.column.clone(), s.descending))
}

/// A run of rows under one group header; `label` is `None` when ungrouped.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub label: Option<String>,
    /// Indices into the table's rows, in display order.
    pub rows: Vec<usize>,
}

fn column_index(table: &ExtTable, key: &str) -> Option<usize> {
    table.columns.iter().position(|c| c.key == key)
}

fn cell_text(row: &ExtRow, index: Option<usize>) -> &str {
    index
        .and_then(|i| row.cells.get(i))
        .map_or("", |c| c.text.as_str())
}

/// Every cell's text, or its detail's, contains every word of the filter.
fn matches(row: &ExtRow, words: &[String]) -> bool {
    words.iter().all(|word| {
        row.cells.iter().any(|c| c.text.to_lowercase().contains(word))
            || row.id.to_lowercase().contains(word)
            || row.detail.iter().any(|f| f.value.to_lowercase().contains(word))
    })
}

fn compare(a: &ExtRow, b: &ExtRow, index: usize) -> Ordering {
    let (ca, cb) = (a.cells.get(index), b.cells.get(index));
    match (ca.and_then(|c| c.number), cb.and_then(|c| c.number)) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        // Numbers before blanks (after them when the sort is reversed).
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => {
            let ta = ca.map_or("", |c| c.text.as_str());
            let tb = cb.map_or("", |c| c.text.as_str());
            natural_cmp(ta, tb)
        }
    }
}

/// Case-insensitive, with runs of digits compared as numbers: `job-2`
/// before `job-10`.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        it.next();
                    }
                    digits
                };
                let (na, nb) = (take(&mut a), take(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// The rows to show, filtered, sorted and grouped as `state` says. Groups
/// come in natural order of their label; rows keep the sort within them.
pub fn arrange(table: &ExtTable, state: &TableState) -> Vec<Group> {
    let words: Vec<String> = state
        .filter
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    let mut rows: Vec<usize> = (0..table.rows.len())
        .filter(|&i| matches(&table.rows[i], &words))
        .collect();

    if let Some((column, descending)) = state.sort_column(table)
        && let Some(index) = column_index(table, column)
    {
        rows.sort_by(|&a, &b| {
            let ord = compare(&table.rows[a], &table.rows[b], index);
            if descending { ord.reverse() } else { ord }
        });
    }

    let Some(group_index) = state
        .group_column(table)
        .and_then(|column| column_index(table, column))
    else {
        return vec![Group { label: None, rows }];
    };

    let mut groups: Vec<Group> = Vec::new();
    for i in rows {
        let label = cell_text(&table.rows[i], Some(group_index)).to_string();
        match groups.iter_mut().find(|g| g.label.as_deref() == Some(label.as_str())) {
            Some(group) => group.rows.push(i),
            None => groups.push(Group {
                label: Some(label),
                rows: vec![i],
            }),
        }
    }
    groups.sort_by(|a, b| {
        natural_cmp(
            a.label.as_deref().unwrap_or(""),
            b.label.as_deref().unwrap_or(""),
        )
    });
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_core::extension::{ColumnKind, ExtCell, ExtColumn, ExtSort};

    fn column(key: &str) -> ExtColumn {
        ExtColumn {
            key: key.into(),
            label: key.into(),
            kind: ColumnKind::Text,
            groupable: true,
            sortable: true,
        }
    }

    fn row(id: &str, tenant: &str, age: Option<f64>, status: &str) -> ExtRow {
        ExtRow {
            id: id.into(),
            cells: vec![
                ExtCell { text: tenant.into(), ..Default::default() },
                ExtCell {
                    text: age.map(|a| format!("{a}m")).unwrap_or_default(),
                    number: age,
                    tone: None,
                },
                ExtCell { text: status.into(), ..Default::default() },
            ],
            detail: vec![],
        }
    }

    fn table() -> ExtTable {
        ExtTable {
            id: "jobs".into(),
            columns: vec![column("tenant"), column("age"), column("status")],
            rows: vec![
                row("job-10", "beta", Some(5.0), "stuck"),
                row("job-2", "acme", Some(30.0), "running"),
                row("job-3", "acme", None, "stuck"),
                row("job-1", "beta", Some(12.0), "done"),
            ],
            group_by: None,
            sort: None,
            row_actions: vec![],
            bulk_actions: vec![],
            filter_placeholder: None,
            empty_text: None,
        }
    }

    fn ids(t: &ExtTable, groups: &[Group]) -> Vec<Vec<String>> {
        groups
            .iter()
            .map(|g| g.rows.iter().map(|&i| t.rows[i].id.clone()).collect())
            .collect()
    }

    #[test]
    fn without_choices_rows_keep_the_extensions_order() {
        let t = table();
        let groups = arrange(&t, &TableState::default());
        assert_eq!(ids(&t, &groups), vec![vec!["job-10", "job-2", "job-3", "job-1"]]);
    }

    #[test]
    fn numbers_sort_numerically_and_blanks_go_last() {
        let t = table();
        let mut state = TableState::default();
        state.cycle_sort(default_sort(&t), "age");
        assert_eq!(ids(&t, &arrange(&t, &state)), vec![vec!["job-10", "job-1", "job-2", "job-3"]]);
        state.cycle_sort(default_sort(&t), "age");
        assert_eq!(state.sort_column(&t), Some(("age", true)));
        assert_eq!(ids(&t, &arrange(&t, &state))[0][0], "job-3".to_string());
        state.cycle_sort(default_sort(&t), "age");
        assert_eq!(state.sort_column(&t), None, "a third click unsorts");
    }

    #[test]
    fn text_sorts_naturally() {
        assert_eq!(natural_cmp("job-2", "job-10"), Ordering::Less);
        assert_eq!(natural_cmp("Acme", "beta"), Ordering::Less);
        assert_eq!(natural_cmp("job-02", "job-2"), Ordering::Equal);
    }

    #[test]
    fn grouping_collects_rows_under_their_value_in_order() {
        let t = table();
        let state = TableState {
            group_by: Some(Some("tenant".into())),
            sort: Some(Some(("age".into(), false))),
            ..Default::default()
        };
        let groups = arrange(&t, &state);
        let labels: Vec<_> = groups.iter().map(|g| g.label.clone().unwrap_or_default()).collect();
        assert_eq!(labels, vec!["acme", "beta"]);
        assert_eq!(ids(&t, &groups), vec![vec!["job-2", "job-3"], vec!["job-10", "job-1"]]);
    }

    #[test]
    fn the_extensions_defaults_apply_until_the_user_chooses() {
        let mut t = table();
        t.group_by = Some("status".into());
        t.sort = Some(ExtSort { column: "age".into(), descending: true });
        let mut state = TableState::default();
        assert_eq!(state.group_column(&t), Some("status"));
        assert_eq!(state.sort_column(&t), Some(("age", true)));
        state.group_by = Some(None);
        assert_eq!(state.group_column(&t), None, "choosing no grouping overrides the default");
        assert_eq!(arrange(&t, &state).len(), 1);
    }

    #[test]
    fn the_filter_needs_every_word_somewhere_in_the_row() {
        let t = table();
        let state = TableState { filter: "ACME stuck".into(), ..Default::default() };
        assert_eq!(ids(&t, &arrange(&t, &state)), vec![vec!["job-3"]]);
        let state = TableState { filter: "nothing-matches".into(), ..Default::default() };
        assert_eq!(ids(&t, &arrange(&t, &state)), vec![Vec::<String>::new()]);
    }

    #[test]
    fn selections_of_rows_that_went_away_are_dropped() {
        let mut t = table();
        let mut state = TableState::default();
        state.toggle_selected("job-2");
        state.toggle_selected("job-3");
        state.focused = Some("job-3".into());
        t.rows.retain(|r| r.id != "job-3");
        state.retain_rows(&t);
        assert_eq!(state.selected.iter().collect::<Vec<_>>(), vec!["job-2"]);
        assert_eq!(state.focused, None);
        state.toggle_selected("job-2");
        assert!(state.selected.is_empty());
    }
}
