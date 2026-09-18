//! The components okena draws, and builders for them.
//!
//! A view is a flat list of nodes: [`View::add`] returns a [`NodeId`] that
//! containers ([`View::stack`], [`View::columns`], [`View::section`]) take as
//! children. okena handles grouping, sorting, filtering and selection of
//! tables itself; the extension only supplies the rows.

use crate::wit::okena::extension::ui_v1 as w;

pub use crate::wit::okena::extension::types::Tone;
pub use w::{ColumnKind, Status, TextStyle};

/// A node's place in its [`View`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeId(u32);

/// A view under construction.
#[derive(Default)]
pub struct View {
    nodes: Vec<w::Node>,
}

/// A view ready to return from `refresh`.
pub struct FinishedView(pub(crate) w::View);

impl View {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, node: Node) -> NodeId {
        self.nodes.push(node.0);
        NodeId(self.nodes.len() as u32 - 1)
    }

    /// Children one under another.
    pub fn stack(&mut self, children: impl IntoIterator<Item = NodeId>) -> NodeId {
        let children = ids(children);
        self.add(Node(w::Node::Stack(children)))
    }

    /// Children side by side.
    pub fn columns(&mut self, children: impl IntoIterator<Item = NodeId>) -> NodeId {
        let children = ids(children);
        self.add(Node(w::Node::Columns(children)))
    }

    /// A titled group of children, optionally collapsible.
    pub fn section(
        &mut self,
        title: impl Into<String>,
        children: impl IntoIterator<Item = NodeId>,
        collapsible: bool,
    ) -> NodeId {
        let section = w::Section {
            title: title.into(),
            children: ids(children),
            collapsible,
            collapsed: false,
        };
        self.add(Node(w::Node::Section(section)))
    }

    pub fn finish(self, root: NodeId) -> FinishedView {
        FinishedView(w::View {
            nodes: self.nodes,
            root: root.0,
        })
    }
}

fn ids(children: impl IntoIterator<Item = NodeId>) -> Vec<u32> {
    children.into_iter().map(|id| id.0).collect()
}

/// One component, before it is added to a [`View`].
pub struct Node(w::Node);

fn styled(text: impl Into<String>, style: TextStyle, tone: Option<Tone>) -> Node {
    Node(w::Node::Text(w::Text {
        text: text.into(),
        style,
        tone,
    }))
}

pub fn text(text: impl Into<String>) -> Node {
    styled(text, TextStyle::Body, None)
}

pub fn heading(text: impl Into<String>) -> Node {
    styled(text, TextStyle::Heading, None)
}

pub fn muted(text: impl Into<String>) -> Node {
    styled(text, TextStyle::Muted, None)
}

pub fn code(text: impl Into<String>) -> Node {
    styled(text, TextStyle::Code, None)
}

pub fn toned(text: impl Into<String>, tone: Tone) -> Node {
    styled(text, TextStyle::Body, Some(tone))
}

pub fn badge(label: impl Into<String>, tone: Tone) -> Node {
    Node(w::Node::Badge(w::Badge {
        label: label.into(),
        tone,
    }))
}

pub fn badges<S: Into<String>>(badges: impl IntoIterator<Item = (S, Tone)>) -> Node {
    Node(w::Node::Badges(
        badges
            .into_iter()
            .map(|(label, tone)| w::Badge {
                label: label.into(),
                tone,
            })
            .collect(),
    ))
}

pub fn loading(text: impl Into<String>) -> Node {
    Node(w::Node::Loading(text.into()))
}

pub fn empty(text: impl Into<String>) -> Node {
    Node(w::Node::Empty(text.into()))
}

pub fn error(text: impl Into<String>) -> Node {
    Node(w::Node::Error(text.into()))
}

/// Buttons for view-level actions, by action id.
pub fn actions<S: Into<String>>(ids: impl IntoIterator<Item = S>) -> Node {
    Node(w::Node::Actions(ids.into_iter().map(Into::into).collect()))
}

/// A key-value pane.
pub fn detail(title: Option<&str>, fields: Vec<Field>) -> Node {
    Node(w::Node::Detail(w::Detail {
        title: title.map(Into::into),
        fields: fields.into_iter().map(|f| f.0).collect(),
    }))
}

/// Stat tiles: a big value under a small label.
pub fn stats(stats: impl IntoIterator<Item = Stat>) -> Node {
    Node(w::Node::Stats(stats.into_iter().map(|s| s.0).collect()))
}

/// A label and a value, for detail panes.
pub struct Field(w::Field);

impl Field {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self(w::Field {
            label: label.into(),
            value: value.into(),
            tone: None,
        })
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.0.tone = Some(tone);
        self
    }
}

pub struct Stat(w::Stat);

impl Stat {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self(w::Stat {
            label: label.into(),
            value: value.into(),
            hint: None,
            tone: None,
        })
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.0.hint = Some(hint.into());
        self
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.0.tone = Some(tone);
        self
    }
}

// ─── Tables ─────────────────────────────────────────────────────────────────

pub struct Column(w::Column);

impl Column {
    pub fn new(key: impl Into<String>, label: impl Into<String>, kind: ColumnKind) -> Self {
        Self(w::Column {
            key: key.into(),
            label: label.into(),
            kind,
            groupable: false,
            sortable: true,
        })
    }

    pub fn text(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(key, label, ColumnKind::Text)
    }

    pub fn number(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(key, label, ColumnKind::Number)
    }

    pub fn badge(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(key, label, ColumnKind::Badge)
    }

    pub fn date(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(key, label, ColumnKind::Date)
    }

    /// Offered in the table's group-by menu.
    pub fn groupable(mut self) -> Self {
        self.0.groupable = true;
        self
    }

    pub fn unsortable(mut self) -> Self {
        self.0.sortable = false;
        self
    }
}

pub struct Cell(w::Cell);

impl Cell {
    pub fn text(text: impl Into<String>) -> Self {
        Self(w::Cell {
            text: text.into(),
            number: None,
            tone: None,
        })
    }

    /// Shown as `text`, sorted by `value`.
    pub fn number(value: f64, text: impl Into<String>) -> Self {
        Self(w::Cell {
            text: text.into(),
            number: Some(value),
            tone: None,
        })
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.0.tone = Some(tone);
        self
    }
}

impl<S: Into<String>> From<S> for Cell {
    fn from(text: S) -> Self {
        Cell::text(text)
    }
}

pub struct Row(w::Row);

impl Row {
    /// `id` must stay the same across refreshes: actions and agent sessions
    /// are keyed by it.
    pub fn new(id: impl Into<String>) -> Self {
        Self(w::Row {
            id: id.into(),
            cells: Vec::new(),
            detail: Vec::new(),
        })
    }

    pub fn cell(mut self, cell: impl Into<Cell>) -> Self {
        self.0.cells.push(cell.into().0);
        self
    }

    /// Shown in a detail pane while the row is selected.
    pub fn detail(mut self, field: Field) -> Self {
        self.0.detail.push(field.0);
        self
    }
}

pub struct Table(w::Table);

impl Table {
    pub fn new(id: impl Into<String>) -> Self {
        Self(w::Table {
            id: id.into(),
            columns: Vec::new(),
            rows: Vec::new(),
            group_by: None,
            sort: None,
            row_actions: Vec::new(),
            bulk_actions: Vec::new(),
            filter_placeholder: None,
            empty_text: None,
        })
    }

    pub fn column(mut self, column: Column) -> Self {
        self.0.columns.push(column.0);
        self
    }

    pub fn row(mut self, row: Row) -> Self {
        self.0.rows.push(row.0);
        self
    }

    pub fn rows(mut self, rows: impl IntoIterator<Item = Row>) -> Self {
        self.0.rows.extend(rows.into_iter().map(|r| r.0));
        self
    }

    /// Grouped by this column at first; the user can change it.
    pub fn group_by(mut self, column: impl Into<String>) -> Self {
        self.0.group_by = Some(column.into());
        self
    }

    pub fn sort_by(mut self, column: impl Into<String>, descending: bool) -> Self {
        self.0.sort = Some(w::Sort {
            column: column.into(),
            descending,
        });
        self
    }

    pub fn row_actions<S: Into<String>>(mut self, ids: impl IntoIterator<Item = S>) -> Self {
        self.0.row_actions = ids.into_iter().map(Into::into).collect();
        self
    }

    pub fn bulk_actions<S: Into<String>>(mut self, ids: impl IntoIterator<Item = S>) -> Self {
        self.0.bulk_actions = ids.into_iter().map(Into::into).collect();
        self
    }

    pub fn filter_placeholder(mut self, text: impl Into<String>) -> Self {
        self.0.filter_placeholder = Some(text.into());
        self
    }

    pub fn empty_text(mut self, text: impl Into<String>) -> Self {
        self.0.empty_text = Some(text.into());
        self
    }

    pub fn build(self) -> Node {
        Node(w::Node::Table(self.0))
    }
}

// ─── Trees ──────────────────────────────────────────────────────────────────

/// An item's place in its [`Tree`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemId(u32);

pub struct TreeItem(w::TreeItem);

impl TreeItem {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self(w::TreeItem {
            id: id.into(),
            label: label.into(),
            hint: None,
            badge: None,
            children: Vec::new(),
            expanded: false,
            detail: Vec::new(),
        })
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.0.hint = Some(hint.into());
        self
    }

    pub fn badge(mut self, label: impl Into<String>, tone: Tone) -> Self {
        self.0.badge = Some(w::Badge {
            label: label.into(),
            tone,
        });
        self
    }

    pub fn expanded(mut self) -> Self {
        self.0.expanded = true;
        self
    }

    pub fn detail(mut self, field: Field) -> Self {
        self.0.detail.push(field.0);
        self
    }
}

pub struct Tree(w::Tree);

impl Tree {
    pub fn new(id: impl Into<String>) -> Self {
        Self(w::Tree {
            id: id.into(),
            items: Vec::new(),
            roots: Vec::new(),
            show_detail: false,
            empty_text: None,
        })
    }

    /// Adds `item` under `parent`, or at the top when `parent` is `None`.
    pub fn add(&mut self, parent: Option<ItemId>, item: TreeItem) -> ItemId {
        let index = self.0.items.len() as u32;
        self.0.items.push(item.0);
        match parent {
            Some(parent) => {
                if let Some(parent) = self.0.items.get_mut(parent.0 as usize) {
                    parent.children.push(index);
                }
            }
            None => self.0.roots.push(index),
        }
        ItemId(index)
    }

    /// Draws a detail pane for the selected item beside the tree.
    pub fn with_detail_pane(mut self) -> Self {
        self.0.show_detail = true;
        self
    }

    pub fn empty_text(mut self, text: impl Into<String>) -> Self {
        self.0.empty_text = Some(text.into());
        self
    }

    pub fn build(self) -> Node {
        Node(w::Node::Tree(self.0))
    }
}

// ─── Charts ─────────────────────────────────────────────────────────────────

pub struct Chart(w::Chart);

impl Chart {
    pub fn new() -> Self {
        Self(w::Chart {
            title: None,
            series: Vec::new(),
        })
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.0.title = Some(title.into());
        self
    }

    pub fn series<S: Into<String>>(
        mut self,
        label: impl Into<String>,
        points: impl IntoIterator<Item = (S, f64)>,
    ) -> Self {
        self.0.series.push(w::Series {
            label: label.into(),
            points: points
                .into_iter()
                .map(|(label, value)| w::Point {
                    label: label.into(),
                    value,
                })
                .collect(),
        });
        self
    }

    pub fn bar(self) -> Node {
        Node(w::Node::BarChart(self.0))
    }

    pub fn line(self) -> Node {
        Node(w::Node::LineChart(self.0))
    }
}

impl Default for Chart {
    fn default() -> Self {
        Self::new()
    }
}
