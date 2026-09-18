//! From the WIT types an extension returns to the wire types clients draw.
//!
//! Everything here is bounded: an extension cannot make okena ship an
//! unbounded snapshot, and an index that points nowhere is dropped.

use okena_core::context::{ContextKind, ContextOwner, ContextRef};
use okena_core::extension as api;

use crate::bindings::exports::okena::extension::guest as g;
use crate::bindings::okena::extension::types as t;
use crate::bindings::okena::extension::ui_v1 as u;

/// The most rows a table (or items a tree) keeps; the rest are dropped with a note.
pub const MAX_ROWS: usize = 5_000;
/// The most nodes a view keeps.
pub const MAX_NODES: usize = 2_000;

pub fn tone(tone: t::Tone) -> api::Tone {
    match tone {
        t::Tone::Neutral => api::Tone::Neutral,
        t::Tone::Info => api::Tone::Info,
        t::Tone::Success => api::Tone::Success,
        t::Tone::Warning => api::Tone::Warning,
        t::Tone::Danger => api::Tone::Danger,
    }
}

fn badge(b: u::Badge) -> api::ExtBadge {
    api::ExtBadge {
        label: b.label,
        tone: tone(b.tone),
    }
}

fn field(f: u::Field) -> api::ExtField {
    api::ExtField {
        label: f.label,
        value: f.value,
        tone: f.tone.map(tone),
    }
}

fn chart(c: u::Chart) -> api::ExtChart {
    api::ExtChart {
        title: c.title,
        series: c
            .series
            .into_iter()
            .map(|s| api::ExtSeries {
                label: s.label,
                points: s
                    .points
                    .into_iter()
                    .take(MAX_ROWS)
                    .map(|p| api::ExtPoint {
                        label: p.label,
                        value: if p.value.is_finite() { p.value } else { 0.0 },
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn table(t: u::Table) -> api::ExtTable {
    api::ExtTable {
        id: t.id,
        columns: t
            .columns
            .into_iter()
            .map(|c| api::ExtColumn {
                key: c.key,
                label: c.label,
                kind: match c.kind {
                    u::ColumnKind::Text => api::ColumnKind::Text,
                    u::ColumnKind::Number => api::ColumnKind::Number,
                    u::ColumnKind::Badge => api::ColumnKind::Badge,
                    u::ColumnKind::Date => api::ColumnKind::Date,
                },
                groupable: c.groupable,
                sortable: c.sortable,
            })
            .collect(),
        rows: t
            .rows
            .into_iter()
            .take(MAX_ROWS)
            .map(|r| api::ExtRow {
                id: r.id,
                cells: r
                    .cells
                    .into_iter()
                    .map(|c| api::ExtCell {
                        text: c.text,
                        number: c.number.filter(|n| n.is_finite()),
                        tone: c.tone.map(tone),
                    })
                    .collect(),
                detail: r.detail.into_iter().map(field).collect(),
            })
            .collect(),
        group_by: t.group_by,
        sort: t.sort.map(|s| api::ExtSort {
            column: s.column,
            descending: s.descending,
        }),
        row_actions: t.row_actions,
        bulk_actions: t.bulk_actions,
        filter_placeholder: t.filter_placeholder,
        empty_text: t.empty_text,
    }
}

fn tree(t: u::Tree) -> api::ExtTree {
    let items: Vec<api::ExtTreeItem> = t
        .items
        .into_iter()
        .take(MAX_ROWS)
        .map(|i| api::ExtTreeItem {
            id: i.id,
            label: i.label,
            hint: i.hint,
            badge: i.badge.map(badge),
            children: i.children,
            expanded: i.expanded,
            detail: i.detail.into_iter().map(field).collect(),
        })
        .collect();
    let len = items.len() as u32;
    let items = items
        .into_iter()
        .enumerate()
        .map(|(index, mut item)| {
            // Children come after their parent, which rules out cycles.
            item.children.retain(|&c| c < len && c as usize > index);
            item
        })
        .collect();
    api::ExtTree {
        id: t.id,
        items,
        roots: t.roots.into_iter().filter(|&r| r < len).collect(),
        show_detail: t.show_detail,
        empty_text: t.empty_text,
    }
}

fn node(n: u::Node) -> api::ExtNode {
    match n {
        u::Node::Text(t) => api::ExtNode::Text {
            text: t.text,
            style: match t.style {
                u::TextStyle::Body => api::TextStyle::Body,
                u::TextStyle::Heading => api::TextStyle::Heading,
                u::TextStyle::Muted => api::TextStyle::Muted,
                u::TextStyle::Code => api::TextStyle::Code,
            },
            tone: t.tone.map(tone),
        },
        u::Node::Badge(b) => api::ExtNode::Badge { badge: badge(b) },
        u::Node::Badges(bs) => api::ExtNode::Badges {
            badges: bs.into_iter().map(badge).collect(),
        },
        u::Node::Table(t) => api::ExtNode::Table { table: table(t) },
        u::Node::Detail(d) => api::ExtNode::Detail {
            detail: api::ExtDetail {
                title: d.title,
                fields: d.fields.into_iter().map(field).collect(),
            },
        },
        u::Node::Tree(t) => api::ExtNode::Tree { tree: tree(t) },
        u::Node::BarChart(c) => api::ExtNode::BarChart { chart: chart(c) },
        u::Node::LineChart(c) => api::ExtNode::LineChart { chart: chart(c) },
        u::Node::Stats(stats) => api::ExtNode::Stats {
            stats: stats
                .into_iter()
                .map(|s| api::ExtStat {
                    label: s.label,
                    value: s.value,
                    hint: s.hint,
                    tone: s.tone.map(tone),
                })
                .collect(),
        },
        u::Node::Actions(actions) => api::ExtNode::Actions { actions },
        u::Node::Loading(text) => api::ExtNode::Loading { text },
        u::Node::Empty(text) => api::ExtNode::Empty { text },
        u::Node::Error(text) => api::ExtNode::Error { text },
        u::Node::Stack(children) => api::ExtNode::Stack { children },
        u::Node::Columns(children) => api::ExtNode::Columns { children },
        u::Node::Section(s) => api::ExtNode::Section {
            title: s.title,
            children: s.children,
            collapsible: s.collapsible,
            collapsed: s.collapsed,
        },
    }
}

/// The view, with nodes past [`MAX_NODES`] dropped and child indices that
/// point nowhere — or at the node itself or a later one — removed, so a
/// client can walk it without guarding against cycles.
pub fn view(v: u::View) -> api::ExtView {
    let mut nodes: Vec<api::ExtNode> = v.nodes.into_iter().take(MAX_NODES).map(node).collect();
    let len = nodes.len() as u32;
    for (index, node) in nodes.iter_mut().enumerate() {
        if let api::ExtNode::Stack { children }
        | api::ExtNode::Columns { children }
        | api::ExtNode::Section { children, .. } = node
        {
            // Views are built bottom-up, so children come before their
            // container; holding to that rules out cycles.
            children.retain(|&c| (c as usize) < index);
        }
    }
    let root = if v.root < len { v.root } else { 0 };
    if nodes.is_empty() {
        nodes.push(api::ExtNode::Empty {
            text: "Nothing to show".into(),
        });
    }
    api::ExtView { nodes, root }
}

pub fn status(s: u::Status) -> api::ExtStatus {
    api::ExtStatus {
        label: s.label.chars().take(40).collect(),
        tone: s.tone.map(tone),
        tooltip: s.tooltip,
    }
}

fn input(i: g::Input) -> api::ExtInput {
    api::ExtInput {
        key: i.key,
        label: i.label,
        kind: match i.kind {
            g::InputKind::Text => api::InputKind::Text,
            g::InputKind::Multiline => api::InputKind::Multiline,
            g::InputKind::Number => api::InputKind::Number,
            g::InputKind::Toggle => api::InputKind::Toggle,
            g::InputKind::Select => api::InputKind::Select,
        },
        required: i.required,
        default: i.default,
        placeholder: i.placeholder,
        options: i.options,
    }
}

pub fn agent_mode(mode: g::AgentMode) -> api::AgentMode {
    match mode {
        g::AgentMode::Prefill => api::AgentMode::Prefill,
        g::AgentMode::Start => api::AgentMode::Start,
    }
}

pub fn info(info: g::Info) -> (Vec<api::ExtActionDef>, Vec<api::ExtQueryDef>) {
    let actions = info
        .actions
        .into_iter()
        .map(|a| api::ExtActionDef {
            id: a.id,
            label: a.label,
            description: a.description,
            destructive: a.destructive,
            inputs: a.inputs.into_iter().map(input).collect(),
            agent: a.agent.map(agent_mode),
            agent_callable: a.agent_callable,
        })
        .collect();
    let queries = info
        .queries
        .into_iter()
        .map(|q| api::ExtQueryDef {
            id: q.id,
            description: q.description,
            params: q.params.into_iter().map(input).collect(),
        })
        .collect();
    (actions, queries)
}

fn context_ref(c: g::ContextRef) -> Option<ContextRef> {
    let kind = match c.kind {
        g::ContextKind::MapEntry => ContextKind::MapEntry,
        g::ContextKind::Spec => ContextKind::Spec,
        g::ContextKind::Doc => ContextKind::Doc,
        g::ContextKind::Skill => ContextKind::Skill,
        g::ContextKind::Agent => ContextKind::Agent,
    };
    let owner = match (c.project_id, c.store) {
        (Some(project_id), None) => ContextOwner::Project { project_id },
        (None, Some(root_key)) => ContextOwner::Store { root_key },
        _ => return None,
    };
    Some(ContextRef {
        kind,
        owner,
        locator: c.locator,
    })
}

pub fn outcome(o: g::ActionOutcome) -> api::ExtActionOutcome {
    api::ExtActionOutcome {
        message: o.message,
        tone: o.tone.map(tone),
        agent: o.agent.map(|a| api::ExtAgentLaunch {
            goal: a.goal,
            name: a.name,
            root: a.root,
            project_ids: a.project_ids,
            context: a.context.into_iter().filter_map(context_ref).collect(),
            item: a.item,
            item_label: a.item_label,
        }),
        agent_mode: None,
        session_project_id: None,
        refresh: o.refresh,
    }
}

pub fn invoker(invoker: api::Invoker) -> g::Invoker {
    match invoker {
        api::Invoker::Agent => g::Invoker::Agent,
        api::Invoker::User => g::Invoker::User,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> u::Node {
        u::Node::Text(u::Text {
            text: s.into(),
            style: u::TextStyle::Body,
            tone: None,
        })
    }

    #[test]
    fn child_indices_must_point_at_earlier_nodes() {
        let v = view(u::View {
            nodes: vec![
                text("a"),
                u::Node::Stack(vec![0, 1, 2, 7]),
                u::Node::Columns(vec![0, 1]),
            ],
            root: 9,
        });
        assert_eq!(v.root, 0, "a root out of range falls back to the first node");
        assert_eq!(v.nodes[1], api::ExtNode::Stack { children: vec![0] });
        assert_eq!(v.nodes[2], api::ExtNode::Columns { children: vec![0, 1] });
    }

    #[test]
    fn tree_indices_out_of_range_or_backwards_are_dropped() {
        let t = tree(u::Tree {
            id: "t".into(),
            items: vec![u::TreeItem {
                id: "a".into(),
                label: "a".into(),
                hint: None,
                badge: None,
                children: vec![0, 5],
                expanded: true,
                detail: vec![],
            }],
            roots: vec![0, 3],
            show_detail: false,
            empty_text: None,
        });
        assert_eq!(t.roots, vec![0]);
        assert!(t.items[0].children.is_empty());
    }

    #[test]
    fn a_context_ref_needs_exactly_one_owner() {
        let make = |project: Option<&str>, store: Option<&str>| g::ContextRef {
            kind: g::ContextKind::Doc,
            project_id: project.map(Into::into),
            store: store.map(Into::into),
            locator: "docs/a.md".into(),
        };
        assert!(context_ref(make(Some("p"), None)).is_some());
        assert!(context_ref(make(None, Some("store:x"))).is_some());
        assert!(context_ref(make(None, None)).is_none());
        assert!(context_ref(make(Some("p"), Some("store:x"))).is_none());
    }
}
