//! The canvas's geometry and graph, independent of how it is drawn.
//!
//! Everything here is in *canvas units* — pixels at 100% zoom — and pure, so
//! placement, edge anchoring and the pan/zoom transform can be tested without
//! a window.

use okena_core::project_map::{InterfaceKind, LinkSource, ProjectLinks, ProjectMap};
use std::collections::{HashMap, HashSet, VecDeque};

/// Width of every project card.
pub const CARD_WIDTH: f32 = 280.0;
/// The card's title row.
pub const HEADER_HEIGHT: f32 = 46.0;
/// Space around the areas inside a card.
pub const CARD_PADDING: f32 = 10.0;
/// One area box.
pub const AREA_HEIGHT: f32 = 44.0;
/// Between area boxes.
pub const AREA_GAP: f32 = 8.0;
/// The body of a card with no areas to show: a status line.
pub const EMPTY_BODY_HEIGHT: f32 = 40.0;
/// Between cards in the automatic layout, across: room for edges and labels.
pub const LAYOUT_GAP_X: f32 = 160.0;
/// Between rows of cards in the automatic layout.
pub const LAYOUT_GAP_Y: f32 = 90.0;

pub const MIN_ZOOM: f32 = 0.25;
pub const MAX_ZOOM: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn union(&self, other: &Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect {
            x,
            y,
            w: self.right().max(other.right()) - x,
            h: self.bottom().max(other.bottom()) - y,
        }
    }
}

/// How tall a card is for a project showing `areas` areas.
pub fn card_height(areas: usize) -> f32 {
    if areas == 0 {
        HEADER_HEIGHT + EMPTY_BODY_HEIGHT
    } else {
        HEADER_HEIGHT + 2.0 * CARD_PADDING + areas as f32 * (AREA_HEIGHT + AREA_GAP) - AREA_GAP
    }
}

/// A card at `origin` showing `areas` areas.
pub fn card_rect(origin: (f32, f32), areas: usize) -> Rect {
    Rect {
        x: origin.0,
        y: origin.1,
        w: CARD_WIDTH,
        h: card_height(areas),
    }
}

/// Area `index` of a card at `origin`.
pub fn area_rect(origin: (f32, f32), index: usize) -> Rect {
    Rect {
        x: origin.0 + CARD_PADDING,
        y: origin.1 + HEADER_HEIGHT + CARD_PADDING + index as f32 * (AREA_HEIGHT + AREA_GAP),
        w: CARD_WIDTH - 2.0 * CARD_PADDING,
        h: AREA_HEIGHT,
    }
}

/// One edge on the canvas: `consumer` uses `provider` through one interface,
/// drawn from the area that uses it to the area that serves it where the maps
/// name them, and from the project card otherwise.
#[derive(Clone, Debug, PartialEq)]
pub struct CanvasEdge {
    /// Daemon project ids.
    pub consumer: String,
    pub provider: String,
    /// Index into the consumer map's `areas`, when its `consumes` entry names
    /// one.
    pub consumer_area: Option<usize>,
    /// Index into the provider map's `areas`, when its `exposes` entry names
    /// one.
    pub provider_area: Option<usize>,
    pub kind: InterfaceKind,
    pub name: String,
    pub source: LinkSource,
    /// Listed in only one of the two maps.
    pub one_sided: bool,
}

impl CanvasEdge {
    /// What the edge's label says.
    pub fn label(&self) -> String {
        format!("{} {}", self.kind.id(), self.name)
    }
}

/// The edges to draw, from the matched links and the maps of the projects on
/// the canvas. `maps` is keyed by daemon project id; a link whose ends are not
/// both on the canvas is left out.
pub fn canvas_edges(links: &ProjectLinks, maps: &HashMap<String, ProjectMap>) -> Vec<CanvasEdge> {
    let mut edges = Vec::new();
    for link in &links.links {
        let (Some(consumer), Some(provider)) = (maps.get(&link.consumer), maps.get(&link.provider))
        else {
            continue;
        };
        let consumer_areas = interface_areas(consumer, &consumer.consumes, link.kind, &link.name);
        let provider_areas = interface_areas(provider, &provider.exposes, link.kind, &link.name);
        for consumer_area in &consumer_areas {
            for provider_area in &provider_areas {
                edges.push(CanvasEdge {
                    consumer: link.consumer.clone(),
                    provider: link.provider.clone(),
                    consumer_area: *consumer_area,
                    provider_area: *provider_area,
                    kind: link.kind,
                    name: link.name.clone(),
                    source: link.source,
                    one_sided: link.listed_only_by.is_some(),
                });
            }
        }
    }
    edges
}

/// The areas of `map` an interface in `list` names, as area indices; `[None]`
/// when the map does not say, so the edge still has one end on the card.
fn interface_areas(
    map: &ProjectMap,
    list: &[okena_core::project_map::Interface],
    kind: InterfaceKind,
    name: &str,
) -> Vec<Option<usize>> {
    let indices: Vec<Option<usize>> = list
        .iter()
        .find(|i| i.kind == kind && i.name.trim() == name.trim())
        .map(|i| {
            i.areas
                .iter()
                .filter_map(|id| map.areas.iter().position(|a| &a.id == id))
                .map(Some)
                .collect()
        })
        .unwrap_or_default();
    if indices.is_empty() {
        vec![None]
    } else {
        indices
    }
}

/// Where each card goes when nobody has placed it.
///
/// `projects` is `(id, card height)` in the overview's order. Linked projects
/// are laid out one after another, so they land beside each other in the grid:
/// each connected group starts from its most-linked project and grows outward
/// from it, and unlinked projects follow in overview order.
pub fn auto_layout(
    projects: &[(String, f32)],
    edges: &[CanvasEdge],
) -> HashMap<String, (f32, f32)> {
    let index: HashMap<&str, usize> = projects
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (id.as_str(), i))
        .collect();
    let mut neighbours: Vec<Vec<usize>> = vec![Vec::new(); projects.len()];
    let mut seen_pairs = HashSet::new();
    for edge in edges {
        let (Some(&a), Some(&b)) = (
            index.get(edge.consumer.as_str()),
            index.get(edge.provider.as_str()),
        ) else {
            continue;
        };
        if a != b && seen_pairs.insert((a.min(b), a.max(b))) {
            neighbours[a].push(b);
            neighbours[b].push(a);
        }
    }
    let degree = |i: usize| neighbours[i].len();
    // Most-linked first; overview order breaks ties, so the result is stable.
    let by_degree = |a: &usize, b: &usize| degree(*b).cmp(&degree(*a)).then(a.cmp(b));

    let mut order = Vec::with_capacity(projects.len());
    let mut placed = vec![false; projects.len()];
    let mut starts: Vec<usize> = (0..projects.len()).collect();
    starts.sort_by(by_degree);
    for start in starts {
        if placed[start] {
            continue;
        }
        placed[start] = true;
        let mut queue = VecDeque::from([start]);
        while let Some(current) = queue.pop_front() {
            order.push(current);
            let mut next: Vec<usize> = neighbours[current]
                .iter()
                .copied()
                .filter(|n| !placed[*n])
                .collect();
            next.sort_by(by_degree);
            for n in next {
                placed[n] = true;
                queue.push_back(n);
            }
        }
    }

    let columns = (projects.len() as f32).sqrt().ceil().max(1.0) as usize;
    let mut positions = HashMap::with_capacity(projects.len());
    let mut row_top = 0.0;
    for row in order.chunks(columns) {
        let mut row_height: f32 = 0.0;
        for (column, &i) in row.iter().enumerate() {
            let (id, height) = &projects[i];
            positions.insert(
                id.clone(),
                (column as f32 * (CARD_WIDTH + LAYOUT_GAP_X), row_top),
            );
            row_height = row_height.max(*height);
        }
        row_top += row_height + LAYOUT_GAP_Y;
    }
    positions
}

/// The curve an edge follows from `from` to `to`: leaving the side of `from`
/// that faces `to` and entering the facing side of `to`. Returns the start,
/// both control points and the end.
pub fn edge_curve(from: Rect, to: Rect) -> [(f32, f32); 4] {
    let (from_cx, from_cy) = from.center();
    let (to_cx, to_cy) = to.center();
    let (start, end) = if to_cx >= from_cx {
        ((from.right(), from_cy), (to.x, to_cy))
    } else {
        ((from.x, from_cy), (to.right(), to_cy))
    };
    let pull = ((end.0 - start.0).abs() / 2.0).max(40.0);
    let direction = if end.0 >= start.0 { 1.0 } else { -1.0 };
    [
        start,
        (start.0 + pull * direction, start.1),
        (end.0 - pull * direction, end.1),
        end,
    ]
}

/// The point at `t` (0 to 1) along a curve from [`edge_curve`].
pub fn curve_point(curve: [(f32, f32); 4], t: f32) -> (f32, f32) {
    let [p0, c1, c2, p3] = curve;
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    (
        a * p0.0 + b * c1.0 + c * c2.0 + d * p3.0,
        a * p0.1 + b * c1.1 + c * c2.1 + d * p3.1,
    )
}

/// The point halfway along a curve from [`edge_curve`], where its label goes.
pub fn curve_midpoint(curve: [(f32, f32); 4]) -> (f32, f32) {
    curve_point(curve, 0.5)
}

/// Which part of the canvas is on screen: the screen offset of the canvas
/// origin, and the zoom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}

impl Default for View {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            zoom: 1.0,
        }
    }
}

impl View {
    pub fn to_screen(self, point: (f32, f32)) -> (f32, f32) {
        (point.0 * self.zoom + self.x, point.1 * self.zoom + self.y)
    }

    pub fn to_canvas(self, point: (f32, f32)) -> (f32, f32) {
        (
            (point.0 - self.x) / self.zoom,
            (point.1 - self.y) / self.zoom,
        )
    }

    pub fn rect_to_screen(self, rect: Rect) -> Rect {
        let (x, y) = self.to_screen((rect.x, rect.y));
        Rect {
            x,
            y,
            w: rect.w * self.zoom,
            h: rect.h * self.zoom,
        }
    }

    /// Zoom by `factor`, keeping the canvas point under the screen point
    /// `anchor` where it is — the way a map zooms toward the cursor.
    pub fn zoomed_at(self, factor: f32, anchor: (f32, f32)) -> View {
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let under = self.to_canvas(anchor);
        View {
            x: anchor.0 - under.0 * zoom,
            y: anchor.1 - under.1 * zoom,
            zoom,
        }
    }

    /// The view that fits `content` into a screen of `size`, centred, with
    /// room around it, never zoomed in past 100%.
    pub fn fit(content: Rect, size: (f32, f32)) -> View {
        const MARGIN: f32 = 48.0;
        let usable = (
            (size.0 - 2.0 * MARGIN).max(1.0),
            (size.1 - 2.0 * MARGIN).max(1.0),
        );
        let zoom = (usable.0 / content.w.max(1.0))
            .min(usable.1 / content.h.max(1.0))
            .clamp(MIN_ZOOM, 1.0);
        let (cx, cy) = content.center();
        View {
            x: size.0 / 2.0 - cx * zoom,
            y: size.1 / 2.0 - cy * zoom,
            zoom,
        }
    }
}

/// The smallest rectangle around every card.
pub fn content_bounds(cards: impl IntoIterator<Item = Rect>) -> Option<Rect> {
    cards.into_iter().reduce(|all, card| all.union(&card))
}

#[cfg(test)]
mod tests {
    // A glob is safe here: this module imports nothing from gpui, so it
    // cannot shadow `#[test]`.
    use super::*;
    use okena_core::project_map::{LinkedProject, ProjectLink};

    /// A map with `rest` merged over a minimal project.
    fn map(rest: serde_json::Value) -> ProjectMap {
        let mut v = serde_json::json!({
            "version": 1,
            "project": { "name": "p", "description": "d" },
        });
        let obj = v.as_object_mut().expect("object");
        for (k, value) in rest.as_object().expect("object") {
            obj.insert(k.clone(), value.clone());
        }
        serde_json::from_value(v).expect("map")
    }

    fn area(id: &str) -> serde_json::Value {
        serde_json::json!({ "id": id, "description": "d", "paths": ["src"] })
    }

    fn link(consumer: &str, provider: &str, name: &str, source: LinkSource) -> ProjectLink {
        ProjectLink {
            consumer: consumer.into(),
            provider: provider.into(),
            kind: InterfaceKind::Topic,
            name: name.into(),
            source,
            listed_only_by: None,
        }
    }

    fn links(list: Vec<ProjectLink>) -> ProjectLinks {
        ProjectLinks {
            projects: Vec::<LinkedProject>::new(),
            links: list,
            unmatched: Vec::new(),
            unresolved: Vec::new(),
        }
    }

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
    }

    #[test]
    fn an_edge_runs_between_the_areas_the_maps_name() {
        let mut maps = HashMap::new();
        maps.insert(
            "worker".to_string(),
            map(serde_json::json!({
                "areas": [area("jobs"), area("mail")],
                "consumes": [{ "type": "topic", "name": "invoice-issued", "areas": ["mail"] }],
            })),
        );
        maps.insert(
            "billing".to_string(),
            map(serde_json::json!({
                "areas": [area("invoices")],
                "exposes": [{ "type": "topic", "name": "invoice-issued", "areas": ["invoices"] }],
            })),
        );
        let edges = canvas_edges(
            &links(vec![link(
                "worker",
                "billing",
                "invoice-issued",
                LinkSource::Matched,
            )]),
            &maps,
        );
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].consumer_area, Some(1));
        assert_eq!(edges[0].provider_area, Some(0));
        assert_eq!(edges[0].label(), "topic invoice-issued");
    }

    #[test]
    fn an_edge_falls_back_to_the_card_when_no_area_is_named() {
        let mut maps = HashMap::new();
        // Found by scan: neither map has the interface at all.
        maps.insert("api".to_string(), map(serde_json::json!({})));
        maps.insert("accounts".to_string(), map(serde_json::json!({})));
        let mut one_sided = link("api", "accounts", "acme.accounts", LinkSource::FoundByScan);
        one_sided.listed_only_by = Some("api".into());
        let edges = canvas_edges(&links(vec![one_sided]), &maps);
        assert_eq!(edges.len(), 1);
        assert_eq!(
            (edges[0].consumer_area, edges[0].provider_area),
            (None, None)
        );
        assert!(edges[0].one_sided);
    }

    #[test]
    fn an_interface_used_by_two_areas_draws_an_edge_from_each() {
        let mut maps = HashMap::new();
        maps.insert(
            "api".to_string(),
            map(serde_json::json!({
                "areas": [area("http"), area("jobs")],
                "consumes": [{ "type": "topic", "name": "t", "areas": ["http", "jobs"] }],
            })),
        );
        maps.insert("billing".to_string(), map(serde_json::json!({})));
        let edges = canvas_edges(
            &links(vec![link("api", "billing", "t", LinkSource::Matched)]),
            &maps,
        );
        let from: Vec<_> = edges.iter().map(|e| e.consumer_area).collect();
        assert_eq!(from, [Some(0), Some(1)]);
    }

    #[test]
    fn a_link_to_a_project_not_on_the_canvas_is_not_drawn() {
        let mut maps = HashMap::new();
        maps.insert("api".to_string(), map(serde_json::json!({})));
        assert!(
            canvas_edges(
                &links(vec![link("api", "hidden", "t", LinkSource::Matched)]),
                &maps
            )
            .is_empty()
        );
    }

    fn edge(consumer: &str, provider: &str) -> CanvasEdge {
        CanvasEdge {
            consumer: consumer.into(),
            provider: provider.into(),
            consumer_area: None,
            provider_area: None,
            kind: InterfaceKind::Http,
            name: "x".into(),
            source: LinkSource::Matched,
            one_sided: false,
        }
    }

    fn slots(positions: &HashMap<String, (f32, f32)>) -> Vec<(String, (f32, f32))> {
        let mut v: Vec<_> = positions.iter().map(|(k, p)| (k.clone(), *p)).collect();
        v.sort_by(|a, b| (a.1.1, a.1.0).partial_cmp(&(b.1.1, b.1.0)).expect("finite"));
        v
    }

    #[test]
    fn linked_projects_are_placed_next_to_each_other() {
        // Overview order a, b, c, d; only a and d are linked.
        let projects: Vec<(String, f32)> = ["a", "b", "c", "d"]
            .iter()
            .map(|id| (id.to_string(), 100.0))
            .collect();
        let positions = auto_layout(&projects, &[edge("a", "d")]);
        let order: Vec<String> = slots(&positions).into_iter().map(|(id, _)| id).collect();
        assert_eq!(order, ["a", "d", "b", "c"]);
        // Two columns for four cards; the second row sits below the first.
        assert_eq!(positions["d"], (CARD_WIDTH + LAYOUT_GAP_X, 0.0));
        assert_eq!(positions["b"], (0.0, 100.0 + LAYOUT_GAP_Y));
    }

    #[test]
    fn the_most_linked_project_starts_its_group() {
        let projects: Vec<(String, f32)> = ["a", "hub", "c"]
            .iter()
            .map(|id| (id.to_string(), 80.0))
            .collect();
        let positions = auto_layout(&projects, &[edge("a", "hub"), edge("c", "hub")]);
        assert_eq!(slots(&positions)[0].0, "hub");
    }

    #[test]
    fn layout_handles_nothing_and_is_stable() {
        assert!(auto_layout(&[], &[]).is_empty());
        let projects: Vec<(String, f32)> = ["a", "b", "c"]
            .iter()
            .map(|id| (id.to_string(), 50.0))
            .collect();
        let edges = [edge("b", "c")];
        assert_eq!(
            auto_layout(&projects, &edges),
            auto_layout(&projects, &edges)
        );
    }

    #[test]
    fn a_row_is_as_tall_as_its_tallest_card() {
        let projects = vec![
            ("a".to_string(), 300.0),
            ("b".to_string(), 90.0),
            ("c".to_string(), 90.0),
        ];
        let positions = auto_layout(&projects, &[]);
        assert_eq!(positions["c"].1, 300.0 + LAYOUT_GAP_Y);
    }

    #[test]
    fn card_geometry_stacks_areas_under_the_title() {
        assert_eq!(card_height(0), HEADER_HEIGHT + EMPTY_BODY_HEIGHT);
        let origin = (100.0, 50.0);
        let first = area_rect(origin, 0);
        let second = area_rect(origin, 1);
        assert_eq!(first.y, 50.0 + HEADER_HEIGHT + CARD_PADDING);
        assert_eq!(second.y - first.y, AREA_HEIGHT + AREA_GAP);
        assert_eq!(
            card_rect(origin, 2).bottom(),
            second.bottom() + CARD_PADDING
        );
    }

    #[test]
    fn an_edge_leaves_the_side_facing_its_target() {
        let left = Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 40.0,
        };
        let right = Rect {
            x: 300.0,
            y: 200.0,
            w: 100.0,
            h: 40.0,
        };
        let forward = edge_curve(left, right);
        assert_eq!(forward[0], (100.0, 20.0));
        assert_eq!(forward[3], (300.0, 220.0));
        let backward = edge_curve(right, left);
        assert_eq!(backward[0], (300.0, 220.0));
        assert_eq!(backward[3], (100.0, 20.0));
        assert!(close(curve_midpoint(forward), (200.0, 120.0)));
        assert!(close(curve_point(forward, 0.0), forward[0]));
        assert!(close(curve_point(forward, 1.0), forward[3]));
    }

    #[test]
    fn zooming_keeps_the_point_under_the_cursor_still() {
        let view = View {
            x: 40.0,
            y: -20.0,
            zoom: 1.0,
        };
        let cursor = (300.0, 200.0);
        let under = view.to_canvas(cursor);
        let zoomed = view.zoomed_at(1.5, cursor);
        assert!(close(zoomed.to_screen(under), cursor));
        assert_eq!(zoomed.zoom, 1.5);
        assert_eq!(view.zoomed_at(100.0, cursor).zoom, MAX_ZOOM);
        assert_eq!(view.zoomed_at(0.001, cursor).zoom, MIN_ZOOM);
    }

    #[test]
    fn fitting_centres_the_content_and_never_zooms_past_100_percent() {
        let small = Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
        };
        let view = View::fit(small, (1000.0, 800.0));
        assert_eq!(view.zoom, 1.0);
        assert!(close(view.to_screen(small.center()), (500.0, 400.0)));

        let wide = Rect {
            x: -500.0,
            y: 0.0,
            w: 4000.0,
            h: 400.0,
        };
        let view = View::fit(wide, (1000.0, 800.0));
        assert!(view.zoom < 1.0);
        let left = view.to_screen((wide.x, wide.y)).0;
        let right = view.to_screen((wide.right(), wide.y)).0;
        assert!(left >= 0.0 && right <= 1000.0, "{left}..{right}");
    }

    #[test]
    fn content_bounds_covers_every_card() {
        let a = Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        };
        let b = Rect {
            x: 50.0,
            y: -20.0,
            w: 10.0,
            h: 10.0,
        };
        assert_eq!(
            content_bounds([a, b]),
            Some(Rect {
                x: 0.0,
                y: -20.0,
                w: 60.0,
                h: 30.0
            })
        );
        assert_eq!(content_bounds(Vec::new()), None);
    }
}
