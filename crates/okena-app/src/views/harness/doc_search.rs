//! Searching the Library from the island (QBL-436, QBL-440).
//!
//! The sidebar shows one origin's tree; a search is across all of them, of
//! every type, by name, path and content. The client has only file names until a file is
//! opened, so the daemon does the matching (`okena_core::doc_search` holds the
//! rules) and this holds what the island is narrowed to, what came back, and
//! the two things drawn from them: the island's chips, and the flat list of
//! matches by origin that stands in for the tree while anything narrows it.

use crate::keybindings::Cancel;
use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text_md, ui_text_ms};
use crate::views::components::SimpleInputState;
use crate::views::components::island::{
    island_bar, island_button, island_chip, island_count, island_menu, island_menu_group,
    island_search_box,
};
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use okena_core::api::ActionRequest;
use okena_core::doc_search::KnowledgeFacet;
use okena_core::library::{LibrarySearchResult, OriginType};
use okena_ui::simple_input::InputChangedEvent;
use std::collections::BTreeSet;
use std::time::Duration;

use super::HarnessPane;

/// How long typing must pause before the daemon is asked. A search reads
/// files; one per keystroke of a word would be several thrown away.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);

/// What a page's island is narrowed to. `Default` is everything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DocFilter {
    /// The search box's text, as typed.
    query: String,
    /// Selected origin keys.
    roots: BTreeSet<String>,
    /// Selected origin types.
    types: BTreeSet<OriginType>,
    /// Selected kinds. Only knowledge files have one, so choosing any leaves
    /// out every file that is not a knowledge file.
    kinds: BTreeSet<KnowledgeFacet>,
}

impl DocFilter {
    /// Whether anything narrows the page. Blank text does not.
    pub(crate) fn is_active(&self) -> bool {
        !self.query.trim().is_empty()
            || !self.roots.is_empty()
            || !self.types.is_empty()
            || !self.kinds.is_empty()
    }

    pub(super) fn set_query(&mut self, text: &str) {
        self.query = text.to_string();
    }

    pub(super) fn root_selected(&self, key: &str) -> bool {
        self.roots.contains(key)
    }

    pub(super) fn toggle_root(&mut self, key: &str) {
        if !self.roots.remove(key) {
            self.roots.insert(key.to_string());
        }
    }

    pub(super) fn type_selected(&self, origin_type: OriginType) -> bool {
        self.types.contains(&origin_type)
    }

    pub(super) fn toggle_type(&mut self, origin_type: OriginType) {
        if !self.types.remove(&origin_type) {
            self.types.insert(origin_type);
        }
    }

    pub(super) fn kind_selected(&self, kind: KnowledgeFacet) -> bool {
        self.kinds.contains(&kind)
    }

    pub(super) fn toggle_kind(&mut self, kind: KnowledgeFacet) {
        if !self.kinds.remove(&kind) {
            self.kinds.insert(kind);
        }
    }

    /// How many values are picked, for the "Filters · 3" on the button. The
    /// text is not one of them.
    pub(super) fn selected_count(&self) -> usize {
        self.roots.len() + self.types.len() + self.kinds.len()
    }

    /// Empty the chips and the text alike: there is one Clear.
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Drop selected origins that are no longer on offer, so one that was
    /// removed does not leave a filter matching nothing behind. The text,
    /// types and kinds are left alone.
    pub(super) fn prune(&mut self, known_roots: &[&str]) {
        self.roots.retain(|key| known_roots.contains(&key.as_str()));
    }

    /// What to ask the daemon — `None` when nothing narrows the page, which
    /// is when the sidebar shows its tree and there is nothing to ask.
    pub(super) fn request(&self) -> Option<ActionRequest> {
        if !self.is_active() {
            return None;
        }
        Some(ActionRequest::LibrarySearch {
            query: self.query.trim().to_string(),
            roots: self.roots.iter().cloned().collect(),
            types: self.types.iter().copied().collect(),
            kinds: self.kinds.iter().copied().collect(),
        })
    }
}

/// One file a search found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hit {
    pub(crate) root_key: String,
    /// Relative to the root: what opening it takes.
    pub(crate) path: String,
    pub(crate) label: String,
    /// What it is, when it is a knowledge file.
    pub(crate) kind: Option<KnowledgeFacet>,
}

/// What a search found, and out of how many.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Found {
    pub(crate) hits: Vec<Hit>,
    pub(crate) total: usize,
}

impl From<LibrarySearchResult> for Found {
    fn from(result: LibrarySearchResult) -> Self {
        Self {
            total: result.total,
            hits: result
                .hits
                .into_iter()
                .map(|h| Hit {
                    root_key: h.root_key,
                    path: h.path,
                    label: h.label,
                    kind: h.facet,
                })
                .collect(),
        }
    }
}

/// A page's filter and the daemon's answer to it.
#[derive(Debug, Default)]
pub(crate) struct DocSearchState {
    pub(crate) filter: DocFilter,
    /// What the daemon found for the filter — or, while a newer question is
    /// out, for the one before it, so the list does not blink on every key.
    /// `None` when nothing narrows the page, and until the first answer.
    pub(crate) found: Option<Found>,
    pub(crate) error: Option<String>,
    /// Bumped on every question, so a slow answer to an older one is dropped
    /// instead of replacing a newer one.
    generation: u64,
}

impl DocSearchState {
    /// The filter changed: what to ask, and the generation the answer must
    /// carry. `None` when nothing narrows the page any more — the results are
    /// dropped, and any answer still on its way with them.
    pub(super) fn begin(&mut self) -> Option<(u64, ActionRequest)> {
        self.generation += 1;
        self.error = None;
        match self.filter.request() {
            Some(request) => Some((self.generation, request)),
            None => {
                self.found = None;
                None
            }
        }
    }

    /// Whether `generation` is still the question being waited on.
    pub(super) fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }

    /// Take an answer. `false` when a newer question has been asked since, and
    /// the answer was dropped.
    pub(super) fn finish(&mut self, generation: u64, result: Result<Found, String>) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        match result {
            Ok(found) => {
                self.found = Some(found);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
        true
    }

    /// How many files are showing and how many there are, once known.
    pub(super) fn shown_of_total(&self) -> (usize, usize) {
        self.found
            .as_ref()
            .map_or((0, 0), |f| (f.hits.len(), f.total))
    }
}

/// The Library's search box and what it is narrowed to. Never saved: a
/// restart starts empty.
pub(crate) struct DocSearch {
    pub(crate) input: Entity<SimpleInputState>,
    pub(crate) state: DocSearchState,
}

impl DocSearch {
    pub(crate) fn new(cx: &mut Context<HarnessPane>) -> Self {
        let input = cx.new(|cx| SimpleInputState::new(cx).placeholder("Search the library"));
        cx.subscribe(
            &input,
            move |this: &mut HarnessPane, input, _: &InputChangedEvent, cx| {
                let text = input.read(cx).value().to_string();
                this.search.state.filter.set_query(&text);
                this.run_doc_search(true, cx);
            },
        )
        .detach();
        Self {
            input,
            state: DocSearchState::default(),
        }
    }
}

/// Hits under the origin each was found in, in the order the sidebar lists
/// the origins. An origin nothing was found in is left out; a hit whose origin
/// is not listed any more keeps its place at the end, under its key.
pub(crate) fn group_hits<'a>(
    hits: &'a [Hit],
    roots: &[(String, String)],
) -> Vec<(String, Vec<&'a Hit>)> {
    let mut groups: Vec<(String, Vec<&Hit>)> = Vec::new();
    for (key, name) in roots {
        let found: Vec<&Hit> = hits.iter().filter(|h| &h.root_key == key).collect();
        if !found.is_empty() {
            groups.push((name.clone(), found));
        }
    }
    for hit in hits {
        if roots.iter().any(|(key, _)| key == &hit.root_key) {
            continue;
        }
        match groups.iter_mut().find(|(name, _)| name == &hit.root_key) {
            Some((_, found)) => found.push(hit),
            None => groups.push((hit.root_key.clone(), vec![hit])),
        }
    }
    groups
}

/// What a click on one of the island's chips changes.
#[derive(Clone)]
enum Chip {
    Root(String),
    Type(OriginType),
    Kind(KnowledgeFacet),
}

impl HarnessPane {
    /// Whether the island is narrowing the Library, which is when the sidebar
    /// shows matches instead of its tree.
    pub(super) fn doc_search_active(&self) -> bool {
        self.search.state.filter.is_active()
    }

    /// The origins a search covers, as `(key, name)` in sidebar order: the
    /// usable ones, which are the ones the daemon reads.
    pub(super) fn doc_search_roots(&self) -> Vec<(String, String)> {
        self.library
            .origins
            .iter()
            .flat_map(|o| &o.origins)
            .filter(|o| o.healthy)
            .map(|o| (o.key.clone(), o.name.clone()))
            .collect()
    }

    /// The origin types a search can be narrowed to: those that have a usable
    /// origin, in the order the sidebar groups them.
    fn doc_search_types(&self) -> Vec<OriginType> {
        OriginType::all()
            .into_iter()
            .filter(|t| {
                self.library
                    .origins
                    .iter()
                    .flat_map(|o| &o.origins)
                    .any(|o| o.healthy && o.origin_type == *t)
            })
            .collect()
    }

    /// The filter changed: ask the daemon what it leaves, or go back to the
    /// tree when it leaves everything. `typing` waits for a pause first.
    pub(super) fn run_doc_search(&mut self, typing: bool, cx: &mut Context<Self>) {
        let Some((generation, request)) = self.search.state.begin() else {
            cx.notify();
            return;
        };
        // The matches are listed in the sidebar, so a search needs it open.
        if !self.files.open {
            self.toggle_file_sidebar(cx);
        }
        cx.notify();

        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            if typing {
                cx.background_executor().timer(SEARCH_DEBOUNCE).await;
                let current =
                    this.update(cx, |this, _| this.search.state.is_current(generation));
                if !matches!(current, Ok(true)) {
                    return;
                }
            }
            let result = smol::unblock(move || -> Result<Found, String> {
                let value = client
                    .post_action(request)?
                    .ok_or_else(|| "Missing search results".to_string())?;
                serde_json::from_value::<LibrarySearchResult>(value)
                    .map(Found::from)
                    .map_err(|e| format!("Unexpected search results: {e}"))
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                if this.search.state.finish(generation, result) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The origins were listed again: forget chips for origins that have
    /// gone, and search again if anything still narrows the page, since a
    /// refresh is how files added, renamed or deleted get noticed.
    pub(super) fn doc_search_refreshed(&mut self, cx: &mut Context<Self>) {
        let roots = self.doc_search_roots();
        let known: Vec<&str> = roots.iter().map(|(key, _)| key.as_str()).collect();
        let search = &mut self.search;
        search.state.filter.prune(&known);
        if search.state.filter.is_active() || search.state.found.is_some() {
            self.run_doc_search(false, cx);
        }
    }

    pub(super) fn clear_doc_search(&mut self, cx: &mut Context<Self>) {
        self.search.state.filter.clear();
        let input = self.search.input.clone();
        // Emits a change, which runs the now empty search and so drops the
        // results; run it here too in case the box was already empty.
        input.update(cx, |input, cx| input.set_value("", cx));
        self.run_doc_search(false, cx);
    }

    /// The Library island: the search box, the Filters button, then "N of M"
    /// and Clear while anything narrows the page. The button's menu holds a
    /// chip per origin, per origin type and per kind of knowledge file.
    pub(super) fn render_doc_island(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let search = &self.search;
        let filter = &search.state.filter;
        let active = filter.is_active();
        let (shown, total) = search.state.shown_of_total();

        let bar = island_bar("docs-island", cx)
            .child(
                island_search_box("doc-search", &search.input, cx).on_action(cx.listener(
                    |this, _: &Cancel, window, cx| this.cancel_island_search(window, cx),
                )),
            )
            .child(self.island_filters_toggle(filter.selected_count(), cx))
            // No count until the daemon has answered: "0 of 0" would read as
            // a result.
            .children(
                (active && search.state.found.is_some()).then(|| island_count(shown, total, cx)),
            )
            .children(active.then(|| {
                island_button("doc-search-clear", "Clear", cx).on_click(
                    cx.listener(move |this, _, _window, cx| this.clear_doc_search(cx)),
                )
            }))
            .child(self.island_close_button("docs-island-close", cx));

        let menu = self.island_menu_open.then(|| {
            let chip = |id: String, label: String, on: bool, chip: Chip, cx: &mut Context<Self>| {
                island_chip(SharedString::from(id), label, on, cx).on_click(cx.listener(
                    move |this, _, _window, cx| {
                        let filter = &mut this.search.state.filter;
                        match &chip {
                            Chip::Root(key) => filter.toggle_root(key),
                            Chip::Type(origin_type) => filter.toggle_type(*origin_type),
                            Chip::Kind(kind) => filter.toggle_kind(*kind),
                        }
                        this.run_doc_search(false, cx);
                    },
                ))
            };
            let roots: Vec<_> = self
                .doc_search_roots()
                .into_iter()
                .map(|(key, name)| {
                    let on = filter.root_selected(&key);
                    chip(
                        format!("doc-search-root-{key}"),
                        name,
                        on,
                        Chip::Root(key),
                        cx,
                    )
                })
                .collect();
            let mut menu =
                island_menu("docs-island-menu", cx).child(island_menu_group("Origin", roots, cx));
            let types = self.doc_search_types();
            // One type on offer is not a choice.
            if types.len() > 1 {
                let chips: Vec<_> = types
                    .iter()
                    .map(|origin_type| {
                        chip(
                            format!("doc-search-type-{}", origin_type.slug()),
                            origin_type.label().to_string(),
                            filter.type_selected(*origin_type),
                            Chip::Type(*origin_type),
                            cx,
                        )
                    })
                    .collect();
                menu = menu.child(island_menu_group("Type", chips, cx));
            }
            // A kind is what a knowledge file is, so the chips are offered
            // only where there is knowledge to narrow.
            if types.contains(&OriginType::Knowledge) {
                let kinds: Vec<_> = KnowledgeFacet::all()
                    .into_iter()
                    .map(|kind| {
                        chip(
                            format!("doc-search-kind-{}", kind.label()),
                            kind.label().to_string(),
                            filter.kind_selected(kind),
                            Chip::Kind(kind),
                            cx,
                        )
                    })
                    .collect();
                menu = menu.child(island_menu_group("Kind", kinds, cx));
            }
            menu.into_any_element()
        });

        Some(self.island_with_menu(menu, bar, cx))
    }

    /// The sidebar while the island narrows the page: every match, under the
    /// origin it is in. Stands where the tree stands, and gives way to it again
    /// once the search and filters are cleared.
    pub(super) fn render_doc_results(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let mut col = self.file_sidebar_column("doc-search-results");
        let search = &self.search;
        let note = |text: String| {
            div()
                .px(px(6.0))
                .py(px(3.0))
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(text)
        };
        if let Some(error) = search.state.error.clone() {
            col = col.child(
                div()
                    .px(px(6.0))
                    .pt(px(10.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.error))
                    .child(error),
            );
        }
        let Some(found) = search.state.found.as_ref() else {
            if search.state.error.is_none() {
                col = col.child(div().pt(px(10.0)).child(note("Searching…".into())));
            }
            return col.into_any_element();
        };
        col =
            col.child(self.section_label(&format!("{} of {}", found.hits.len(), found.total), cx));
        if found.hits.is_empty() {
            col = col.child(note(
                "Nothing matches this search and these filters.".into(),
            ));
            col = col.child(
                div()
                    .id("doc-search-no-match-clear")
                    .px(px(6.0))
                    .py(px(3.0))
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(t.border_active))
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .child("Clear")
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.clear_doc_search(cx);
                    })),
            );
            return col.into_any_element();
        }

        let (open_root, open_path) = (
            self.library.root_key.as_deref(),
            self.library.selected.as_deref(),
        );
        for (root_name, hits) in group_hits(&found.hits, &self.doc_search_roots()) {
            col = col.child(self.section_label(&root_name, cx));
            for hit in hits {
                let selected = open_root == Some(hit.root_key.as_str())
                    && open_path == Some(hit.path.as_str());
                let (root_key, path) = (hit.root_key.clone(), hit.path.clone());
                col = col.child(
                    h_flex()
                        .id(SharedString::from(format!(
                            "doc-search-hit-{}-{}",
                            hit.root_key, hit.path
                        )))
                        .cursor_pointer()
                        .w_full()
                        .min_w_0()
                        .items_center()
                        .gap(px(6.0))
                        .pl(px(10.0))
                        .pr(px(8.0))
                        .py(px(3.0))
                        .rounded(px(3.0))
                        .when(selected, |d| d.bg(with_alpha(t.button_primary_bg, 0.18)))
                        .when(!selected, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(ui_text_md(cx))
                                .text_color(rgb(if selected {
                                    t.text_primary
                                } else {
                                    t.text_secondary
                                }))
                                .child(hit.label.clone()),
                        )
                        .children(hit.kind.map(|kind| {
                            div()
                                .flex_shrink_0()
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_muted))
                                .child(kind.label())
                        }))
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            // The Roots page stands where the document does;
                            // a match you clicked has to be what you see.
                            if this.roots.open {
                                this.close_roots_page(cx);
                            }
                            this.open_library_doc(root_key.clone(), path.clone(), cx);
                        })),
                );
            }
        }
        col.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{DocFilter, DocSearchState, Found, Hit, group_hits};
    use crate::views::harness::HarnessSection;
    use okena_core::api::ActionRequest;
    use okena_core::doc_search::KnowledgeFacet;
    use okena_core::library::OriginType;

    fn hit(root: &str, path: &str) -> Hit {
        Hit {
            root_key: root.into(),
            path: path.into(),
            label: path.into(),
            kind: None,
        }
    }

    fn found(hits: &[(&str, &str)], total: usize) -> Found {
        Found {
            hits: hits.iter().map(|(r, p)| hit(r, p)).collect(),
            total,
        }
    }

    #[test]
    fn nothing_selected_and_blank_text_asks_nothing() {
        let mut f = DocFilter::default();
        assert!(!f.is_active());
        assert!(f.request().is_none());
        f.set_query("   ");
        assert!(!f.is_active(), "blank text is not a search");
        assert!(f.request().is_none());
    }

    #[test]
    fn a_request_carries_trimmed_text_origins_types_and_kinds() {
        let mut f = DocFilter::default();
        f.set_query("  Smoke ");
        f.toggle_root("knowledge:store:acme");
        f.toggle_type(OriginType::Knowledge);
        f.toggle_kind(KnowledgeFacet::Partial);
        f.toggle_kind(KnowledgeFacet::Brief);
        let Some(ActionRequest::LibrarySearch {
            query,
            roots,
            types,
            kinds,
        }) = f.request()
        else {
            panic!("expected a library search");
        };
        assert_eq!(query, "Smoke");
        assert_eq!(roots, ["knowledge:store:acme"]);
        assert_eq!(types, [OriginType::Knowledge]);
        assert_eq!(kinds, [KnowledgeFacet::Partial, KnowledgeFacet::Brief]);
    }

    #[test]
    fn an_origin_or_a_type_alone_is_a_search() {
        let mut f = DocFilter::default();
        f.toggle_root("spec:path:/repo");
        assert!(f.is_active(), "choosing an origin narrows without any text");
        let Some(ActionRequest::LibrarySearch { query, roots, .. }) = f.request() else {
            panic!("expected a library search");
        };
        assert_eq!(query, "");
        assert_eq!(roots, ["spec:path:/repo"]);

        let mut by_type = DocFilter::default();
        by_type.toggle_type(OriginType::Freeform);
        assert!(by_type.is_active() && by_type.type_selected(OriginType::Freeform));
        assert_eq!(by_type.selected_count(), 1);
        by_type.toggle_type(OriginType::Freeform);
        assert!(!by_type.is_active());
    }

    #[test]
    fn a_chip_toggles_off_and_clear_empties_everything() {
        let mut f = DocFilter::default();
        f.toggle_root("a");
        f.toggle_root("a");
        assert!(!f.root_selected("a"));
        assert!(!f.is_active());

        f.toggle_root("a");
        f.toggle_kind(KnowledgeFacet::Skill);
        f.set_query("x");
        assert!(f.root_selected("a") && f.kind_selected(KnowledgeFacet::Skill));
        assert_eq!(f.selected_count(), 2, "the text is not a picked value");
        f.clear();
        assert_eq!(f, DocFilter::default());
    }

    #[test]
    fn pruning_drops_a_root_that_is_gone_and_keeps_the_rest() {
        let mut f = DocFilter::default();
        f.toggle_root("store:gone");
        f.toggle_root("store:acme");
        f.toggle_kind(KnowledgeFacet::Doc);
        f.set_query("x");
        f.prune(&["store:acme", "store:other"]);
        assert!(!f.root_selected("store:gone"));
        assert!(f.root_selected("store:acme"));
        assert!(f.kind_selected(KnowledgeFacet::Doc));
        assert!(f.is_active());
    }

    #[test]
    fn an_answer_to_an_older_question_is_dropped() {
        let mut s = DocSearchState::default();
        s.filter.set_query("a");
        let (first, _) = s.begin().expect("a search");
        s.filter.set_query("ab");
        let (second, _) = s.begin().expect("a search");

        // The newer answer lands first; the older one must not replace it.
        assert!(s.finish(second, Ok(found(&[("r", "ab.md")], 5))));
        assert!(!s.finish(first, Ok(found(&[("r", "a.md"), ("r", "ab.md")], 5))));
        assert_eq!(s.shown_of_total(), (1, 5));
    }

    #[test]
    fn clearing_drops_the_results_and_any_answer_still_on_its_way() {
        let mut s = DocSearchState::default();
        s.filter.set_query("a");
        let (asked, _) = s.begin().expect("a search");
        assert!(s.finish(asked, Ok(found(&[("r", "a.md")], 3))));
        assert!(s.found.is_some());

        s.filter.set_query("b");
        let (late, _) = s.begin().expect("a search");
        s.filter.clear();
        assert!(s.begin().is_none());
        assert!(s.found.is_none(), "cleared: the tree comes back");
        assert!(!s.finish(late, Ok(found(&[("r", "b.md")], 3))));
        assert!(s.found.is_none());
    }

    #[test]
    fn a_failed_search_keeps_the_last_results_and_says_so() {
        let mut s = DocSearchState::default();
        s.filter.set_query("a");
        let (g, _) = s.begin().expect("a search");
        s.finish(g, Ok(found(&[("r", "a.md")], 3)));
        s.filter.set_query("ab");
        let (g, _) = s.begin().expect("a search");
        assert!(s.finish(g, Err("daemon unreachable".into())));
        assert_eq!(s.error.as_deref(), Some("daemon unreachable"));
        assert_eq!(s.shown_of_total(), (1, 3));
        // Asking again clears the complaint.
        s.begin();
        assert!(s.error.is_none());
    }

    #[test]
    fn hits_are_grouped_under_their_roots_in_sidebar_order() {
        let hits = [
            hit("store:ops", "docs/a.md"),
            hit("store:eng", "docs/b.md"),
            hit("store:ops", "docs/c.md"),
            hit("store:gone", "docs/d.md"),
        ];
        let roots = vec![
            ("store:eng".to_string(), "Engineering".to_string()),
            ("store:quiet".to_string(), "Quiet".to_string()),
            ("store:ops".to_string(), "Ops".to_string()),
        ];
        let groups: Vec<(String, Vec<&str>)> = group_hits(&hits, &roots)
            .into_iter()
            .map(|(name, hits)| (name, hits.iter().map(|h| h.path.as_str()).collect()))
            .collect();
        assert_eq!(
            groups,
            [
                ("Engineering".to_string(), vec!["docs/b.md"]),
                // A root nothing was found in has no heading.
                ("Ops".to_string(), vec!["docs/a.md", "docs/c.md"]),
                // A root no longer listed keeps its hits, under its key.
                ("store:gone".to_string(), vec!["docs/d.md"]),
            ]
        );
    }

    // ---- the island on the page ----

    use super::super::island::tests::pane_in_window;
    use gpui::TestAppContext;

    fn origins(keys: &[&str]) -> okena_core::library::LibraryOrigins {
        let origins: Vec<serde_json::Value> = keys
            .iter()
            .map(|key| {
                let origin_type = key.split(':').next().unwrap_or("knowledge");
                serde_json::json!({
                    "key": key, "type": origin_type, "kind": "store", "name": key,
                    "path": format!("/{key}"), "healthy": true,
                })
            })
            .collect();
        serde_json::from_value(serde_json::json!({ "origins": origins })).expect("library origins")
    }

    #[gpui::test]
    fn the_island_appears_with_the_roots_and_takes_the_cursor(cx: &mut TestAppContext) {
        let (pane, _, cx) = pane_in_window(HarnessSection::Library, cx);
        cx.update(|window, cx| {
            pane.update(cx, |p, cx| {
                // Nothing listed yet: no island, and the key is let through.
                assert!(!p.focus_island_search(window, cx));

                p.library.origins = Some(origins(&["knowledge:store:eng", "spec:store:ops"]));
                p.set_island_open(false, window, cx);
                assert!(p.focus_island_search(window, cx));
                assert!(p.island_open, "a closed island is shown first");
                assert!(
                    p.search
                        .input
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                );
                // One search over every type of origin.
                assert_eq!(
                    p.doc_search_roots(),
                    [
                        (
                            "knowledge:store:eng".to_string(),
                            "knowledge:store:eng".to_string()
                        ),
                        ("spec:store:ops".to_string(), "spec:store:ops".to_string()),
                    ]
                );
                assert_eq!(
                    p.doc_search_types(),
                    [OriginType::Knowledge, OriginType::Spec]
                );
            });
        });
    }

    #[gpui::test]
    fn typing_swaps_the_tree_for_results_and_escape_brings_it_back(cx: &mut TestAppContext) {
        let (pane, window_focus, cx) = pane_in_window(HarnessSection::Library, cx);
        cx.update(|window, cx| {
            pane.update(cx, |p, cx| {
                p.library.origins = Some(origins(&["knowledge:store:eng"]));
                p.files.open = false;
                p.focus_island_search(window, cx);
                p.search
                    .input
                    .update(cx, |i, cx| i.set_value("zeppelin", cx));
            });
        });
        cx.update(|window, cx| {
            pane.update(cx, |p, cx| {
                assert!(p.doc_search_active());
                assert!(p.files.open, "the matches are listed in the sidebar");

                // Esc: the text goes, focus goes back to the window.
                p.cancel_island_search(window, cx);
                assert_eq!(p.search.input.read(cx).value(), "");
                assert!(window_focus.is_focused(window));
            });
        });
        pane.update(cx, |p, _| {
            assert!(!p.doc_search_active());
            assert!(p.search.state.found.is_none(), "the tree is back");
        });
    }

    #[gpui::test]
    fn a_chip_narrows_without_text_and_clear_brings_the_tree_back(cx: &mut TestAppContext) {
        let (pane, _, cx) = pane_in_window(HarnessSection::Library, cx);
        pane.update(cx, |p, cx| {
            p.library.origins = Some(origins(&["knowledge:store:eng", "knowledge:store:gone"]));
            let filter = &mut p.search.state.filter;
            filter.toggle_kind(KnowledgeFacet::Partial);
            filter.toggle_root("knowledge:store:gone");
            p.run_doc_search(false, cx);
            assert!(p.doc_search_active());

            // The roots are listed again without the one that was chosen.
            p.library.origins = Some(origins(&["knowledge:store:eng"]));
            p.doc_search_refreshed(cx);
            let filter = &p.search.state.filter;
            assert!(!filter.root_selected("knowledge:store:gone"));
            assert!(filter.kind_selected(KnowledgeFacet::Partial));

            p.clear_doc_search(cx);
            assert!(!p.doc_search_active());
            assert!(p.search.state.found.is_none());
        });
    }
}
