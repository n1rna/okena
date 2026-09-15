//! What okena knows about one project, independent of how it is shown.
//!
//! The facts the harness Projects view used to gather into a lane per repo,
//! read here for a single project so they can sit beside its own terminal.

use crate::workspace::state::{ProjectData, Workspace};
use okena_core::api::{ApiGitStatus, CiStatus, PrState, RepoPullRequest};
use okena_core::context::{ContextItem, ContextKind, ContextOwner};
use okena_core::knowledge::KnowledgeStores;
use okena_core::project_map::{InterfaceKind, ProjectLinks};
use std::collections::HashSet;
use std::path::Path;

/// Git facts for one checkout: branch, diff, divergence, PR and pipeline.
#[derive(Clone, Debug, Default)]
pub struct GitFacts {
    pub branch: Option<String>,
    pub lines_added: usize,
    pub lines_removed: usize,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    pub pr: Option<(u32, PrState)>,
    pub ci: Option<(CiStatus, usize, usize, usize)>,
}

impl GitFacts {
    /// Read from the daemon snapshot that already feeds the column header, so
    /// the panel and the header chip cannot disagree.
    pub fn collect(ws: &Workspace, project_id: &str) -> Self {
        let Some(g) = ws
            .remote_snapshot(project_id)
            .and_then(|snap| snap.git_status.as_ref())
        else {
            return Self::default();
        };
        Self {
            branch: g.branch.clone(),
            lines_added: g.lines_added,
            lines_removed: g.lines_removed,
            ahead: g.ahead,
            behind: g.behind,
            pr: g.pr_info.as_ref().map(|pr| (pr.number, pr.state.clone())),
            ci: g
                .ci_checks
                .as_ref()
                .map(|c| (c.status.clone(), c.passed, c.failed, c.pending)),
        }
    }

    /// Whether anything has changed in the checkout — and so whether a diff is
    /// worth offering at all.
    pub fn has_changes(&self) -> bool {
        self.lines_added > 0 || self.lines_removed > 0
    }
}

/// What the project is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectInfoKind {
    Repo,
    /// A worktree, with the name of the repo it was created from while that is
    /// still open.
    Worktree {
        repo: Option<String>,
    },
}

/// Everything the panel shows about one project.
#[derive(Clone, Debug)]
pub struct ProjectInfo {
    pub project_id: String,
    pub path: String,
    pub kind: ProjectInfoKind,
    pub git: GitFacts,
    /// Worktrees open against a repo, by id. Always empty for a worktree.
    pub worktrees: Vec<String>,
    /// Agent sessions working this project's tasks, by id.
    pub sessions: Vec<String>,
    /// Every open pull request in the project's GitHub repository — a
    /// worktree's is its repository's. `None` when there is no list: no
    /// github.com remote, no token, or no poll answered yet.
    pub pull_requests: Option<Vec<RepoPullRequest>>,
}

impl ProjectInfo {
    /// Collect everything shown about `project_id`, or `None` if it is gone.
    pub fn collect(ws: &Workspace, project_id: &str) -> Option<Self> {
        let project = ws.project(project_id)?;
        let worktrees: Vec<&ProjectData> = project
            .worktree_ids
            .iter()
            .filter_map(|id| ws.project(id))
            .collect();
        let kind = match &project.worktree_info {
            Some(info) => ProjectInfoKind::Worktree {
                repo: ws
                    .project(&info.parent_project_id)
                    .map(|parent| parent.name.clone()),
            },
            None => ProjectInfoKind::Repo,
        };
        let tasks = task_ids(project, worktrees.iter().copied());

        Some(Self {
            project_id: project.id.clone(),
            path: project.path.clone(),
            kind,
            git: GitFacts::collect(ws, project_id),
            worktrees: worktrees.iter().map(|w| w.id.clone()).collect(),
            sessions: sessions_working(ws.projects(), &tasks),
            pull_requests: pull_requests_of(
                ws.remote_snapshot(project_id)
                    .and_then(|snap| snap.git_status.as_ref()),
            ),
        })
    }
}

/// The open pull requests a project shows, from the daemon snapshot that
/// feeds its header — so a local and a remote project read alike.
pub(super) fn pull_requests_of(git: Option<&ApiGitStatus>) -> Option<Vec<RepoPullRequest>> {
    git?.repo_pull_requests.clone()
}

/// What a PR row says under its title: its number, who opened it, and the
/// branch it merges from and into.
pub(super) fn pr_caption(pr: &RepoPullRequest) -> String {
    let mut parts = vec![format!("#{}", pr.pr.number)];
    parts.extend(pr.author.clone());
    let branches = match (pr.head.as_str(), pr.pr.base.as_deref()) {
        ("", None) => None,
        (head, None) => Some(head.to_string()),
        ("", Some(base)) => Some(format!("→ {base}")),
        (head, Some(base)) => Some(format!("{head} → {base}")),
    };
    parts.extend(branches);
    parts.join(" · ")
}

/// Tasks a project has work in flight for: its own, when one was started on the
/// project itself, and those of the worktrees open against it.
pub(super) fn task_ids<'a>(
    project: &'a ProjectData,
    worktrees: impl Iterator<Item = &'a ProjectData>,
) -> HashSet<String> {
    project
        .linked_tasks()
        .chain(worktrees.flat_map(|w| w.linked_tasks()))
        .map(|t| t.id.external_id.clone())
        .collect()
}

/// Agent sessions working any of `task_ids`, in workspace order.
///
/// Matched by task rather than by directory: a session is rooted above the
/// repos precisely so one agent can span several, so it has no path that would
/// place it under a project. A worktree carries its task too, but it is a
/// checkout, not a session, and is never listed as one.
pub(super) fn sessions_working(
    projects: &[ProjectData],
    task_ids: &HashSet<String>,
) -> Vec<String> {
    projects
        .iter()
        .filter(|p| p.worktree_info.is_none() && p.is_any_agent_session())
        .filter(|p| {
            p.linked_tasks()
                .any(|t| task_ids.contains(&t.id.external_id))
        })
        .map(|p| p.id.clone())
        .collect()
}

/// A header of the project's menu. Declared in the order the menu lists them:
/// the map's parts, then specs, knowledge, skills, agents, and links last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MenuGroup {
    Areas,
    Concepts,
    Exposes,
    Consumes,
    Ci,
    Infrastructure,
    /// A map entry of a part this build does not know.
    Map,
    Specs,
    Knowledge,
    Skills,
    Agents,
    Links,
}

impl MenuGroup {
    pub fn id(self) -> &'static str {
        match self {
            MenuGroup::Areas => "areas",
            MenuGroup::Concepts => "concepts",
            MenuGroup::Exposes => "exposes",
            MenuGroup::Consumes => "consumes",
            MenuGroup::Ci => "ci",
            MenuGroup::Infrastructure => "infrastructure",
            MenuGroup::Map => "map",
            MenuGroup::Specs => "specs",
            MenuGroup::Knowledge => "knowledge",
            MenuGroup::Skills => "skills",
            MenuGroup::Agents => "agents",
            MenuGroup::Links => "links",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            MenuGroup::Areas => "Areas",
            MenuGroup::Concepts => "Concepts",
            MenuGroup::Exposes => "Exposes",
            MenuGroup::Consumes => "Consumes",
            MenuGroup::Ci => "CI/CD",
            MenuGroup::Infrastructure => "Infrastructure",
            MenuGroup::Map => "Map",
            MenuGroup::Specs => "Specs",
            MenuGroup::Knowledge => "Knowledge",
            MenuGroup::Skills => "Skills",
            MenuGroup::Agents => "Agents",
            MenuGroup::Links => "Links",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            MenuGroup::Areas | MenuGroup::Map => "icons/map.svg",
            MenuGroup::Concepts => "icons/lightbulb.svg",
            MenuGroup::Exposes | MenuGroup::Consumes => "icons/arrow-up-down.svg",
            MenuGroup::Ci => "icons/play.svg",
            MenuGroup::Infrastructure => "icons/settings.svg",
            MenuGroup::Specs => "icons/file-text.svg",
            MenuGroup::Knowledge => "icons/book-open.svg",
            MenuGroup::Skills => "icons/sparkles.svg",
            MenuGroup::Agents => "icons/bot.svg",
            MenuGroup::Links => "icons/link.svg",
        }
    }

    /// Where an item of the context index is listed.
    fn of(item: &ContextItem) -> Self {
        match item.reference.kind {
            ContextKind::MapEntry => {
                match item.map_id.as_deref().and_then(|id| id.split_once(':')) {
                    Some(("area", _)) => MenuGroup::Areas,
                    Some(("concept", _)) => MenuGroup::Concepts,
                    Some(("exposes", _)) => MenuGroup::Exposes,
                    Some(("consumes", _)) => MenuGroup::Consumes,
                    Some(("ci", _)) => MenuGroup::Ci,
                    Some(("infrastructure", _)) => MenuGroup::Infrastructure,
                    _ => MenuGroup::Map,
                }
            }
            ContextKind::Spec => MenuGroup::Specs,
            ContextKind::Doc => MenuGroup::Knowledge,
            ContextKind::Skill => MenuGroup::Skills,
            ContextKind::Agent => MenuGroup::Agents,
        }
    }
}

/// What picking a menu row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuPick {
    /// Open a file, by its absolute path on the daemon's machine.
    File(String),
    /// Open another project's info panel, by its daemon id.
    Project(String),
    /// Nothing to open: a link that names nothing okena has.
    Nothing,
}

/// One row of the project's menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuEntry {
    /// Unique within the menu.
    pub id: String,
    pub group: MenuGroup,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub pick: MenuPick,
}

/// Everything the menu lists for `project_id` (a daemon id): the context items
/// it owns, then its links, grouped in [`MenuGroup`] order and each group in
/// the order it was found. Items owned by anything else — a store it follows,
/// another project — are left out.
pub(super) fn menu_entries(
    items: &[ContextItem],
    project_id: &str,
    links: Option<&ProjectLinks>,
) -> Vec<MenuEntry> {
    let owner = ContextOwner::project(project_id);
    let mut entries: Vec<MenuEntry> = items
        .iter()
        .filter(|item| item.reference.owner == owner)
        .map(|item| {
            let is_change = item.reference.kind == ContextKind::Spec
                && item.reference.locator.split('/').any(|s| s == "changes");
            // A change is a folder: its proposal is what to read first.
            let path = if is_change {
                format!("{}/proposal.md", item.path.trim_end_matches('/'))
            } else {
                item.path.clone()
            };
            MenuEntry {
                id: format!("{:?}|{}", item.reference.kind, item.reference.locator),
                group: MenuGroup::of(item),
                title: item.title.clone(),
                description: item.description.clone(),
                tags: item
                    .map_id
                    .iter()
                    .cloned()
                    .chain(is_change.then(|| "Change".to_string()))
                    .collect(),
                pick: MenuPick::File(path),
            }
        })
        .collect();
    if let Some(links) = links {
        entries.extend(link_entries(links, project_id));
    }
    // Stable: each group keeps the order its items came in.
    entries.sort_by_key(|e| e.group);
    entries
}

/// A project's links as menu rows, with the text the panel's link cards had.
fn link_entries(links: &ProjectLinks, me: &str) -> Vec<MenuEntry> {
    let name_of = |id: &str| links.project(id).map_or(id.to_string(), |p| p.name.clone());
    let mut out = Vec::new();
    for (label, list, other_is_provider) in [
        ("Uses", links.uses(me).collect::<Vec<_>>(), true),
        ("Used by", links.used_by(me).collect::<Vec<_>>(), false),
    ] {
        for (i, link) in list.into_iter().enumerate() {
            let other = if other_is_provider {
                &link.provider
            } else {
                &link.consumer
            };
            let mut tags = vec![label.to_string(), link.source.label().to_string()];
            if let Some(only) = &link.listed_only_by {
                tags.push(format!("only in {}'s map", name_of(only)));
            }
            out.push(MenuEntry {
                id: format!("link|{label}|{i}"),
                group: MenuGroup::Links,
                title: name_of(other),
                description: format!("{} {}", link.kind.label(), link.name),
                tags,
                pick: MenuPick::Project(other.clone()),
            });
        }
    }
    for (i, u) in links.unmatched.iter().filter(|u| u.project == me).enumerate() {
        out.push(MenuEntry {
            id: format!("link|unmatched|{i}"),
            group: MenuGroup::Links,
            title: u.interface.name.clone(),
            description: u.interface.kind.label().to_string(),
            tags: vec!["No scanned project exposes".to_string()],
            pick: MenuPick::Nothing,
        });
    }
    for (i, u) in links.unresolved.iter().filter(|u| u.project == me).enumerate() {
        out.push(MenuEntry {
            id: format!("link|unresolved|{i}"),
            group: MenuGroup::Links,
            title: u.link.project.clone(),
            description: format!("{} {}", u.link.kind.label(), u.link.name),
            tags: vec!["Names no scanned project".to_string()],
            pick: MenuPick::Nothing,
        });
    }
    out
}

/// Whether a row matches what is typed: every word, in any case, somewhere in
/// its title, description or tags.
pub(super) fn entry_matches(entry: &MenuEntry, query: &str) -> bool {
    let haystack = std::iter::once(entry.title.as_str())
        .chain(std::iter::once(entry.description.as_str()))
        .chain(entry.tags.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    query
        .split_whitespace()
        .all(|word| haystack.contains(&word.to_lowercase()))
}

/// One other project on the panel's compact links rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkChip {
    /// Its daemon id.
    pub project_id: String,
    pub name: String,
    /// `kind name` of each interface the link is made of.
    pub interfaces: Vec<String>,
}

/// The panel's links, compact: who this project uses, who uses it — one chip
/// per other project, in the order first linked — and how many links match or
/// name nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompactLinks {
    pub uses: Vec<LinkChip>,
    pub used_by: Vec<LinkChip>,
    pub unresolved: usize,
}

pub(super) fn compact_links(links: &ProjectLinks, me: &str) -> CompactLinks {
    let chips = |pairs: Vec<(&String, String)>| {
        let mut chips: Vec<LinkChip> = Vec::new();
        for (other, interface) in pairs {
            match chips.iter_mut().find(|c| &c.project_id == other) {
                Some(chip) => chip.interfaces.push(interface),
                None => chips.push(LinkChip {
                    project_id: other.clone(),
                    name: links
                        .project(other)
                        .map_or(other.clone(), |p| p.name.clone()),
                    interfaces: vec![interface],
                }),
            }
        }
        chips
    };
    let label = |kind: InterfaceKind, name: &str| format!("{} {name}", kind.label());
    CompactLinks {
        uses: chips(
            links
                .uses(me)
                .map(|l| (&l.provider, label(l.kind, &l.name)))
                .collect(),
        ),
        used_by: chips(
            links
                .used_by(me)
                .map(|l| (&l.consumer, label(l.kind, &l.name)))
                .collect(),
        ),
        unresolved: links.unmatched.iter().filter(|u| u.project == me).count()
            + links.unresolved.iter().filter(|u| u.project == me).count(),
    }
}

/// A store a project follows, as a chip on its panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreChip {
    pub name: String,
    /// The root to open Knowledge on. `None` when the store is not on this
    /// machine: the chip is muted and opens nothing.
    pub root_key: Option<String>,
    /// Why it is unavailable, for the tooltip.
    pub reason: Option<String>,
}

/// One chip per store the project at `project_path` names in its
/// `.okena/knowledge.yaml`, in the order it names them.
pub(super) fn store_chips(stores: &KnowledgeStores, project_path: &str) -> Vec<StoreChip> {
    let project = okena_core::fs::expand_home(project_path);
    stores
        .pointers
        .iter()
        .filter(|p| Path::new(&p.path) == project)
        .map(|p| {
            let root = p.root_key.as_deref().and_then(|key| stores.root(key));
            StoreChip {
                name: root.map_or(p.store_id.clone(), |r| r.name.clone()),
                root_key: p.root_key.clone(),
                reason: p.root_key.is_none().then(|| {
                    p.status
                        .first()
                        .map(|d| d.message.clone())
                        .unwrap_or_else(|| format!("{} isn't on this machine.", p.store_id))
                }),
            }
        })
        .collect()
}

/// Which harness section a file opens in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootSection {
    Knowledge,
    Specs,
}

/// Where a file opens: the root holding it, and its path inside that root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileTarget {
    pub section: RootSection,
    pub root_key: String,
    pub path: String,
}

/// The root an absolute `path` lies in, among knowledge and spec roots given
/// as `(key, directory)`. The deepest wins: a project's knowledge folder sits
/// inside the repository its spec root is.
pub(super) fn locate_file(
    path: &str,
    knowledge: &[(String, String)],
    specs: &[(String, String)],
) -> Option<FileTarget> {
    let file = Path::new(path);
    knowledge
        .iter()
        .map(|root| (RootSection::Knowledge, root))
        .chain(specs.iter().map(|root| (RootSection::Specs, root)))
        .filter_map(|(section, (key, dir))| {
            let rel = file.strip_prefix(dir).ok()?;
            let rel: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            (!rel.is_empty()).then(|| {
                (
                    Path::new(dir).components().count(),
                    FileTarget {
                        section,
                        root_key: key.clone(),
                        path: rel.join("/"),
                    },
                )
            })
        })
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, target)| target)
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{
        FileTarget, MenuGroup, MenuPick, RootSection, compact_links, entry_matches, locate_file,
        menu_entries, pr_caption, pull_requests_of, sessions_working, store_chips, task_ids,
    };
    use crate::workspace::state::ProjectData;
    use okena_core::api::{ApiGitStatus, PrInfo, PrState, RepoPullRequest};
    use okena_core::context::{ContextItem, ContextKind, ContextOwner, ContextRef};
    use okena_core::knowledge::KnowledgeStores;
    use okena_core::project_map::ProjectLinks;
    use std::collections::HashSet;

    fn project(json: serde_json::Value) -> ProjectData {
        serde_json::from_value(json).unwrap()
    }

    fn task(external_id: &str) -> serde_json::Value {
        serde_json::json!({
            "id": { "provider": "linear", "external_id": external_id },
            "display_key": "QBL-1", "title": "t", "url": "http://x",
        })
    }

    fn task_session(id: &str, external_id: &str) -> ProjectData {
        project(serde_json::json!({
            "id": id, "name": "QBL-1 (agent)", "path": "/p", "task_ref": task(external_id),
        }))
    }

    fn worktree(id: &str, external_id: &str) -> ProjectData {
        project(serde_json::json!({
            "id": id, "name": "okena (QBL-1)", "path": "/p/wt",
            "worktree_info": {
                "parent_project_id": "repo",
                "main_repo_path": "/p/okena",
                "worktree_path": "/p/wt",
                "branch_name": "feat/x",
            },
            "task_ref": task(external_id),
        }))
    }

    fn ids(v: &[&str]) -> HashSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_repo_has_the_tasks_of_its_worktrees() {
        let repo = project(serde_json::json!({ "id": "repo", "name": "okena", "path": "/p" }));
        let worktrees = [worktree("wt1", "u1"), worktree("wt2", "u2")];
        assert_eq!(task_ids(&repo, worktrees.iter()), ids(&["u1", "u2"]));
    }

    #[test]
    fn a_task_started_on_the_repo_itself_counts_too() {
        let repo = project(serde_json::json!({
            "id": "repo", "name": "okena", "path": "/p", "task_ref": task("u3"),
        }));
        assert_eq!(task_ids(&repo, std::iter::empty()), ids(&["u3"]));
    }

    #[test]
    fn a_session_is_listed_under_the_project_whose_task_it_works() {
        let projects = [task_session("s1", "u1"), task_session("s2", "u9")];
        assert_eq!(sessions_working(&projects, &ids(&["u1", "u2"])), ["s1"]);
    }

    #[test]
    fn a_worktree_on_the_task_is_not_listed_as_a_session() {
        // It carries the same task as the session working it, and is the
        // checkout that session writes to — not a second agent.
        let projects = [worktree("wt1", "u1"), task_session("s1", "u1")];
        assert_eq!(sessions_working(&projects, &ids(&["u1"])), ["s1"]);
    }

    #[test]
    fn a_spec_session_belongs_to_no_project() {
        // It works the spec repository, which is not a project here.
        let spec = project(serde_json::json!({
            "id": "s1", "name": "add-login (spec)", "path": "/specs",
            "spec_change": "add-login",
        }));
        assert!(sessions_working(&[spec], &ids(&["u1"])).is_empty());
    }

    #[test]
    fn a_project_with_no_tasks_claims_no_sessions() {
        // Nothing but the task links a session to a project — a loose match
        // would list one agent under every repo.
        assert!(sessions_working(&[task_session("s1", "u1")], &ids(&[])).is_empty());
    }

    #[test]
    fn a_session_on_several_tasks_is_listed_for_each_of_them() {
        let mut session = task_session("s1", "u1");
        session.also_tasks = vec![serde_json::from_value(task("u2")).unwrap()];
        assert_eq!(sessions_working(&[session], &ids(&["u2"])), ["s1"]);
    }

    #[test]
    fn a_repo_has_every_task_its_worktrees_cover() {
        let repo = project(serde_json::json!({ "id": "repo", "name": "okena", "path": "/p" }));
        let mut wt = worktree("wt1", "u1");
        wt.also_tasks = vec![serde_json::from_value(task("u2")).unwrap()];
        assert_eq!(task_ids(&repo, [wt].iter()), ids(&["u1", "u2"]));
    }

    fn listed(author: Option<&str>, head: &str, base: Option<&str>) -> RepoPullRequest {
        RepoPullRequest {
            pr: PrInfo {
                url: "https://github.com/o/r/pull/12".into(),
                state: PrState::Open,
                number: 12,
                base: base.map(Into::into),
                readiness: None,
                readiness_unavailable: false,
            },
            title: "Someone's change".into(),
            author: author.map(Into::into),
            head: head.into(),
            ci: None,
        }
    }

    #[test]
    fn a_pr_row_names_its_author_and_where_it_merges() {
        assert_eq!(
            pr_caption(&listed(Some("octo"), "feat/x", Some("main"))),
            "#12 · octo · feat/x → main"
        );
        assert_eq!(
            pr_caption(&listed(None, "feat/x", None)),
            "#12 · feat/x",
            "a deleted author and an unknown base are left out"
        );
        assert_eq!(pr_caption(&listed(None, "", Some("main"))), "#12 · → main");
    }

    #[test]
    fn the_panel_shows_the_list_the_snapshot_carries_and_no_section_without_one() {
        assert_eq!(pull_requests_of(None), None, "no git status yet");
        let mut git = ApiGitStatus::default();
        assert_eq!(
            pull_requests_of(Some(&git)),
            None,
            "not on github.com, or no token"
        );
        git.repo_pull_requests = Some(Vec::new());
        assert_eq!(
            pull_requests_of(Some(&git)),
            Some(Vec::new()),
            "a section that says none are open"
        );
        git.repo_pull_requests = Some(vec![listed(Some("octo"), "feat/x", Some("main"))]);
        assert_eq!(pull_requests_of(Some(&git)).map(|l| l.len()), Some(1));
    }

    fn ctx(
        kind: ContextKind,
        owner: ContextOwner,
        locator: &str,
        path: &str,
        map_id: Option<&str>,
    ) -> ContextItem {
        ContextItem {
            reference: ContextRef {
                kind,
                owner,
                locator: locator.into(),
            },
            title: locator.into(),
            description: String::new(),
            owner_name: "shop".into(),
            path: path.into(),
            map_id: map_id.map(Into::into),
            chosen: true,
        }
    }

    /// What a search for the shop project returns, ranked as the daemon ranks:
    /// kinds interleaved, and a store's and another project's items mixed in.
    fn shop_items() -> Vec<ContextItem> {
        let shop = || ContextOwner::project("shop");
        let manifest = "/r/shop/.okena/knowledge/project-map.yaml";
        vec![
            ctx(ContextKind::Skill, shop(), ".okena/knowledge/skills/release/SKILL.md", "/r/shop/.okena/knowledge/skills/release/SKILL.md", None),
            ctx(ContextKind::MapEntry, shop(), "ci:build", manifest, Some("ci:build")),
            ctx(ContextKind::Doc, ContextOwner::store("store:acme"), "docs/style.md", "/kb/docs/style.md", None),
            ctx(ContextKind::MapEntry, shop(), "area:cart", "/r/shop/.okena/knowledge/docs/cart.md", Some("area:cart")),
            ctx(ContextKind::Spec, shop(), "openspec/changes/add-gift-cards", "/r/shop/openspec/changes/add-gift-cards", None),
            ctx(ContextKind::MapEntry, ContextOwner::project("billing"), "area:ledger", "/r/billing/x", Some("area:ledger")),
            ctx(ContextKind::Spec, shop(), "openspec/specs/cart/spec.md", "/r/shop/openspec/specs/cart/spec.md", None),
            ctx(ContextKind::MapEntry, shop(), "area:checkout", manifest, Some("area:checkout")),
            ctx(ContextKind::Doc, shop(), ".okena/knowledge/docs/cart.md", "/r/shop/.okena/knowledge/docs/cart.md", None),
        ]
    }

    #[test]
    fn the_menu_groups_by_kind_in_a_fixed_order_leaving_empty_groups_out() {
        let entries = menu_entries(&shop_items(), "shop", None);
        let listed: Vec<(MenuGroup, &str)> =
            entries.iter().map(|e| (e.group, e.title.as_str())).collect();
        assert_eq!(
            listed,
            [
                // Areas in the order found, before CI though ranked after it.
                (MenuGroup::Areas, "area:cart"),
                (MenuGroup::Areas, "area:checkout"),
                (MenuGroup::Ci, "ci:build"),
                (MenuGroup::Specs, "openspec/changes/add-gift-cards"),
                (MenuGroup::Specs, "openspec/specs/cart/spec.md"),
                (MenuGroup::Knowledge, ".okena/knowledge/docs/cart.md"),
                (MenuGroup::Skills, ".okena/knowledge/skills/release/SKILL.md"),
            ]
        );
        // No concepts, interfaces, infrastructure, agents or links: no rows,
        // so no headers.
        let ids: HashSet<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids.len(), entries.len(), "row ids are unique");
    }

    #[test]
    fn the_menu_lists_only_what_this_project_owns() {
        let entries = menu_entries(&shop_items(), "shop", None);
        assert!(entries.iter().all(|e| e.title != "docs/style.md"), "a store's doc");
        assert!(entries.iter().all(|e| e.title != "area:ledger"), "another project's area");
        // An unscanned project still lists its specs and knowledge.
        let unscanned: Vec<ContextItem> = shop_items()
            .into_iter()
            .filter(|i| i.reference.kind != ContextKind::MapEntry)
            .collect();
        let groups: Vec<MenuGroup> = menu_entries(&unscanned, "shop", None)
            .iter()
            .map(|e| e.group)
            .collect();
        assert_eq!(
            groups,
            [MenuGroup::Specs, MenuGroup::Specs, MenuGroup::Knowledge, MenuGroup::Skills]
        );
    }

    #[test]
    fn picking_opens_a_doc_the_manifest_a_spec_or_a_changes_proposal() {
        let entries = menu_entries(&shop_items(), "shop", None);
        let pick = |title: &str| entries.iter().find(|e| e.title == title).unwrap().pick.clone();
        let file = |p: &str| MenuPick::File(p.to_string());
        assert_eq!(pick("area:cart"), file("/r/shop/.okena/knowledge/docs/cart.md"));
        // No doc: the index already points it at the manifest.
        assert_eq!(pick("ci:build"), file("/r/shop/.okena/knowledge/project-map.yaml"));
        assert_eq!(pick("openspec/specs/cart/spec.md"), file("/r/shop/openspec/specs/cart/spec.md"));
        assert_eq!(
            pick("openspec/changes/add-gift-cards"),
            file("/r/shop/openspec/changes/add-gift-cards/proposal.md")
        );
        let change = entries.iter().find(|e| e.title == "openspec/changes/add-gift-cards").unwrap();
        assert_eq!(change.tags, ["Change"]);
        let area = entries.iter().find(|e| e.title == "area:cart").unwrap();
        assert_eq!(area.tags, ["area:cart"]);
    }

    fn links() -> ProjectLinks {
        serde_json::from_value(serde_json::json!({
            "projects": [
                { "project_id": "shop", "name": "shop", "status": "scanned" },
                { "project_id": "billing", "name": "Billing", "status": "scanned" },
                { "project_id": "web", "name": "web", "status": "scanned" },
            ],
            "links": [
                { "consumer": "shop", "provider": "billing", "type": "http", "name": "invoices/v1", "source": "matched" },
                { "consumer": "web", "provider": "shop", "type": "http", "name": "cart/v1", "source": "confirmed", "listed_only_by": "web" },
                { "consumer": "shop", "provider": "billing", "type": "queue", "name": "invoice.paid", "source": "found_by_scan" },
                { "consumer": "web", "provider": "billing", "type": "http", "name": "invoices/v1", "source": "matched" },
            ],
            "unmatched": [
                { "project": "shop", "interface": { "type": "package", "name": "@acme/money" } },
                { "project": "web", "interface": { "type": "package", "name": "@acme/ui" } },
            ],
            "unresolved": [
                { "project": "shop", "link": { "project": "search", "direction": "uses", "type": "http", "name": "q/v1" } },
            ],
        }))
        .unwrap()
    }

    #[test]
    fn link_rows_go_under_links_and_open_the_other_project_or_nothing() {
        let entries = menu_entries(&[], "shop", Some(&links()));
        assert!(entries.iter().all(|e| e.group == MenuGroup::Links));
        let rows: Vec<(&str, &str, Vec<&str>, MenuPick)> = entries
            .iter()
            .map(|e| {
                (
                    e.title.as_str(),
                    e.description.as_str(),
                    e.tags.iter().map(String::as_str).collect(),
                    e.pick.clone(),
                )
            })
            .collect();
        let project = |id: &str| MenuPick::Project(id.to_string());
        assert_eq!(
            rows,
            [
                ("Billing", "HTTP API invoices/v1", vec!["Uses", "matched"], project("billing")),
                ("Billing", "Queue invoice.paid", vec!["Uses", "found by scan"], project("billing")),
                ("web", "HTTP API cart/v1", vec!["Used by", "confirmed", "only in web's map"], project("web")),
                ("@acme/money", "Package", vec!["No scanned project exposes"], MenuPick::Nothing),
                ("search", "HTTP API q/v1", vec!["Names no scanned project"], MenuPick::Nothing),
            ]
        );
        // Links follow every context group.
        let mut all = menu_entries(&shop_items(), "shop", Some(&links()));
        assert_eq!(all.pop().map(|e| e.group), Some(MenuGroup::Links));
        assert_eq!(all.first().map(|e| e.group), Some(MenuGroup::Areas));
    }

    #[test]
    fn typing_narrows_rows_by_every_word() {
        let entries = menu_entries(&shop_items(), "shop", Some(&links()));
        let titles = |q: &str| -> Vec<String> {
            entries
                .iter()
                .filter(|e| entry_matches(e, q))
                .map(|e| e.title.clone())
                .collect()
        };
        assert_eq!(titles("CART spec"), ["openspec/specs/cart/spec.md"]);
        assert_eq!(titles("used by"), ["web"]);
        assert_eq!(titles("").len(), entries.len());
    }

    #[test]
    fn compact_links_chip_each_other_project_once_and_count_the_rest() {
        let compact = compact_links(&links(), "shop");
        assert_eq!(compact.uses.len(), 1);
        assert_eq!(compact.uses[0].project_id, "billing");
        assert_eq!(compact.uses[0].name, "Billing");
        assert_eq!(compact.uses[0].interfaces, ["HTTP API invoices/v1", "Queue invoice.paid"]);
        assert_eq!(compact.used_by.len(), 1);
        assert_eq!(compact.used_by[0].interfaces, ["HTTP API cart/v1"]);
        // One unmatched and one unresolved of shop's own; web's is not counted.
        assert_eq!(compact.unresolved, 2);
        assert_eq!(compact_links(&ProjectLinks::default(), "shop"), Default::default());
    }

    fn stores() -> KnowledgeStores {
        serde_json::from_value(serde_json::json!({
            "roots": [
                { "key": "store:acme-eng", "kind": "store", "name": "acme-eng", "path": "/kb/acme", "healthy": true },
            ],
            "pointers": [
                { "project": "shop", "path": "/r/shop", "store_id": "acme-eng", "root_key": "store:acme-eng" },
                { "project": "billing", "path": "/r/billing", "store_id": "acme-eng", "root_key": "store:acme-eng" },
                { "project": "shop", "path": "/r/shop", "store_id": "design",
                  "status": [{ "severity": "warning", "code": "store-not-registered", "message": "design is not cloned here." }] },
                { "project": "shop", "path": "/r/shop", "store_id": "ops" },
            ],
        }))
        .unwrap()
    }

    #[test]
    fn store_chips_are_this_projects_pointers_with_a_missing_store_unavailable() {
        let chips = store_chips(&stores(), "/r/shop");
        let rows: Vec<(&str, Option<&str>, Option<&str>)> = chips
            .iter()
            .map(|c| (c.name.as_str(), c.root_key.as_deref(), c.reason.as_deref()))
            .collect();
        assert_eq!(
            rows,
            [
                ("acme-eng", Some("store:acme-eng"), None),
                ("design", None, Some("design is not cloned here.")),
                ("ops", None, Some("ops isn't on this machine.")),
            ]
        );
        assert!(store_chips(&stores(), "/r/web").is_empty(), "follows none: no chips");
    }

    #[test]
    fn a_file_opens_in_the_deepest_root_holding_it() {
        let knowledge = [
            ("path:/r/shop/.okena/knowledge".to_string(), "/r/shop/.okena/knowledge".to_string()),
            ("store:acme".to_string(), "/kb/acme".to_string()),
        ];
        let specs = [("path:/r/shop".to_string(), "/r/shop".to_string())];
        let at = |p: &str| locate_file(p, &knowledge, &specs);
        assert_eq!(
            at("/r/shop/.okena/knowledge/docs/cart.md"),
            Some(FileTarget {
                section: RootSection::Knowledge,
                root_key: "path:/r/shop/.okena/knowledge".into(),
                path: "docs/cart.md".into(),
            })
        );
        assert_eq!(
            at("/r/shop/openspec/changes/x/proposal.md"),
            Some(FileTarget {
                section: RootSection::Specs,
                root_key: "path:/r/shop".into(),
                path: "openspec/changes/x/proposal.md".into(),
            })
        );
        // A sibling whose name only starts the same is not inside.
        assert_eq!(at("/r/shopping/openspec/specs/a/spec.md"), None);
        assert_eq!(at("/r/shop"), None, "a root itself is not a file in it");
    }

    #[test]
    fn every_menu_group_has_an_icon_that_ships() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        for group in [
            MenuGroup::Areas, MenuGroup::Concepts, MenuGroup::Exposes, MenuGroup::Consumes,
            MenuGroup::Ci, MenuGroup::Infrastructure, MenuGroup::Map, MenuGroup::Specs,
            MenuGroup::Knowledge, MenuGroup::Skills, MenuGroup::Agents, MenuGroup::Links,
        ] {
            assert!(assets.join(group.icon()).is_file(), "{} is missing", group.icon());
        }
    }
}
