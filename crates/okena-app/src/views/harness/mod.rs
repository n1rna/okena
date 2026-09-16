//! Harness views, rendered full-width in the window's main content area.
//!
//! One view shows at a time, selected from the sidebar's HARNESS nav. The pane
//! holds handles to the same workspace mirror and focus manager the terminal
//! workspace uses, so a view can act on projects (focus one, start a worktree)
//! rather than only display them.

mod context_dialog;
mod doc_agents;
mod editor;
mod file_ops;
mod knowledge_draft;
mod knowledge_override;
mod knowledge_view;
mod markdown;
mod new_task_form;
mod sections;
mod specs_view;
mod store_git;
mod task_filter;
mod task_tree;
mod tasks_view;
mod testing_view;

use crate::views::components::SimpleInputState;
use crate::views::components::source_editor::BriefInput;
use crate::workspace::focus::FocusManager;
use crate::workspace::state::{WindowId, Workspace};
use gpui::*;
use okena_core::tasks::{Task, TaskAuthState};
use okena_terminal::TerminalsRegistry;
use std::cell::RefCell;
use std::rc::Rc;

pub use editor::EDITOR_CONTEXT;

/// Key context around the Tasks view. `FocusTaskSearch` is bound in it, so
/// `cmd-f` means "search tasks" there and keeps meaning terminal search in a
/// terminal pane.
pub const TASKS_CONTEXT: &str = "HarnessTasks";
pub use okena_core::harness::HarnessSection;
pub(crate) use tasks_view::{notify_task_auth_changed, provider_label};

/// Tasks-view state. Grouped so the pane struct stays readable as more views
/// grow their own state.
pub(crate) struct TasksState {
    pub(crate) provider: String,
    pub(crate) provider_display_name: String,
    pub(crate) connection: TaskAuthState,
    pub(crate) tasks: Vec<Task>,
    pub(crate) loading: bool,
    /// External id of the task whose worktree is being created. Blocks a second
    /// concurrent create, which would race on the worktree path and git's
    /// index lock.
    pub(crate) starting: Option<String>,
    /// Share of the board width given to the Todo lane, 0..1.
    pub(crate) lane_fraction: f32,
    /// Task ids whose sub-tasks are hidden. Collapsed rather than expanded
    /// state so a fresh view shows the whole breakdown by default.
    pub(crate) collapsed: std::collections::HashSet<String>,
    /// Sub-tasks fetched per task, keyed by the parent's provider id.
    ///
    /// Fetched rather than read off the loaded list because the list is the
    /// user's own queue: a task's children are often assigned to somebody
    /// else, or to nobody, and would simply be missing.
    pub(crate) children: std::collections::HashMap<String, Vec<Task>>,
    /// Task whose children are being fetched, so a slow provider does not
    /// spawn a request per frame.
    pub(crate) children_loading: Option<String>,
    /// Ancestors of the listed tasks that are not in your queue themselves —
    /// somebody else's, or closed — by provider id, up to the top of each
    /// chain. Loaded only so the list can nest a task under its parents, where
    /// they show as context rows.
    pub(crate) ancestors: std::collections::HashMap<String, Task>,
    /// Bumped on every ancestor load, so a slow walk for an older list or
    /// another provider cannot replace a newer one.
    pub(crate) ancestors_generation: u64,
    /// The task shown in the detail pane, by provider id.
    pub(crate) selected: Option<String>,
    /// A task opened from outside the list — an agent panel's task card —
    /// fetched on its own, since it is often not in the user's queue at all:
    /// a closed epic, or somebody else's story. Kept so the detail can show
    /// it and a refresh does not drop its selection.
    pub(crate) opened: Option<Task>,
    /// The selected task's description, parsed as Markdown.
    pub(crate) description: markdown::MarkdownCache,
    /// Sections folded shut in the list. Collapsed rather than expanded state,
    /// so a fresh view shows everything.
    pub(crate) sections_collapsed: std::collections::HashSet<String>,
    /// External id of the task whose breakdown agent is starting. Blocks a
    /// second start: creating a project and launching an agent takes seconds,
    /// and without this a second click during the wait made a second agent.
    pub(crate) breaking_down: Option<String>,
    /// External id of the task whose refine agent is starting, for the same
    /// reason as `breaking_down`.
    pub(crate) refining: Option<String>,
    /// The "New task" draft, if one has been started. Kept until the task is
    /// created or the form is cancelled — not when the form is hidden.
    pub(crate) new_task: Option<new_task_form::NewTaskForm>,
    /// Whether the form is showing. Apart from `new_task` so that opening a
    /// task's detail hides the draft rather than throwing it away.
    pub(crate) new_task_open: bool,
    pub(crate) new_task_title: Entity<SimpleInputState>,
    pub(crate) new_task_body: BriefInput,
    /// Open "Start work" dialog, if any.
    pub(crate) start_form: Option<StartWorkForm>,
    /// Starts waiting their turn.
    ///
    /// Creating a worktree takes git's index lock, so two at once race. A
    /// fan-out is several starts asked for at once, which makes the queue the
    /// only honest way to do it — the alternative was a guard that silently
    /// dropped every start after the first.
    pub(crate) queued_starts: Vec<QueuedStart>,
    /// How the selected task should be split among agents, when it has
    /// sub-tasks. Per-task, so switching tasks does not carry a choice made
    /// about a different breakdown.
    pub(crate) strategy: std::collections::HashMap<String, tasks_view::StartStrategy>,
    /// What the list is narrowed to. Empty means everything.
    pub(crate) filter: task_filter::TaskFilter,
    /// Whether the facet panel is open. Shut by default: the filters are a
    /// tool you reach for, and a permanent wall of chips above the list would
    /// cost every reader space to show nothing most of the time.
    pub(crate) filter_open: bool,
    /// The filter bar's search box. Its text is mirrored into `filter`, so
    /// the one `matches` test covers it.
    pub(crate) search: Entity<SimpleInputState>,
    /// Tracked by the Tasks view's root, so its key context — and with it
    /// `cmd-f` — is on the dispatch path whenever the view is in use.
    pub(crate) focus: FocusHandle,
    /// Set when the view is shown, so the next render takes focus for it.
    pub(crate) focus_on_show: bool,
    /// How the list is ordered within each section.
    pub(crate) sort: tasks_view::TaskSort,
    /// Agent command configured on the daemon, drawn as every launcher's
    /// default. `None` until settings have been read.
    pub(crate) default_agent: Option<String>,
    /// Projects the last start worked in, so the next one-click start on
    /// another task lands in the same repos.
    pub(crate) last_projects: Vec<String>,
    /// Tasks ticked to start together, by provider id.
    ///
    /// Beside `selected` rather than replacing it: clicking a row still opens
    /// one task, and ticking a box is a different question — what to start.
    pub(crate) checked: std::collections::HashSet<String>,
    /// How a set of ticked tasks is split among agents. One for the view
    /// rather than per task: the set is the thing being decided about.
    pub(crate) selection_strategy: tasks_view::SelectionStrategy,
}

/// Specs-view state.
pub(crate) struct SpecsState {
    /// Every root the daemon discovered. `None` until the first load lands.
    pub(crate) stores: Option<okena_core::specs::SpecStores>,
    /// Projects and context for the new-change form's agent, once its dialog
    /// has been opened.
    pub(crate) pickers: Option<Entity<crate::views::components::launch_pickers::LaunchPickers>>,
    /// Key of the root being shown. `None` until the first load picks the
    /// default one.
    pub(crate) root_key: Option<String>,
    /// The open root's planning tree.
    pub(crate) tree: Option<okena_core::specs::SpecTree>,
    pub(crate) loading: bool,
    /// Bumped on every load, so a slow response for a root the user has since
    /// left is dropped instead of replacing the newer one.
    pub(crate) load_generation: u64,
    pub(crate) error: Option<String>,
    /// Path of the document being read, relative to the root.
    pub(crate) selected: Option<String>,
    /// The open document's buffer, and any other with unsaved edits.
    pub(crate) documents: editor::Documents,
    pub(crate) content_error: Option<String>,
    /// The idea a new change is drafted from.
    pub(crate) idea_input: BriefInput,
    /// Whether the document panel is showing the new-change form.
    ///
    /// It stands where a document's text stands rather than taking the whole
    /// view, which hid the tree you were adding to and the specs you are meant
    /// to read before proposing.
    pub(crate) composing: bool,
    /// Directory name for the change being configured. Blank derives one from
    /// the prompt.
    pub(crate) name_input: Entity<SimpleInputState>,
    /// Root a new change is drafted into. Follows the open root until the
    /// user picks another in the form.
    pub(crate) draft_root: Option<String>,
    pub(crate) drafting: bool,
    /// Change names whose documents are hidden. Collapsed rather than expanded
    /// state, so a fresh view shows everything.
    pub(crate) collapsed: std::collections::HashSet<String>,
    /// The open store's fetch, pull, commit and push.
    pub(crate) git: store_git::StoreGitPanel,
}

impl SpecsState {
    /// Whether `path` is still listed anywhere in `tree`.
    ///
    /// Used after a refresh to drop a selection whose file has gone, so a
    /// deleted document doesn't leave stale content on screen looking current.
    pub(crate) fn contains(tree: &okena_core::specs::SpecTree, path: &str) -> bool {
        let in_change = |c: &okena_core::specs::SpecChange| {
            c.artifacts
                .iter()
                .chain(c.specs.iter())
                .any(|d| d.path == path)
        };
        tree.specs.iter().any(|d| d.path == path)
            || tree.changes.iter().any(in_change)
            || tree.archived.iter().any(in_change)
    }

    /// Stop showing the selected document. Its buffer stays only if it holds
    /// unsaved edits. Call before `root_key` changes: buffers are keyed by it.
    pub(crate) fn leave_selection(&mut self) {
        if let Some(path) = self.selected.take() {
            self.documents
                .leave(self.root_key.as_deref().unwrap_or_default(), &path);
        }
        self.content_error = None;
    }

    /// Whether the currently-loaded tree still lists `path`.
    fn tree_contains(&self, path: &str) -> bool {
        self.tree.as_ref().is_some_and(|t| Self::contains(t, path))
    }
}

/// A start that has been asked for and is waiting for the one before it.
pub(crate) struct QueuedStart {
    pub(crate) task: Task,
    pub(crate) project_ids: Vec<String>,
    /// The branch it was asked for, or `None` for the provider's own. Kept,
    /// so a start that waited is not quietly renamed.
    pub(crate) branch: Option<String>,
    pub(crate) agent_command: String,
    pub(crate) extras: StartExtras,
}

/// How a start differs from a plain one. Facts only: okena words them itself,
/// from knowledge, so the client never composes what an agent is told.
#[derive(Clone, Debug, Default)]
pub(crate) struct StartExtras {
    /// Brief the agent to split the task among sub-agents.
    pub(crate) coordinate: bool,
    /// Other sub-tasks being worked in parallel, for a fan-out.
    pub(crate) siblings: Vec<String>,
    /// Other tasks this one agent covers too, by key.
    pub(crate) also: Vec<String>,
    /// Branch names for the tasks in `also` and `siblings`, by key. Each gets
    /// worktrees of its own, so each name has to reach the daemon.
    pub(crate) branches: std::collections::BTreeMap<String, String>,
    /// The user picked these tasks together, rather than a parent having them
    /// as sub-tasks.
    pub(crate) hand_picked: bool,
    /// Map entries, specs and knowledge picked on the launcher. Every agent a
    /// fan-out or a coordinator starts from one launch is handed the same.
    pub(crate) context: Vec<okena_core::context::ContextRef>,
}

/// State of the "Start work" dialog.
///
/// Everything the run needs is decided here rather than inferred from the view,
/// so what the user sees is exactly what gets dispatched.
pub(crate) struct StartWorkForm {
    pub(crate) task: Task,
    /// What the dialog is configuring. One dialog for every agent a task can
    /// have, so starting work and breaking down offer the same things in the
    /// same places rather than being two unrelated forms.
    pub(crate) flow: LaunchFlow,
    /// Projects and context. Only the dialog picks them; a card starts in
    /// one click with none.
    pub(crate) pickers: Entity<crate::views::components::launch_pickers::LaunchPickers>,
    /// Branch name, which also determines each worktree's directory name.
    /// Only read for `LaunchFlow::Work`: a breakdown gets no worktrees.
    pub(crate) branch_input: Entity<SimpleInputState>,
    /// Which template will brief the agent, once the daemon has said.
    ///
    /// Shown instead of an editable goal: the instructions live in knowledge,
    /// and a text box here would be a second, unversioned place to keep them.
    pub(crate) brief_source: Option<String>,
    /// Every task being started, in list order, when the dialog starts several
    /// ticked together. `task` is then the first of them. Empty for one task.
    pub(crate) selection: Vec<Task>,
    /// One branch name per task in `selection`, in the same order. Each task
    /// gets worktrees of its own on its branch, so each is named here.
    pub(crate) branch_inputs: Vec<Entity<SimpleInputState>>,
}

/// The agents a task's launch dialog can configure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LaunchFlow {
    /// Doing the task, in worktrees.
    Work,
    /// Breaking the task into sub-tasks, or refining the ones it has.
    BreakDown,
    /// Rewriting the task's own title and description.
    Refine,
}

/// Smallest share either lane may be squeezed to, so a drag can never collapse
/// one entirely and strand its tasks.
pub(crate) const MIN_LANE_FRACTION: f32 = 0.15;

/// Provider id of Azure DevOps, the one provider that needs an organization
/// URL next to its token.
pub(crate) const AZURE_DEVOPS: &str = "azure_devops";

/// Where to get a credential for `provider`, and what it must be allowed to do.
pub(crate) fn provider_hint(provider: &str) -> &'static str {
    match provider {
        AZURE_DEVOPS => {
            "Azure DevOps → User settings → Personal access tokens. \
             Scope: Work Items (Read & write)."
        }
        _ => "Linear → Settings → Security & access → Personal API keys",
    }
}

pub struct HarnessPane {
    pub(crate) client: okena_transport::remote_action::RemoteActionClient,
    /// Lets a view open an overlay — the settings modal, mainly — without
    /// knowing anything about the window that owns it.
    pub(crate) request_broker: Entity<okena_workspace::request_broker::RequestBroker>,
    pub(crate) workspace: Entity<Workspace>,
    pub(crate) focus_manager: Entity<FocusManager>,
    pub(crate) window_id: WindowId,
    pub(crate) terminals: TerminalsRegistry,
    /// Shared with the window so lane dragging uses the same global mouse-move
    /// handler every other resize in the app goes through.
    pub(crate) active_drag:
        Rc<RefCell<Option<okena_views_terminal::layout::split_pane::DragState>>>,
    /// Board width from the last frame, used to turn a drag into a fraction.
    pub(crate) board_width: Rc<RefCell<f32>>,
    pub(crate) section: HarnessSection,
    pub(crate) tasks: TasksState,
    pub(crate) specs: SpecsState,
    pub(crate) knowledge: knowledge_view::KnowledgeState,
    /// The Knowledge view's "New" form.
    pub(crate) knowledge_draft: knowledge_draft::DraftForm,
    /// Overriding one of okena's read-only defaults from the open file.
    pub(crate) knowledge_override: knowledge_override::OverrideState,
    /// The agent card under an open spec document.
    pub(crate) spec_refine: doc_agents::DocRefine,
    /// The agent card under an open knowledge file.
    pub(crate) knowledge_refine: doc_agents::DocRefine,
    /// Creating, renaming and deleting files in the Specs tree.
    pub(crate) spec_files: file_ops::FileOps,
    /// Creating, renaming and deleting files in the Knowledge tree.
    pub(crate) knowledge_files: file_ops::FileOps,
    /// The Testing view's canvas. Built only for that section's pane.
    pub(crate) testing: Option<Entity<testing_view::TestingView>>,
    /// The projects-and-context dialog a Specs or Knowledge launcher opened.
    pub(crate) context_dialog: Option<context_dialog::ContextTarget>,
}

/// Everything a harness pane needs from its window.
///
/// Bundled because the window hands over the same set for every pane, and a
/// seven-parameter constructor invites silent argument transposition.
#[derive(Clone)]
pub struct PaneContext {
    pub client: okena_transport::remote_action::RemoteActionClient,
    pub request_broker: Entity<okena_workspace::request_broker::RequestBroker>,
    pub workspace: Entity<Workspace>,
    pub focus_manager: Entity<FocusManager>,
    pub window_id: WindowId,
    pub terminals: TerminalsRegistry,
    pub active_drag: Rc<RefCell<Option<okena_views_terminal::layout::split_pane::DragState>>>,
}

impl HarnessPane {
    pub fn new(section: HarnessSection, ctx: PaneContext, cx: &mut Context<Self>) -> Self {
        let provider = crate::settings::settings_entity(cx)
            .read(cx)
            .settings
            .harness
            .task_provider
            .clone();
        // Follow the setting: choosing another provider swaps the whole queue.
        cx.observe(
            &crate::settings::settings_entity(cx),
            |this: &mut Self, settings, cx| {
                let wanted = settings.read(cx).settings.harness.task_provider.clone();
                if wanted != this.tasks.provider {
                    this.switch_provider(wanted, cx);
                }
            },
        )
        .detach();
        // Connecting and disconnecting happen in Settings, so hear about them
        // from there: otherwise the empty state stays up until the pane is
        // reopened, after the user has done exactly what it asked.
        cx.observe_global::<tasks_view::TaskAuthChanged>(|this: &mut Self, cx| {
            this.refresh_auth(cx);
        })
        .detach();
        let new_task_title =
            cx.new(|cx| SimpleInputState::new(cx).placeholder("What needs doing?"));
        let new_task_body = BriefInput::new("What it covers, and what finishing it means");
        let task_search = cx.new(|cx| SimpleInputState::new(cx).placeholder("Search tasks"));
        // Rows narrow as you type: the loaded tasks are all there is to search.
        cx.subscribe(
            &task_search,
            |this: &mut Self, input, _: &okena_ui::simple_input::InputChangedEvent, cx| {
                let text = input.read(cx).value().to_string();
                this.tasks.filter.set_search(&text);
                cx.notify();
            },
        )
        .detach();
        let name_input = cx.new(|cx| SimpleInputState::new(cx).placeholder("add-login"));
        let idea_input = BriefInput::new(
            "e.g. let users sign in with Google, alongside the existing email flow",
        );
        let specs_git = store_git::StoreGitPanel::new(cx);
        let knowledge = knowledge_view::KnowledgeState::new(cx);
        // Their projects-and-context pickers are made when their dialog first
        // opens: most panes never open one.
        let knowledge_draft = knowledge_draft::DraftForm::new();
        let knowledge_override = knowledge_override::OverrideState::default();
        let spec_refine = doc_agents::DocRefine::new();
        let knowledge_refine = doc_agents::DocRefine::new();
        let spec_files = file_ops::FileOps::new(cx);
        let knowledge_files = file_ops::FileOps::new(cx);
        let testing = (section == HarnessSection::Testing).then(|| {
            let (workspace, focus_manager, window_id) = (
                ctx.workspace.clone(),
                ctx.focus_manager.clone(),
                ctx.window_id,
            );
            cx.new(|cx| testing_view::TestingView::new(workspace, focus_manager, window_id, cx))
        });
        let mut pane = Self {
            client: ctx.client,
            request_broker: ctx.request_broker,
            workspace: ctx.workspace,
            focus_manager: ctx.focus_manager,
            window_id: ctx.window_id,
            terminals: ctx.terminals,
            active_drag: ctx.active_drag,
            board_width: Rc::new(RefCell::new(0.0)),
            section,
            tasks: TasksState {
                provider_display_name: tasks_view::provider_label(&provider).to_string(),
                provider,
                connection: TaskAuthState::Unknown,
                tasks: Vec::new(),
                loading: false,
                starting: None,
                lane_fraction: 0.5,
                collapsed: std::collections::HashSet::new(),
                children: std::collections::HashMap::new(),
                children_loading: None,
                ancestors: std::collections::HashMap::new(),
                ancestors_generation: 0,
                selected: None,
                opened: None,
                description: markdown::MarkdownCache::default(),
                sections_collapsed: std::collections::HashSet::new(),
                breaking_down: None,
                refining: None,
                new_task: None,
                new_task_open: false,
                new_task_title,
                new_task_body,
                start_form: None,
                queued_starts: Vec::new(),
                strategy: std::collections::HashMap::new(),
                filter: task_filter::TaskFilter::default(),
                filter_open: false,
                search: task_search,
                focus: cx.focus_handle(),
                focus_on_show: section == HarnessSection::Tasks,
                sort: tasks_view::TaskSort::default(),
                default_agent: None,
                last_projects: Vec::new(),
                checked: std::collections::HashSet::new(),
                selection_strategy: tasks_view::SelectionStrategy::default(),
            },
            specs: SpecsState {
                stores: None,
                pickers: None,
                root_key: None,
                tree: None,
                loading: false,
                load_generation: 0,
                error: None,
                selected: None,
                documents: editor::Documents::default(),
                content_error: None,
                idea_input,
                composing: false,
                name_input,
                draft_root: None,
                drafting: false,
                collapsed: std::collections::HashSet::new(),
                git: specs_git,
            },
            knowledge,
            knowledge_draft,
            knowledge_override,
            spec_refine,
            knowledge_refine,
            spec_files,
            knowledge_files,
            testing,
            context_dialog: None,
        };
        match section {
            HarnessSection::Tasks => {
                pane.refresh_auth(cx);
                // So the dialog can default to the daemon's configured agent.
                pane.refresh_default_agent(cx);
            }
            HarnessSection::Specs => {
                pane.refresh_specs(cx);
                pane.refresh_default_agent(cx);
            }
            HarnessSection::Knowledge => {
                pane.refresh_knowledge(cx);
                // So "New" defaults to the configured agent.
                pane.refresh_default_agent(cx);
            }
            // Runs come with the daemon's state; there is nothing to fetch.
            HarnessSection::Testing => {}
        }
        pane
    }
}

impl HarnessPane {
    /// Apply a lane-divider drag. Clamped so neither lane can be collapsed.
    pub fn set_lane_fraction(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let clamped = fraction.clamp(MIN_LANE_FRACTION, 1.0 - MIN_LANE_FRACTION);
        if (self.tasks.lane_fraction - clamped).abs() > f32::EPSILON {
            self.tasks.lane_fraction = clamped;
            cx.notify();
        }
    }
}
