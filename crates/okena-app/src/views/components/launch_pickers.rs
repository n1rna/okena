//! Projects and context for a launch, picked as chips (QBL-406).
//!
//! What every agent launcher shows between saying what to do and choosing an
//! agent: the projects the agent is pointed at, and the map entries, specs,
//! knowledge docs, skills and agents it is handed. One component, so every
//! launcher picks both the same way and none of them grows its own wall of
//! toggle pills.
//!
//! Projects are filtered here, from the workspace. Context is searched by the
//! daemon (`ContextSearch`), whose index ranks the chosen projects' items — and
//! the stores they follow — first; a chosen project that has never been
//! scanned gets a hint row that starts its scan.

use crate::theme::theme;
use crate::ui::tokens::ui_text_ms;
use crate::workspace::state::Workspace;
use gpui::prelude::*;
use gpui::*;
use gpui_component::v_flex;
use okena_core::api::ActionRequest;
use okena_core::context::{
    ContextItem, ContextKind, ContextOwner, ContextRef, ContextSearchResult,
};
use okena_transport::remote_action::RemoteActionClient;
use okena_ui::chip_search::{ChipGroup, ChipHint, ChipItem, ChipSearch, ChipSearchEvent};
use std::collections::HashMap;
use std::time::Duration;

/// How long typing settles before the daemon is asked.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);
/// Results asked for per search: the list scrolls, and past this a search
/// wants more letters rather than more rows.
const CONTEXT_LIMIT: usize = 50;

/// Something the host shows depends on changed.
pub enum LaunchPickersEvent {
    /// The picked projects changed, so a title counting them is stale.
    ProjectsChanged,
}

pub struct LaunchPickers {
    client: RemoteActionClient,
    workspace: Entity<Workspace>,
    projects: Entity<ChipSearch>,
    context: Entity<ChipSearch>,
    /// What each context chip id stands for.
    refs: HashMap<SharedString, ContextRef>,
    /// Bumped on every context search, so a slow answer to an old query is
    /// dropped rather than replacing a newer one.
    generation: u64,
    /// Under the projects, what picking them does on this launcher.
    projects_hint: SharedString,
    /// Under the context: a scan started, or a search that failed.
    note: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<LaunchPickersEvent> for LaunchPickers {}

impl LaunchPickers {
    pub fn new(
        id: &str,
        client: RemoteActionClient,
        workspace: Entity<Workspace>,
        projects_hint: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> Self {
        let projects = cx.new(|cx| {
            ChipSearch::new(
                format!("{id}-projects"),
                "Search projects by name or path",
                cx,
            )
            .empty_text("No project matches.")
        });
        let context = cx.new(|cx| {
            ChipSearch::new(
                format!("{id}-context"),
                "Search map entries, specs, knowledge and skills",
                cx,
            )
            .empty_text("Nothing matches.")
        });
        let subscriptions = vec![
            cx.subscribe(
                &projects,
                |this, _, event: &ChipSearchEvent, cx| match event {
                    ChipSearchEvent::QueryChanged(query) => this.filter_projects(query, cx),
                    ChipSearchEvent::Added(_) | ChipSearchEvent::Removed(_) => {
                        // The chosen projects rank context, and decide which of
                        // them is reported unmapped.
                        this.search_context(cx);
                        cx.emit(LaunchPickersEvent::ProjectsChanged);
                        cx.notify();
                    }
                    ChipSearchEvent::Hint(_) | ChipSearchEvent::Picked(_) => {}
                },
            ),
            cx.subscribe(
                &context,
                |this, _, event: &ChipSearchEvent, cx| match event {
                    ChipSearchEvent::QueryChanged(_) => this.search_context(cx),
                    ChipSearchEvent::Added(item) => this.record_hit(item, cx),
                    ChipSearchEvent::Removed(_) | ChipSearchEvent::Picked(_) => {}
                    ChipSearchEvent::Hint(project_id) => this.scan(project_id.to_string(), cx),
                },
            ),
        ];
        let mut this = Self {
            client,
            workspace,
            projects,
            context,
            refs: HashMap::new(),
            generation: 0,
            projects_hint: projects_hint.into(),
            note: None,
            _subscriptions: subscriptions,
        };
        this.filter_projects("", cx);
        this
    }

    /// The picked projects' ids, in the order they were picked.
    pub fn project_ids(&self, cx: &App) -> Vec<String> {
        self.projects
            .read(cx)
            .chips()
            .iter()
            .map(|c| c.id.to_string())
            .collect()
    }

    /// The same, as the daemon names them: a remote project's id without its
    /// connection prefix.
    pub fn daemon_project_ids(&self, cx: &App) -> Vec<String> {
        self.project_ids(cx)
            .iter()
            .map(|id| okena_transport::client::strip_prefix(id, self.client.connection_id()))
            .collect()
    }

    /// Pick `ids` without asking — a launcher preselecting where a one-click
    /// start would go. Ids that are not projects any more are skipped.
    pub fn set_project_ids(&mut self, ids: &[String], cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        let chips: Vec<ChipItem> = ids
            .iter()
            .filter_map(|id| ws.project(id))
            .map(|p| project_item(&p.id, &p.name, &p.path))
            .collect();
        self.projects
            .update(cx, |search, cx| search.set_chips(chips, cx));
        self.search_context(cx);
        cx.emit(LaunchPickersEvent::ProjectsChanged);
        cx.notify();
    }

    /// The picked context, as the daemon resolves it again at launch.
    pub fn context_refs(&self, cx: &App) -> Vec<ContextRef> {
        self.context
            .read(cx)
            .chips()
            .iter()
            .filter_map(|c| self.refs.get(&c.id).cloned())
            .collect()
    }

    /// Forget the picked context — after a launch, which used it.
    pub fn clear_context(&mut self, cx: &mut Context<Self>) {
        self.context
            .update(cx, |search, cx| search.set_chips(Vec::new(), cx));
        self.note = None;
        cx.notify();
    }

    fn filter_projects(&mut self, query: &str, cx: &mut Context<Self>) {
        let candidates: Vec<(String, String, String)> = self
            .workspace
            .read(cx)
            .projects()
            .iter()
            // Repositories: a worktree belongs to work in flight, and a
            // session is an agent, so neither is somewhere to point one.
            .filter(|p| p.worktree_info.is_none() && !p.is_any_agent_session())
            .map(|p| (p.id.clone(), p.name.clone(), p.path.clone()))
            .collect();
        let results = match_projects(&candidates, query)
            .into_iter()
            .map(|(id, name, path)| project_item(id, name, path))
            .collect();
        self.projects
            .update(cx, |search, cx| search.set_results(results, cx));
    }

    /// Ask the daemon for context matching the box, once typing settles.
    fn search_context(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let query = self.context.read(cx).query(cx);
        let project_ids = self.daemon_project_ids(cx);
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            smol::Timer::after(SEARCH_DEBOUNCE).await;
            let current = cx.update(|cx| {
                this.update(cx, |this, _| this.generation == generation)
                    .unwrap_or(false)
            });
            if !current {
                return;
            }
            let result = smol::unblock(move || {
                client.post_action(ActionRequest::ContextSearch {
                    query,
                    project_ids,
                    terminal_id: None,
                    limit: Some(CONTEXT_LIMIT),
                })
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return;
                    }
                    match result.and_then(|v| {
                        serde_json::from_value::<ContextSearchResult>(v.unwrap_or_default())
                            .map_err(|e| e.to_string())
                    }) {
                        Ok(found) => this.show_context(found, cx),
                        Err(e) => this.note = Some(format!("Could not search context: {e}")),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn show_context(&mut self, found: ContextSearchResult, cx: &mut Context<Self>) {
        let mut results = Vec::with_capacity(found.items.len());
        for item in found.items {
            let id = chip_id(&item.reference);
            let mut chip = ChipItem::new(id.clone(), item.title.clone())
                .kind(item.reference.kind.label())
                .icon(kind_icon(item.reference.kind))
                .description(item.description.clone())
                .group(ChipGroup {
                    id: format!("{:?}", item.reference.owner).into(),
                    name: item.owner_name.clone().into(),
                    icon: Some(owner_icon(&item.reference.owner).into()),
                });
            for tag in context_tags(&item) {
                chip = chip.tag(tag);
            }
            self.refs.insert(id, item.reference);
            results.push(chip);
        }
        let hints = found
            .unmapped
            .into_iter()
            .map(|p| ChipHint {
                id: p.project_id.into(),
                text: format!(
                    "{} isn't mapped yet, so its map entries can't show.",
                    p.name
                )
                .into(),
                action: "Scan".into(),
            })
            .collect();
        self.context.update(cx, |search, cx| {
            search.set_results(results, cx);
            search.set_hints(hints, cx);
        });
    }

    /// Adding an item ranks it higher next time.
    fn record_hit(&mut self, item: &ChipItem, _cx: &mut Context<Self>) {
        let Some(reference) = self.refs.get(&item.id).cloned() else {
            return;
        };
        let client = self.client.clone();
        smol::spawn(async move {
            let _ = smol::unblock(move || {
                client.post_action(ActionRequest::ContextHit { item: reference })
            })
            .await;
        })
        .detach();
    }

    /// Start a scan of `project_id` (a daemon id, from the hint).
    fn scan(&mut self, project_id: String, cx: &mut Context<Self>) {
        self.note = Some("Starting the scan…".into());
        cx.notify();
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client.post_action(ActionRequest::ProjectScan {
                    project_id,
                    agent_command: None,
                })
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.note = Some(match result {
                        Ok(_) => "Scan started — its map entries show here once the map \
                                  is written."
                            .to_string(),
                        Err(e) => format!("Could not start the scan: {e}"),
                    });
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn caption(&self, text: impl Into<SharedString>, cx: &App) -> Div {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(text.into())
    }

    fn label(&self, text: &'static str, cx: &App) -> Div {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_secondary))
            .child(text)
    }
}

impl Render for LaunchPickers {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .min_w_0()
            .gap(px(10.0))
            .child(
                v_flex()
                    .w_full()
                    .min_w_0()
                    .gap(px(4.0))
                    .child(self.label("Projects", cx))
                    .child(self.projects.clone())
                    // A host whose hint depends on its own state draws it.
                    .when(!self.projects_hint.is_empty(), |d| {
                        d.child(self.caption(self.projects_hint.clone(), cx))
                    }),
            )
            .child(
                v_flex()
                    .w_full()
                    .min_w_0()
                    .gap(px(4.0))
                    .child(self.label("Context", cx))
                    .child(self.context.clone())
                    .child(self.caption(
                        self.note.clone().unwrap_or_else(|| {
                            "Listed in the brief by path; skills load into the session \
                             where the agent can take them."
                                .to_string()
                        }),
                        cx,
                    )),
            )
    }
}

fn project_item(id: &str, name: &str, path: &str) -> ChipItem {
    ChipItem::new(id.to_string(), name.to_string()).description(path.to_string())
}

/// What a context result's row shows for its kind; the kind's name is its
/// tooltip.
fn kind_icon(kind: ContextKind) -> &'static str {
    match kind {
        ContextKind::MapEntry => "icons/map.svg",
        ContextKind::Spec => "icons/file-text.svg",
        ContextKind::Doc => "icons/book-open.svg",
        ContextKind::Skill => "icons/sparkles.svg",
        ContextKind::Agent => "icons/bot.svg",
    }
}

/// The labels at the end of a context result's row: a map entry's id, what
/// the item is, and whose it is.
fn context_tags(item: &ContextItem) -> Vec<String> {
    let mut tags: Vec<String> = item.map_id.iter().cloned().collect();
    tags.push(kind_tag(item));
    tags.push(item.owner_name.clone());
    tags
}

/// What an item is, a little finer than its kind: a change is not a spec, and
/// a map entry says which part of the map it is.
fn kind_tag(item: &ContextItem) -> String {
    match item.reference.kind {
        ContextKind::MapEntry => {
            let section = item
                .map_id
                .as_deref()
                .and_then(|id| id.split_once(':'))
                .map(|(section, _)| section);
            match section {
                Some("area") => "Area",
                Some("concept") => "Concept",
                Some("exposes") => "Exposes",
                Some("consumes") => "Consumes",
                Some("ci") => "CI",
                Some("infrastructure") => "Infrastructure",
                _ => "Map entry",
            }
            .to_string()
        }
        ContextKind::Spec if item.reference.locator.split('/').any(|s| s == "changes") => {
            "Change".to_string()
        }
        ContextKind::Doc => "Doc".to_string(),
        kind => kind.label().to_string(),
    }
}

/// What a group's header shows: a project, or a store.
fn owner_icon(owner: &ContextOwner) -> &'static str {
    match owner {
        ContextOwner::Project { .. } => "icons/folder.svg",
        ContextOwner::Store { .. } => "icons/library.svg",
    }
}

/// The chip id a context ref is picked under.
fn chip_id(reference: &ContextRef) -> SharedString {
    format!(
        "{:?}|{:?}|{}",
        reference.kind, reference.owner, reference.locator
    )
    .into()
}

/// The projects matching `query`, in the order they were added to the
/// workspace: a case-insensitive match on the name or the path.
pub fn match_projects<'a>(
    candidates: &'a [(String, String, String)],
    query: &str,
) -> Vec<(&'a str, &'a str, &'a str)> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    candidates
        .iter()
        .filter(|(_, name, path)| {
            let haystack = format!("{} {}", name.to_lowercase(), path.to_lowercase());
            words.iter().all(|w| haystack.contains(w.as_str()))
        })
        .map(|(id, name, path)| (id.as_str(), name.as_str(), path.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{context_tags, kind_icon, match_projects, owner_icon};
    use okena_core::context::{ContextItem, ContextKind, ContextOwner, ContextRef};

    fn item(kind: ContextKind, locator: &str, map_id: Option<&str>) -> ContextItem {
        ContextItem {
            reference: ContextRef {
                kind,
                owner: ContextOwner::project("p"),
                locator: locator.to_string(),
            },
            title: "t".into(),
            description: "d".into(),
            owner_name: "shop".into(),
            path: "/x".into(),
            map_id: map_id.map(str::to_string),
            chosen: true,
        }
    }

    #[test]
    fn tags_name_the_map_id_a_finer_kind_and_the_origin() {
        let tags = |i: ContextItem| context_tags(&i);
        assert_eq!(
            tags(item(ContextKind::MapEntry, "area:cart", Some("area:cart"))),
            ["area:cart", "Area", "shop"]
        );
        assert_eq!(
            tags(item(ContextKind::MapEntry, "ci:build", Some("ci:build"))),
            ["ci:build", "CI", "shop"]
        );
        assert_eq!(
            tags(item(ContextKind::Spec, "openspec/specs/cart/spec.md", None)),
            ["Spec", "shop"]
        );
        // A change folder is a change, not a spec — but a spec named after
        // changes is still a spec.
        assert_eq!(
            tags(item(
                ContextKind::Spec,
                "openspec/changes/add-gift-cards",
                None
            )),
            ["Change", "shop"]
        );
        assert_eq!(
            tags(item(
                ContextKind::Spec,
                "openspec/specs/changes-feed/spec.md",
                None
            )),
            ["Spec", "shop"]
        );
        assert_eq!(
            tags(item(ContextKind::Doc, "docs/ci.md", None)),
            ["Doc", "shop"]
        );
        assert_eq!(
            tags(item(ContextKind::Skill, "skills/release/SKILL.md", None)),
            ["Skill", "shop"]
        );
    }

    #[test]
    fn every_kind_and_owner_has_an_icon_that_ships() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let mut icons: Vec<&str> = ContextKind::all().into_iter().map(kind_icon).collect();
        icons.push(owner_icon(&ContextOwner::project("p")));
        icons.push(owner_icon(&ContextOwner::store("store:kb")));
        for icon in &icons {
            assert!(assets.join(icon).is_file(), "{icon} is missing");
        }
        // One icon per kind, and a project's differs from a store's.
        let mut distinct = icons.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), icons.len());
    }

    fn workspace(n: usize) -> Vec<(String, String, String)> {
        (0..n)
            .map(|i| {
                (
                    format!("p{i}"),
                    format!("service-{i}"),
                    format!("/Users/me/p/team-{}/service-{i}", i % 7),
                )
            })
            .collect()
    }

    #[test]
    fn typing_part_of_a_name_narrows_a_large_workspace_in_added_order() {
        let mut projects = workspace(150);
        projects.push(("okena".into(), "Okena".into(), "/Users/me/p/okena".into()));
        let ids = |q: &str| -> Vec<String> {
            match_projects(&projects, q)
                .into_iter()
                .map(|(id, _, _)| id.to_string())
                .collect()
        };
        assert_eq!(ids("oke"), ["okena"]);
        assert_eq!(ids("OKENA"), ["okena"]);
        // Every project for an empty box, in the order they were added.
        assert_eq!(ids("").len(), 151);
        assert_eq!(ids("")[..3], ["p0", "p1", "p2"]);
        // Ten through nineteen, then a hundred and ten through nineteen.
        let teens = ids("service-1");
        assert_eq!(teens[..3], ["p1", "p10", "p11"]);
        assert!(teens.iter().all(|id| id.starts_with("p1")));
    }

    #[test]
    fn a_path_and_several_words_match_too() {
        let projects = workspace(30);
        let by_path: Vec<&str> = match_projects(&projects, "team-3")
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        assert_eq!(by_path, ["p3", "p10", "p17", "p24"]);
        let both: Vec<&str> = match_projects(&projects, "team-3 service-17")
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        assert_eq!(both, ["p17"]);
        assert!(match_projects(&projects, "nothing-like-it").is_empty());
    }
}
