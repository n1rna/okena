//! Spaces: separate sets of projects, agents, tasks and roots inside one
//! profile.
//!
//! **A space is not a workspace.** okena already uses "workspace" for the one
//! set of projects and layouts saved per profile (`WorkspaceData`,
//! `workspace.json`, "Delete workspace…"). A *space* is the switch above that:
//! one profile holds several, and each has its own projects (agents included,
//! since an agent session is a project), its own task backend connection and
//! filters, and its own Library origins. Profiles separate whole config
//! directories; spaces separate what is inside one.
//!
//! There is always a Default space. It cannot be renamed or deleted, and
//! everything a profile had before spaces existed belongs to it — which is why
//! [`DEFAULT_SPACE_ID`] is also the value a project deserializes to when its
//! `space_id` is missing.
//!
//! This module holds what a space *is* and the pure rules about a list of
//! them — ordering, cycling, and which dots fit a selector. Where they are
//! stored is `okena-workspace`'s business; how they are drawn is the sidebar's.

use crate::tasks::TaskScope;
use serde::{Deserialize, Serialize};

/// The space every profile has, and the one a project falls back to.
pub const DEFAULT_SPACE_ID: &str = "default";

/// What the Default space is called. Not editable — see [`SpaceData::is_default`].
pub const DEFAULT_SPACE_NAME: &str = "Default";

/// The task connection a space falls back to when it names none: the id the
/// first Linear connection takes, which is the one every profile had before
/// spaces existed. Spelled out here rather than imported from `okena-tasks`,
/// which sits above `okena-core`.
pub const FALLBACK_CONNECTION_ID: &str = "linear";

/// [`DEFAULT_SPACE_ID`] as an owned string, for `#[serde(default = ...)]`.
pub fn default_space_id() -> String {
    DEFAULT_SPACE_ID.to_string()
}

/// One space.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "SpaceWire")]
pub struct SpaceData {
    pub id: String,
    pub name: String,
    /// The task backend connection this space reads, by connection id
    /// (`okena_tasks::Connection`). Exactly one: tasks from two connections are
    /// never merged. `None` is a space with no task backend yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    /// The hard scope on that connection. The Tasks filter bar narrows further
    /// inside it and can never reach outside.
    #[serde(default, skip_serializing_if = "TaskScope::is_empty")]
    pub tasks: TaskScope,
    /// Where this space's Library finds its origins, by type.
    #[serde(default)]
    pub library: LibraryConfig,
}

/// A space as `settings.json` may hold it: today's shape, or the one from
/// before the Library (QBL-440), when a space had `specs` and `knowledge`.
///
/// Reading is the migration. The two old keys become `library.spec` and
/// `library.knowledge` untouched — the same folders, the same stores, the same
/// saved order — and, because [`SpaceData`] writes only `library`, they are
/// gone from the file on the next save rather than lingering as a second,
/// silently ignored source of truth.
#[derive(Deserialize)]
struct SpaceWire {
    id: String,
    name: String,
    #[serde(default)]
    connection: Option<String>,
    #[serde(default)]
    tasks: TaskScope,
    #[serde(default)]
    library: Option<LibraryConfig>,
    #[serde(default)]
    specs: Option<SpecDiscoveryConfig>,
    #[serde(default)]
    knowledge: Option<KnowledgeConfig>,
}

impl From<SpaceWire> for SpaceData {
    fn from(wire: SpaceWire) -> Self {
        // A file holding both was hand-edited halfway; `library` is the one
        // this build writes, so it wins and the old keys fill only what it
        // could not have said.
        let library = match wire.library {
            Some(library) => library,
            None => LibraryConfig {
                knowledge: wire.knowledge.unwrap_or_default(),
                spec: wire.specs.unwrap_or_default(),
                freeform: FreeformConfig::default(),
            },
        };
        Self {
            id: wire.id,
            name: wire.name,
            connection: wire.connection,
            tasks: wire.tasks,
            library,
        }
    }
}

impl SpaceData {
    /// The Default space as a profile that never had spaces should read: the
    /// roots and connection the harness config already named.
    pub fn default_space(connection: Option<String>, library: LibraryConfig) -> Self {
        Self {
            id: DEFAULT_SPACE_ID.to_string(),
            name: DEFAULT_SPACE_NAME.to_string(),
            connection,
            tasks: TaskScope::default(),
            library,
        }
    }

    /// A space someone just added: no projects, no agents, no origins.
    ///
    /// "No origins" is why this is not `Default::default()` for
    /// [`LibraryConfig`]: that one lists OpenSpec's machine registry and
    /// every registered knowledge store, which would hand a brand-new space
    /// every store on the box. Projects stay on, and a new space has none, so
    /// it still starts empty — and the first project added to it brings its
    /// own origins, as it does in Default.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            connection: None,
            tasks: TaskScope::default(),
            library: LibraryConfig {
                spec: SpecDiscoveryConfig {
                    registry: false,
                    projects: true,
                    folders: Vec::new(),
                    data_dir: None,
                    config_dir: None,
                    clone_dir: None,
                },
                knowledge: KnowledgeConfig {
                    projects: true,
                    // No origins: a new space follows no store until one is
                    // added to it. Default keeps `None`, which is every store.
                    stores: Some(Vec::new()),
                    // …and so nothing to order yet.
                    order: Vec::new(),
                    clone_dir: None,
                },
                freeform: FreeformConfig::default(),
            },
        }
    }

    /// Whether this is the space that cannot be renamed or deleted.
    pub fn is_default(&self) -> bool {
        self.id == DEFAULT_SPACE_ID
    }

    /// The connection this space reads. Falls back to
    /// [`FALLBACK_CONNECTION_ID`] so a space that has never been pointed at
    /// one behaves the way the whole app did before spaces.
    pub fn connection_id(&self) -> &str {
        self.connection
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .unwrap_or(FALLBACK_CONNECTION_ID)
    }

    /// Folders to show as spec origins, in order, with blanks dropped.
    ///
    /// The legacy `harness.spec_repo` used to be folded in here; it is folded
    /// into the Default space's folders once, at migration, so by the time
    /// anything reads this there is one list.
    pub fn spec_folders(&self) -> Vec<String> {
        listed_folders(&self.library.spec.folders)
    }

    /// Folders to show as freeform origins, in order, with blanks dropped.
    pub fn freeform_folders(&self) -> Vec<String> {
        listed_folders(&self.library.freeform.folders)
    }
}

/// A folder list as it is read: trimmed, blanks dropped, order untouched —
/// the order is the order they are shown in.
fn listed_folders(folders: &[String]) -> Vec<String> {
    folders
        .iter()
        .map(|f| f.trim())
        .filter(|f| !f.is_empty())
        .map(str::to_string)
        .collect()
}

/// Mint an id for a space called `name`, avoiding the ids already `taken`.
///
/// Slugged from the name so a hand-edited `settings.json` reads, numbered when
/// two spaces share a name, and never [`DEFAULT_SPACE_ID`] — that one is
/// reserved for the space that is always there.
pub fn mint_space_id(name: &str, taken: &[String]) -> String {
    let base: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let base = if base.is_empty() || base == DEFAULT_SPACE_ID {
        "space".to_string()
    } else {
        base
    };
    let is_taken = |id: &str| id == DEFAULT_SPACE_ID || taken.iter().any(|t| t == id);
    if !is_taken(&base) {
        return base;
    }
    // One more candidate than there are ids in use always leaves a free one.
    (2..=taken.len() + 2)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !is_taken(id))
        .unwrap_or_else(|| format!("{base}-{}", taken.len() + 2))
}

/// The space after `active`, wrapping at the end.
///
/// `None` only when there are no spaces at all. An `active` the list does not
/// hold starts from the beginning rather than refusing to move — a selector
/// that does nothing is worse than one that goes somewhere sensible.
pub fn next_space<'a>(spaces: &'a [SpaceData], active: &str) -> Option<&'a SpaceData> {
    step(spaces, active, 1)
}

/// The space before `active`, wrapping at the start.
pub fn previous_space<'a>(spaces: &'a [SpaceData], active: &str) -> Option<&'a SpaceData> {
    step(spaces, active, -1)
}

fn step<'a>(spaces: &'a [SpaceData], active: &str, by: isize) -> Option<&'a SpaceData> {
    if spaces.is_empty() {
        return None;
    }
    let len = spaces.len() as isize;
    let at = spaces.iter().position(|s| s.id == active).unwrap_or(0) as isize;
    let next = (at + by).rem_euclid(len) as usize;
    spaces.get(next)
}

/// The `n`th space, counting from 1 — what Cmd/Ctrl+1..9 jumps to.
///
/// `None` past the end, so Cmd+5 with four spaces does nothing rather than
/// landing somewhere arbitrary.
pub fn nth_space(spaces: &[SpaceData], n: usize) -> Option<&SpaceData> {
    n.checked_sub(1).and_then(|i| spaces.get(i))
}

/// The space `wanted` names: its id, or else its name, ignoring case and the
/// whitespace around it.
///
/// An id wins over a name, so a space named like another space's id cannot
/// hide it. What a person types into the CLI or an agent passes over MCP is a
/// name far more often than an id.
pub fn find_space<'a>(spaces: &'a [SpaceData], wanted: &str) -> Option<&'a SpaceData> {
    let wanted = wanted.trim();
    spaces.iter().find(|s| s.id == wanted).or_else(|| {
        let wanted = wanted.to_lowercase();
        spaces.iter().find(|s| s.name.to_lowercase() == wanted)
    })
}

/// How the selector splits its spaces when they do not all fit.
///
/// `shown` are drawn as dots, in order; `overflow` go behind the **+N** chip,
/// also in order. The active space is always in `shown`, even when its
/// position would have put it in the menu — you must be able to see where you
/// are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectorFit {
    pub shown: Vec<usize>,
    pub overflow: Vec<usize>,
}

/// Decide which dots are drawn given how many fit.
///
/// `capacity` is how many dots the sidebar's width allows, already counting
/// the **+** button. When everything fits there is no chip; when it does not,
/// one of the slots goes to the chip itself, so `capacity - 1` dots are drawn.
pub fn fit_selector(count: usize, active: usize, capacity: usize) -> SelectorFit {
    if count == 0 {
        return SelectorFit {
            shown: Vec::new(),
            overflow: Vec::new(),
        };
    }
    if count <= capacity {
        return SelectorFit {
            shown: (0..count).collect(),
            overflow: Vec::new(),
        };
    }
    // The chip costs a slot. At least one dot is always drawn — a selector
    // showing nothing but "+3" says nothing about where you are.
    let room = capacity.saturating_sub(1).max(1);
    let mut shown: Vec<usize> = (0..room.min(count)).collect();
    if !shown.contains(&active) && active < count {
        // Drop the last one drawn to make room, so the active space is visible
        // without reordering the rest.
        shown.pop();
        shown.push(active);
        shown.sort_unstable();
    }
    let overflow = (0..count).filter(|i| !shown.contains(i)).collect();
    SelectorFit { shown, overflow }
}

// The origin configurations below are per *space*. They moved here from the
// harness settings when spaces arrived: a profile no longer has one set of
// roots, each space does. `okena-workspace` re-exports them, so
// `settings::SpecDiscoveryConfig` still names this type.

/// Where a space's Library finds its origins: one block per origin type
/// (`okena_core::library::OriginType`), under one key.
///
/// The blocks stay separate because the three types are found differently —
/// knowledge stores through okena's registry, spec roots through OpenSpec's
/// own, freeform origins by the folders listed here — and because the first
/// two are exactly what `spaces[].knowledge` and `spaces[].specs` held before
/// the Library, so a settings file from then carries over without a value
/// changing meaning.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryConfig {
    /// `knowledge` origins, and the order they layer in.
    #[serde(default)]
    pub knowledge: KnowledgeConfig,
    /// `spec` origins: OpenSpec roots.
    #[serde(default)]
    pub spec: SpecDiscoveryConfig,
    /// `freeform` origins: folders of markdown.
    #[serde(default, skip_serializing_if = "FreeformConfig::is_unset")]
    pub freeform: FreeformConfig,
}

/// Where a freeform clone goes when no destination is given, before `~`
/// expansion.
pub const DEFAULT_FREEFORM_CLONE_DIR: &str = "~/library";

/// Freeform origins, per space: any folder of markdown.
///
/// The folders are the whole of it. A freeform origin has no identity file
/// and no registry — that is what makes it freeform — so the list of paths is
/// the only record of it, the way `spec.folders` is for an OpenSpec folder.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreeformConfig {
    /// The folders to show, in the order they are shown in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<String>,

    /// Folder a repository is cloned into when no destination is given. Unset
    /// is `~/library`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clone_dir: Option<String>,
}

impl FreeformConfig {
    /// Nothing set, so nothing worth writing to the file.
    pub fn is_unset(&self) -> bool {
        self.folders.is_empty() && self.clone_dir.is_none()
    }

    /// The clone folder with `~` expanded; blank counts as unset.
    pub fn clone_dir(&self) -> std::path::PathBuf {
        let dir = self
            .clone_dir
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .unwrap_or(DEFAULT_FREEFORM_CLONE_DIR);
        crate::fs::expand_home(dir)
    }

    /// Add `folder` to the list unless it is already there, compared after
    /// `~` expansion so `~/notes` and its absolute path are one folder.
    /// Returns whether it was added.
    pub fn add_folder(&mut self, folder: &str) -> bool {
        let folder = folder.trim();
        if folder.is_empty() || self.holds(folder) {
            return false;
        }
        self.folders.push(folder.to_string());
        true
    }

    /// Drop `folder` from the list, compared as [`Self::add_folder`]
    /// compares. Returns whether anything was dropped.
    pub fn remove_folder(&mut self, folder: &str) -> bool {
        let wanted = crate::fs::expand_home(folder.trim());
        let before = self.folders.len();
        self.folders
            .retain(|f| crate::fs::expand_home(f.trim()) != wanted);
        self.folders.len() != before
    }

    fn holds(&self, folder: &str) -> bool {
        let wanted = crate::fs::expand_home(folder);
        self.folders
            .iter()
            .any(|f| crate::fs::expand_home(f.trim()) == wanted)
    }
}

/// Where a space's Library finds OpenSpec roots.
///
/// Discovery follows OpenSpec's own model
/// (<https://openspec.dev/docs/stores>): stores registered on this machine,
/// repositories that carry their own `openspec/` tree or point at a store with
/// `store:`, plus any folders listed here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecDiscoveryConfig {
    /// List the stores in OpenSpec's machine registry — what
    /// `openspec store list` shows.
    #[serde(default = "default_true")]
    pub registry: bool,

    /// Treat okena projects as OpenSpec roots when their repository holds an
    /// `openspec/` tree, and follow their `store:` pointers. Only the active
    /// space's projects, so a root never leaks between spaces.
    #[serde(default = "default_true")]
    pub projects: bool,

    /// Extra folders to show that are neither registered stores nor projects.
    /// The order is the order they are shown in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<String>,

    /// OpenSpec's data directory, where `stores/registry.yaml` lives.
    ///
    /// Unset resolves it the way the CLI does (`$XDG_DATA_HOME/openspec`, else
    /// `~/.local/share/openspec`, `%LOCALAPPDATA%\openspec` on Windows). Needed
    /// when the CLI runs with an `XDG_DATA_HOME` the daemon never saw — an app
    /// launched from the dock does not inherit a shell profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,

    /// OpenSpec's config directory, where `config.json` (and `defaultStore`)
    /// lives. Same resolution and reason as `data_dir`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_dir: Option<String>,

    /// Folder a store is cloned into when no destination is given. Unset is
    /// `~/openspec`, the convention the CLI uses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clone_dir: Option<String>,
}

/// Where an OpenSpec clone goes when no destination is given, before `~`
/// expansion.
pub const DEFAULT_SPEC_CLONE_DIR: &str = "~/openspec";

impl SpecDiscoveryConfig {
    /// The clone folder with `~` expanded; blank counts as unset.
    pub fn clone_dir(&self) -> std::path::PathBuf {
        let dir = self
            .clone_dir
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .unwrap_or(DEFAULT_SPEC_CLONE_DIR);
        crate::fs::expand_home(dir)
    }
}

impl Default for SpecDiscoveryConfig {
    fn default() -> Self {
        Self {
            // Must match the serde defaults above.
            registry: true,
            projects: true,
            folders: Vec::new(),
            data_dir: None,
            config_dir: None,
            clone_dir: None,
        }
    }
}

/// Where a clone goes when no destination is given, before `~` expansion.
pub const DEFAULT_KNOWLEDGE_CLONE_DIR: &str = "~/knowledge";

/// Knowledge origins (ADR-0003), per space.
///
/// The stores themselves are not listed here: checkout paths are machine
/// state, kept in okena's per-profile registry (`knowledge/stores.yaml`), so a
/// synced `settings.json` never carries paths that don't exist elsewhere.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeConfig {
    /// Find knowledge in the space's projects: the stores a repo follows in
    /// `.okena/knowledge.yaml`, and its own `.okena/knowledge/` folders.
    #[serde(default = "default_true")]
    pub projects: bool,

    /// Which registered knowledge stores this space follows, by id, in the
    /// order they are shown.
    ///
    /// `None` is every store okena has registered, which is what a profile had
    /// before spaces and what Default keeps. `Some` is exactly those, in that
    /// order — so `Some(vec![])` is a space that follows none, which is how a
    /// new space starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stores: Option<Vec<String>>,

    /// The order this space's knowledge roots layer in, as root keys, top
    /// first (QBL-425). Empty until somebody arranges them, which leaves
    /// discovery order.
    ///
    /// Keys, not paths: a key is `store:<id>` or `path:<absolute path>`, so
    /// this stays a preference while the checkout paths stay machine state in
    /// the registry (ADR-0003). `okena-defaults` is never in it — it is always
    /// last. The rules, and the pruning of roots that have gone, are
    /// `okena_knowledge::order`.
    ///
    /// Per space, like the rest of this: two spaces may follow the same store
    /// and want it layered differently.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,

    /// Folder a store is cloned into when no destination is given. Unset is
    /// `~/knowledge`, beside OpenSpec's `~/openspec` convention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clone_dir: Option<String>,
    //
    // There is deliberately no setting naming one root as the source of launch
    // briefs (QBL-415). Templates, partials and skills resolve across every
    // healthy root in discovery order, so overriding one is a matter of putting
    // the file somewhere, not of pointing a setting at it. A `prompts` key left
    // in an older settings.json is ignored on load and gone on the next save.
}

impl Default for KnowledgeConfig {
    fn default() -> Self {
        Self {
            // Must match the serde defaults above.
            projects: true,
            stores: None,
            order: Vec::new(),
            clone_dir: None,
        }
    }
}

impl KnowledgeConfig {
    /// The clone folder with `~` expanded; blank counts as unset.
    pub fn clone_dir(&self) -> std::path::PathBuf {
        let dir = self
            .clone_dir
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .unwrap_or(DEFAULT_KNOWLEDGE_CLONE_DIR);
        crate::fs::expand_home(dir)
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spaces(ids: &[&str]) -> Vec<SpaceData> {
        ids.iter().map(|id| SpaceData::new(*id, *id)).collect()
    }

    #[test]
    fn a_space_is_found_by_its_id_or_its_name() {
        let all = vec![
            SpaceData::new("default", "Default"),
            SpaceData::new("client-a", "Client A"),
        ];
        let found = |wanted: &str| find_space(&all, wanted).map(|s| s.id.as_str());
        assert_eq!(found("client-a"), Some("client-a"));
        assert_eq!(found("Client A"), Some("client-a"));
        assert_eq!(found("  client a "), Some("client-a"));
        assert_eq!(found("Client B"), None);
        assert_eq!(found(""), None);
    }

    #[test]
    fn an_id_wins_over_another_spaces_name() {
        let all = vec![
            SpaceData::new("work", "Personal"),
            SpaceData::new("personal", "work"),
        ];
        assert_eq!(find_space(&all, "work").map(|s| s.id.as_str()), Some("work"));
    }

    #[test]
    fn a_new_space_starts_with_no_roots_and_no_connection() {
        let s = SpaceData::new("client-a", "Client A");
        assert!(s.connection.is_none());
        assert!(s.tasks.is_empty());
        assert!(s.library.spec.folders.is_empty());
        assert!(s.library.freeform.folders.is_empty());
        // The machine registry belongs to Default, not to every new space.
        assert!(!s.library.spec.registry);
        assert!(!s.is_default());
    }

    #[test]
    fn the_default_space_keeps_what_the_harness_config_had() {
        let library = LibraryConfig {
            spec: SpecDiscoveryConfig {
                folders: vec!["~/specs".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let d = SpaceData::default_space(Some("linear".into()), library);
        assert!(d.is_default());
        assert_eq!(d.name, DEFAULT_SPACE_NAME);
        assert_eq!(d.library.spec.folders, ["~/specs"]);
        assert_eq!(d.connection.as_deref(), Some("linear"));
    }

    #[test]
    fn a_space_id_is_slugged_from_its_name() {
        assert_eq!(mint_space_id("Client A", &[]), "client-a");
        assert_eq!(mint_space_id("  Ácme //  Corp ", &[]), "cme-corp");
    }

    #[test]
    fn two_spaces_with_one_name_get_distinct_ids() {
        let taken = vec!["client-a".to_string()];
        assert_eq!(mint_space_id("Client A", &taken), "client-a-2");
    }

    #[test]
    fn a_space_can_never_mint_the_default_id() {
        // Otherwise a space called "Default" would become undeletable.
        assert_eq!(mint_space_id("Default", &[]), "space");
        assert_eq!(mint_space_id("!!!", &[]), "space");
    }

    #[test]
    fn next_and_previous_wrap_around() {
        let list = spaces(&["default", "a", "b"]);
        assert_eq!(next_space(&list, "default").map(|s| s.id.as_str()), Some("a"));
        assert_eq!(next_space(&list, "b").map(|s| s.id.as_str()), Some("default"));
        assert_eq!(
            previous_space(&list, "default").map(|s| s.id.as_str()),
            Some("b")
        );
        assert_eq!(previous_space(&list, "a").map(|s| s.id.as_str()), Some("default"));
    }

    #[test]
    fn stepping_from_an_unknown_active_space_still_moves() {
        let list = spaces(&["default", "a"]);
        assert_eq!(next_space(&list, "gone").map(|s| s.id.as_str()), Some("a"));
        assert!(next_space(&[], "default").is_none());
    }

    #[test]
    fn a_single_space_steps_to_itself() {
        let list = spaces(&["default"]);
        assert_eq!(
            next_space(&list, "default").map(|s| s.id.as_str()),
            Some("default")
        );
    }

    #[test]
    fn nth_counts_from_one_and_stops_at_the_end() {
        let list = spaces(&["default", "a", "b"]);
        assert_eq!(nth_space(&list, 1).map(|s| s.id.as_str()), Some("default"));
        assert_eq!(nth_space(&list, 3).map(|s| s.id.as_str()), Some("b"));
        assert!(nth_space(&list, 4).is_none());
        assert!(nth_space(&list, 0).is_none());
    }

    #[test]
    fn every_dot_is_drawn_when_they_all_fit() {
        let fit = fit_selector(3, 0, 5);
        assert_eq!(fit.shown, [0, 1, 2]);
        assert!(fit.overflow.is_empty());
    }

    #[test]
    fn exactly_filling_the_width_needs_no_chip() {
        let fit = fit_selector(4, 0, 4);
        assert_eq!(fit.shown, [0, 1, 2, 3]);
        assert!(fit.overflow.is_empty());
    }

    #[test]
    fn the_ones_that_do_not_fit_go_behind_the_chip() {
        // Six spaces, room for four: the chip takes one slot, so three dots.
        let fit = fit_selector(6, 0, 4);
        assert_eq!(fit.shown, [0, 1, 2]);
        assert_eq!(fit.overflow, [3, 4, 5]);
    }

    #[test]
    fn the_active_space_is_shown_even_when_it_would_overflow() {
        let fit = fit_selector(6, 5, 4);
        assert!(fit.shown.contains(&5), "{fit:?}");
        assert_eq!(fit.shown, [0, 1, 5]);
        assert_eq!(fit.overflow, [2, 3, 4]);
    }

    #[test]
    fn a_selector_with_no_room_still_shows_where_you_are() {
        let fit = fit_selector(4, 2, 1);
        assert_eq!(fit.shown, [2]);
        assert_eq!(fit.overflow, [0, 1, 3]);
    }

    #[test]
    fn no_spaces_fit_into_nothing() {
        assert_eq!(fit_selector(0, 0, 3).shown, Vec::<usize>::new());
    }

    #[test]
    fn a_profile_with_only_default_still_draws_its_dot() {
        // The selector row holds the `+` that adds a space, so a profile with
        // one space must still draw it — otherwise the only way out of having
        // one space is hidden behind having more than one.
        let fit = fit_selector(1, 0, 9);
        assert_eq!(fit.shown, [0]);
        assert!(fit.overflow.is_empty());
    }

    #[test]
    fn a_spaces_spec_folders_keep_the_order_they_were_given() {
        let mut space = SpaceData::new("client-a", "Client A");
        space.library.spec.folders = vec![
            "  ~/b  ".into(),
            "".into(),
            "~/a".into(),
            "   ".into(),
            "~/c".into(),
        ];
        // Trimmed, blanks dropped, order untouched — the order is the order
        // they are shown in.
        assert_eq!(space.spec_folders(), ["~/b", "~/a", "~/c"]);
    }

    #[test]
    fn a_new_space_follows_no_knowledge_store_while_default_follows_them_all() {
        let fresh = SpaceData::new("client-a", "Client A");
        assert_eq!(fresh.library.knowledge.stores.as_deref(), Some(&[][..]));
        let default = SpaceData::default_space(None, Default::default());
        assert!(
            default.library.knowledge.stores.is_none(),
            "Default keeps every registered store, as a profile had before spaces"
        );
    }

    #[test]
    fn a_space_saved_before_the_library_keeps_its_roots_order_and_settings() {
        // What `settings.json` held per space before QBL-440.
        let old = r#"{
            "id": "client-a", "name": "Client A", "connection": "linear-2",
            "specs": { "registry": false, "projects": true,
                       "folders": ["~/acme/specs"], "data_dir": "~/.openspec-data",
                       "clone_dir": "~/acme/openspec" },
            "knowledge": { "projects": false, "stores": ["acme-eng", "acme-ops"],
                           "order": ["store:acme-ops", "path:/repo/.okena/knowledge"],
                           "clone_dir": "~/acme/knowledge" }
        }"#;
        let space: SpaceData = serde_json::from_str(old).expect("an older space still loads");
        assert_eq!(space.connection.as_deref(), Some("linear-2"));

        let spec = &space.library.spec;
        assert!(!spec.registry && spec.projects);
        assert_eq!(spec.folders, ["~/acme/specs"]);
        assert_eq!(spec.data_dir.as_deref(), Some("~/.openspec-data"));
        assert_eq!(spec.clone_dir.as_deref(), Some("~/acme/openspec"));

        let knowledge = &space.library.knowledge;
        assert!(!knowledge.projects);
        assert_eq!(
            knowledge.stores.as_deref(),
            Some(&["acme-eng".to_string(), "acme-ops".to_string()][..])
        );
        assert_eq!(
            knowledge.order,
            ["store:acme-ops", "path:/repo/.okena/knowledge"],
            "the saved order is carried over as written"
        );
        assert_eq!(knowledge.clone_dir.as_deref(), Some("~/acme/knowledge"));
        assert!(space.library.freeform.is_unset());

        // Saved again, the space holds one `library` key and neither old one.
        let saved = serde_json::to_value(&space).expect("serialize");
        assert!(saved.get("specs").is_none() && saved.get("knowledge").is_none(), "got {saved}");
        assert_eq!(saved["library"]["spec"]["folders"][0], "~/acme/specs");
        assert_eq!(saved["library"]["knowledge"]["order"][0], "store:acme-ops");
        let back: SpaceData = serde_json::from_value(saved).expect("reload");
        assert_eq!(back, space);
    }

    #[test]
    fn a_space_with_only_one_of_the_old_keys_defaults_the_other() {
        let space: SpaceData = serde_json::from_str(
            r#"{ "id": "default", "name": "Default", "specs": { "folders": ["~/s"] } }"#,
        )
        .expect("loads");
        assert_eq!(space.library.spec.folders, ["~/s"]);
        assert_eq!(space.library.knowledge, KnowledgeConfig::default());
    }

    #[test]
    fn the_library_key_wins_over_leftover_old_keys() {
        let space: SpaceData = serde_json::from_str(
            r#"{ "id": "default", "name": "Default",
                 "library": { "spec": { "folders": ["~/new"] } },
                 "specs": { "folders": ["~/old"] } }"#,
        )
        .expect("loads");
        assert_eq!(space.library.spec.folders, ["~/new"]);
    }

    #[test]
    fn freeform_folders_are_added_once_and_removed_however_they_are_spelled() {
        let mut freeform = FreeformConfig::default();
        assert!(freeform.add_folder(" ~/notes "));
        assert!(!freeform.add_folder("~/notes"), "already listed");
        let absolute = crate::fs::expand_home("~/notes");
        assert!(
            !freeform.add_folder(&absolute.to_string_lossy()),
            "the same folder by its absolute path"
        );
        assert!(!freeform.add_folder("   "));
        assert!(freeform.add_folder("~/decisions"));
        assert_eq!(freeform.folders, ["~/notes", "~/decisions"]);

        assert!(freeform.remove_folder(&absolute.to_string_lossy()));
        assert!(!freeform.remove_folder("~/notes"), "already gone");
        assert_eq!(freeform.folders, ["~/decisions"]);
    }

    #[test]
    fn a_space_with_no_freeform_origins_writes_no_freeform_block() {
        let mut space = SpaceData::default_space(None, LibraryConfig::default());
        let json = serde_json::to_string(&space).expect("serialize");
        assert!(!json.contains("freeform"), "got {json}");
        space.library.freeform.add_folder("~/notes");
        let json = serde_json::to_value(&space).expect("serialize");
        assert_eq!(json["library"]["freeform"]["folders"][0], "~/notes");
        assert_eq!(space.freeform_folders(), ["~/notes"]);
    }

    #[test]
    fn a_space_round_trips_and_omits_what_it_does_not_set() {
        let s = SpaceData::new("client-a", "Client A");
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(!json.contains("connection"), "got {json}");
        assert!(!json.contains("\"tasks\""), "got {json}");
        let back: SpaceData = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, s);
    }
}
