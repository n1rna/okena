# Knowledge stores

A **knowledge store** is a git repository holding an organisation's engineering
knowledge: how CI works, how the repositories are laid out, where service
boundaries are, how reviews and releases are done. It also holds the skills,
subagents and prompt templates the organisation shares. okena reads stores,
clones them, and keeps them up to date. It lists what is in them in the harness
Knowledge section.

The format belongs to okena ([ADR-0003](../decisions/0003-knowledge-stores.md)).
Skills and agents use the existing Claude formats, so the repository is useful
to an agent that has never heard of okena.

## Layout

```text
<store>/
├── .okena-knowledge/store.yaml   identity (see below)
├── docs/**/*.md                  principles, processes, architecture, runbooks
├── skills/**/SKILL.md            one skill per directory holding a SKILL.md
├── agents/**/*.md                one subagent per file
└── templates/
    ├── briefs/<flow>.md          the brief for one launch flow
    ├── partials/<name>.md        text shared between briefs
    └── **/*.md                   any other prompt template
```

Every folder is optional. A root needs at least one kind folder or an identity.
Anything outside the four kind folders (a `README.md`, CI config) is ignored.

`templates/` has two folders okena reads by name: `briefs/`, one file per
[launch flow](#flows), and `partials/`, the [shared text](#partials) they
include. Both are listed as directories of their own in the Knowledge view.
Anything else under `templates/` is a template the store keeps for itself.

| Kind | What counts as an entry | Name |
|---|---|---|
| Doc | every `.md` file under `docs/`, at any depth | path under `docs/` without `.md`, e.g. `ci/pipeline` |
| Skill | every directory below `skills/` that holds a `SKILL.md`; the other files in it are the skill's supporting files | frontmatter `name`, else the directory path, e.g. `release` |
| Agent | every `.md` file under `agents/` | frontmatter `name`, else the path without `.md` |
| Template | every `.md` file under `templates/` | path under `templates/` without `.md` |

A `SKILL.md` directly in `skills/` is not a skill. A skill's subdirectories
belong to that skill; okena does not look for more skills inside them.

Rules for walking the folders:

- Names starting with `.` are skipped.
- Symlinked directories are never followed.
- A kind folder that is itself a symlink is ignored.
- A symlinked file counts only when it resolves inside the root.

An entry is addressed by its **store id plus its path relative to the store
root**, e.g. `acme-eng` + `docs/ci/pipeline.md`. That address is the same on
every machine, whatever the checkout folder is called.

## Frontmatter

Frontmatter is optional YAML between a `---` on the first line and a closing
`---` or `...`.

| Field | Kinds | Meaning |
|---|---|---|
| `title` | all | Display title |
| `description` | all | One line saying what the entry is for; what a person or an agent picks an entry by |
| `tags` | all | A list, or one comma-separated string |
| `name` | skill, agent | The entry's name, as the Agent Skills and Claude subagent formats define it |
| `for` | template | The launch flows the template applies to: a list, or one comma-separated string |

When there is no `title`, okena uses the first `# ` heading outside a code
fence, then the entry's name.

A template lists the `{placeholder}` names its body uses. A placeholder is a
single identifier in braces (`{key}`, `{change_dir}`), so JSON such as
`{"a": 1}` does not count. The flows are listed under "Launch prompts" below.

Frontmatter okena cannot parse does not hide the entry. The entry is still
listed, carrying a `frontmatter_invalid` warning. Fields other than those above
are ignored when listing, and are shown as they are when the entry is opened.

## Store identity

`.okena-knowledge/store.yaml`:

```yaml
version: 1
id: acme-eng                 # kebab-case: lowercase letters and digits, single hyphens
name: Acme Engineering       # optional; the id when unset
description: How we build    # optional
remote: git@github.com:acme/eng-knowledge.git   # optional canonical clone source
```

Unknown keys are ignored. A `version` newer than okena understands is refused.
Commit this file: it is what makes every clone agree on the store's id.

## Registry

okena lists the stores on a machine in
`<profile config dir>/knowledge/stores.yaml`. On macOS the profile config dir is
under `~/Library/Application Support/okena/`.

```yaml
version: 1
stores:
  acme-eng:
    path: /Users/me/knowledge/eng-knowledge
    remote: git@github.com:acme/eng-knowledge.git   # origin when registered
```

The rules for this file:

- **Writer:** only the daemon writes it.
- **One-to-one:** a store id has one checkout, and a checkout has one id.
- **Not settings:** checkout paths are machine state, so they are kept out of
  `settings.json`.
- **Corrupt file:** it is reported, never overwritten.

There are three ways to add a store:

- **Clone:** clones a URL and registers the checkout.
  - With no destination, it clones into `harness.knowledge.clone_dir`
    (default `~/knowledge`), in the folder `git clone` would name.
  - If the clone fails, a folder okena created is removed.
  - If the repository turns out not to be a knowledge root, the checkout stays
    on disk and the error says where it is.
- **Existing folder:** registers a checkout already on disk. Its id comes from
  `store.yaml`. Without one, the id comes from the folder name, and the store
  carries a `store_identity_missing` warning until an identity is committed.
- **Create:** writes the identity and the four kind folders, each with a
  `.gitkeep`, and registers the store.
  - With git, it runs `git init` when needed and makes one commit,
    `Initialize knowledge store <id>`, of exactly those paths.
  - It refuses a file, a non-empty folder (a folder holding only `.git` is
    allowed), a folder inside another git repository, a taken id, and a
    missing git author identity.
  - If a step fails before the commit, everything it created is removed.

Unregistering removes only the registry entry. The checkout stays on disk.

## Projects

A project repository joins through `.okena/knowledge.yaml` at its root:

```yaml
stores: [acme-eng]           # stores this repository follows
root: docs/knowledge         # this repository's own kind folders (default .okena/knowledge)
```

Both keys and the file are optional.

- **Followed stores:** each id in `stores:` puts the project in that store's
  "used by" list. An id not registered on this machine is reported as
  `unknown_store`.
- **Project root:** the directory at `root:` becomes a project knowledge root
  when it exists. It holds the same kind folders as a store. `root: .` makes the
  repository itself the root.
- **Default root:** a repository with a `.okena/knowledge/` folder has a
  project root without any config file.
- **Stays inside:** `root:` must stay inside the repository, checked both as
  written and after symlinks resolve. Otherwise it is reported as
  `project_root_outside`.
- **Duplicate path:** a project root at the same path as a registered store is
  listed once, as the store.
- **Project map:** a project root can hold the repository's map,
  `project-map.yaml` and `docs/project/`, described in
  [project-map.md](project-map.md).

Worktrees and agent sessions are never searched. Project discovery can be
turned off with `harness.knowledge.projects`.

## Sync

Sync state is read from local git and never touches the network. It is
therefore as fresh as the last fetch:

- the checked-out branch and its upstream
- commits ahead and behind
- the files with uncommitted changes — staged, unstaged and untracked, listed
  file by file, up to 1 000
- when the checkout last fetched (the time of `FETCH_HEAD`)

Only a folder with its own `.git` is treated as a checkout. A store folder
nested inside another repository has no sync state.

- **Fetch** runs `git fetch --all`.
- **Pull** fetches, then runs `git merge --ff-only @{upstream}`. It refuses:
  - a detached HEAD (`detached_head`)
  - a branch without an upstream (`no_upstream`)
  - a branch that is both ahead and behind (`diverged`)
  - a checkout with uncommitted changes (`uncommitted_changes`)

  The error names the checkout to fix it in.
- **Commit** takes the listed files and a message. Every path must be one the
  sync state lists as changed; anything else is refused (`not_a_changed_file`),
  and so is a file with conflicts (`conflicted_file`). Other staged changes stay
  out of the commit and stay staged. Nothing is pushed.
- **Push** sends the branch to its upstream's remote and branch. It refuses a
  branch the upstream is ahead of (`behind_upstream`). A rejected push
  (`push_failed`) keeps the commit.

Git runs non-interactively. Credentials must come from an SSH agent or a
credential helper, and a prompt fails instead of hanging. Only stores sync. A
project root is synced with its project's own git.

okena never commits on its own. The only commit it makes unasked is the initial
one when creating a store; everything else is a commit you asked for from the
store overview. Files are edited in the Knowledge view, in a terminal, or by an
agent working in the checkout. The shared store git behind all of this, also
used by OpenSpec stores, is described in
[ADR-0004](../decisions/0004-store-commit-and-push.md).

## In okena

- **Harness → Knowledge** has these parts:
  - **Root list:** every root, in the order it layers in, with its health and a
    sync badge (`↑` commits to push, `↓` commits to pull, `•` uncommitted
    changes). `+` beside the ROOTS heading opens the **Roots page**.
  - **Entry list:** the open root's entries grouped by kind, with docs and
    templates nested by folder — so `briefs` and `partials` are directories
    holding one row per file — and a filter over titles, names, paths,
    descriptions and tags.
    Markdown entries render formatted, and a skill lists its supporting files.
    A template one of your roots holds a copy of is marked `override` when
    that copy is what a launch reads, and `default` when a copy exists but
    okena's own is still what is sent — an empty file is a placeholder, not an
    answer. A template only okena has is not marked.
  - **Copies:** an opened template, partial or skill lists every root holding
    a copy of it, in [layering order](#resolution) and ending in
    `okena-defaults`, with the copy a launch actually reads highlighted and
    labelled **applied**. Each one opens that root's copy. The list follows
    the roots: reorder them and the highlight moves, delete the winning copy
    and it moves to the next. A doc or an agent has no list — nothing
    overrides them.
  - **Editing:** an opened file can be edited and saved with `cmd-s`
    (`ctrl-s`). A Markdown file toggles between **Edit** (the source) and
    **Preview**; any other file opens straight in the editor. A file with
    unsaved edits is marked `●` in the list and keeps its edits while another
    file is open. **Revert** drops them and reloads the file. The Specs view
    edits spec documents the same way.
  - **Files:** `+` beside the filter creates an entry. Pick the kind and a
    name (`ci/pipeline` makes folders), and it opens in the editor, starting
    from the kind's frontmatter. The open file's **Rename** moves it, and the
    open document follows with any unsaved edits. **Delete…** asks first, then
    removes the file and closes it. Paths are checked the way reads are:
    nothing outside the root, no hidden names, and never over an existing
    file. A folder emptied this way stops being listed.
  - **Store overview:** the branch, the last fetch, and **Fetch**, **Pull** and
    **Push** buttons. Pull and Push are offered only when they can succeed, and
    a line says why not while there is something to pull or push. Below them
    are the uncommitted files and a commit box. Leaving the message blank
    commits with a default that names the file, or the number of files. The
    Specs view shows the same panel for store and folder roots.
  - **Unresolved stores:** projects that follow a store not on this machine are
    listed under "Followed, not here".
- **New with agent** opens an agent session in a root, briefed on this layout
  and on the frontmatter entries are picked by.
  - **In a store:** the agent is told to work on a `knowledge/<topic>` branch,
    commit, and not push unless asked.
  - **In a project root:** committing is left to you.
  - **Agent:** it starts the agent you pick, else `harness.agent_command`;
    without an agent it refuses.

### The Roots page

`+` beside the sidebar's ROOTS heading opens the Roots page in the right-hand
column, where a document would be — the same way **New** opens its form, so the
roots you are changing stay on screen beside it. Knowledge and Specs share it,
and it is also offered from either section's empty state, where there is no
sidebar to put a `+` in.

The page:

- **Adds a root**, with the same three choices Settings offers — clone a
  repository, add an existing folder, create a new one. It is the same form,
  not a copy of it.
- **Lists every root** with its kind, health, path and what it holds, problems
  included, so a broken root is fixed from the same place it is listed.
- **Removes one.** A store is unregistered and its checkout stays on disk. A
  Specs folder root is dropped from `harness.specs.folders`. A project root has
  neither, so it has no Remove: it belongs to its repository. `okena-defaults`
  has none either — it is rewritten on every start.
- **Reorders Knowledge roots by dragging**, which saves at once and changes
  which copy of a template the next agent launch uses. The drop line sits along
  the top of the row you are over, so a root lands where that line is.
  `okena-defaults` is shown last without a handle. Specs roots are not layered,
  so the Specs list has no order and no handles.

### Elsewhere

- **Settings → Knowledge** clones a repository, adds an existing folder or
  creates a new store — the same three choices Settings → Specs offers. It
  removes a store from the registry while leaving the checkout on disk. It also
  switches project discovery on or off and sets the clone folder.

## Launch context

Every agent launcher can hand the agent more than its goal: entries of project
maps, specs, knowledge docs, skills and agents. Nothing is inlined. The brief
lists each by path, and skills and agents are loaded into the session where the
agent's CLI can take them.

### Picking

Projects and context are picked with a **Projects** and a **Context** chip
search, always in a dialog. The New agent dialog shows them directly. Every
other launcher — Start work, Break down, Refine, New change, Write with an
agent and Refine with agent — stays uncluttered: its gear opens a dialog with
the chip searches, and what is picked there stays with the launcher until it
starts.

- **Search:** type to narrow the results. ↑/↓ move, Enter adds the highlighted
  result as a chip, Backspace in an empty box removes the last chip, Esc closes
  the list, and a chip's ✕ removes it. An added item is not offered again.
- **Projects:** repositories, not worktrees or agent sessions, matched by name
  and path. Chips keep the order they were added in, which is the order the
  brief names them. A task's launch dialog preselects where a one-click start
  would go.
- **Context items:** each shows its kind, title, one-line description and owner.

  | Kind | From |
  |---|---|
  | Map entry | Each area, concept, exposed and consumed interface, CI pipeline and infrastructure resource of a valid [`project-map.yaml`](project-map.md#as-launch-context) |
  | Spec | Spec documents and active change folders of an OpenSpec root |
  | Knowledge doc, Skill, Agent | `docs/**`, `skills/**/SKILL.md` and `agents/**` of a knowledge root. Templates are not offered |

- **Where from:** every workspace repository and every root the Knowledge and
  Specs sections list.
- **Ranking:** items from the chosen projects, and from the stores they follow,
  come before the rest. Within each band, match quality plus how often an item
  has been added (adding one records it). Everything stays findable.
- **Not mapped:** a chosen project that has never been scanned gets a hint row
  in the results. **Scan** starts its [project scan](project-map.md#scanning),
  and its entries show up once the map is written.
- **On a card:** the projects on Break down, Refine and the document cards only
  rank context and are named in the brief; they create nothing.

### What the agent gets

From the launcher to the agent's first prompt:

1. **Refs.** The client sends refs, not paths. Each launch action
   (`TaskStartWork`, `SpecDraftChange`, `KnowledgeDraft`,
   `SpecRefineDocument`, `KnowledgeRefineDocument`, `AgentStartSession`) carries
   `context: [{ kind, owner, project_id | root_key, locator }]`, where the
   locator is a map id (`area:checkout`) or a path relative to the owning
   repository or store.
2. **Re-resolution.** The daemon resolves every ref again from its own roots,
   and drops any that no longer resolves.
3. **Skills and agents** are handed over per agent CLI.

   | Agent | How |
   |---|---|
   | `claude` | A plugin written for the session under `<profile>/agent-context/<id>/`, with copies of each skill directory and agent file, passed with `--plugin-dir`. The brief names them in the `context-installed` line instead of listing them |
   | Any other | Their `SKILL.md` or agent file paths are listed in the brief |

4. **The context block.** Everything else is listed in the brief under the
   `context` partial, grouped by project or store, one line per item: its kind
   (with the map id), title and absolute path. There are no descriptions; the
   agent reads what it needs.
   - **Budget:** the listed lines take at most 4 KB. The item that would go
     past it, and every item after it, are named by title only, on one line
     from the `context-more` partial, which points the agent at
     `okena_context_search` and `okena_context_read`.
   - **None picked:** a launch with no context has no block.
5. **The brief file.** The rendered brief is written to
   `<profile>/agent-briefs/<id>.md`, one file per launch, on every route: task
   start, several tasks, coordinator, spec draft, knowledge draft, doc refine,
   custom session and project scans. The agent's command holds
   `@okena-brief-file:<path>` where the brief went: positional for `claude`,
   after `--prompt` for `copilot`. Every other argument (`--session-id`,
   options, `--mcp-config`, `--plugin-dir`) is unchanged, and so is how
   restart reads that command. It resumes `--session-id` with `--resume`, and
   sends no prompt.
6. **The wrapper.** When the terminal spawns, okena runs
   `/bin/sh -c <wrapper> okena-brief <index> <file> <agent> <args…>`. The
   wrapper reads the file byte for byte and execs the agent with it at that
   argument position. This short command is what tmux, screen or dtach carries,
   and its size does not depend on the brief. tmux refuses a command of more
   than about 16 KB, which a long brief used to reach.
   - **Windows and WSL:** the brief is read back into argv. The host has no
     POSIX `sh`, psmux takes the command as argv tokens, and a WSL terminal
     would have to translate the Windows path.

- **Scope:** the session records the chosen projects and the projects owning
  the context it was handed (`context_projects`). Its own lookups are scoped to
  those and the stores they follow.

### Keeping current

The daemon keeps one index, backed by [fff](https://github.com/dmtrKovalenko/fff).

- **Lazy:** a root is read the first time a search reaches it, and cached.
- **Watched:** while cached, an fff watcher on the root makes the next search
  read it again after any change. Editing `project-map.yaml` or adding a doc
  shows up without restarting. A root is also re-read after 30 seconds, since
  fff does not watch gitignored paths.
- **Idle:** a root nobody has searched for 10 minutes loses its cache and its
  watcher.
- **Frecency:** kept in `<profile>/context-frecency`.

### Lookup from a running agent

okena's MCP server offers two tools. Both send only `$OKENA_TERMINAL_ID`, so
the daemon, not the agent, decides the scope.

- **`okena_context_search`:** `query` and an optional `limit` (at most 100).
  It searches only the session's projects and the stores they follow, and
  returns items as the launchers show them, with their absolute `path`.
- **`okena_context_read`:** `path`, as a search returned it. A file outside
  those roots is refused, and so is a path that resolves outside them. A
  shell in an ordinary repository is scoped to that repository. A session
  started with no projects has nothing to look up.

Every sent brief mentions both tools, through the `context-lookup` partial.

## Launch prompts

Every brief okena opens an agent with comes from a template, so an
organisation can change how its agents are briefed without waiting for a
release.

### Flows

A **flow** is a point at which okena briefs an agent. A store overrides one by
putting a file at `templates/briefs/<flow>.md`; its frontmatter should say
`for: <flow>`.

The briefs used to sit flat at `templates/<flow>.md`, beside the partials.
They moved into `briefs/` so the two kinds of file are told apart at a glance,
and **there is no migration**: a copy left at the old path is read by nothing.
okena sweeps the old paths out of its own `okena-defaults` store on each start;
an override of your own stays where you put it and simply stops applying.

| Flow | When | Variables |
|---|---|---|
| `task-start` | Starting work on a task, in its worktrees | `key`, `title`, `url`, `branch`, `description`, `projects`, `note`, `verify`, `context` |
| `tasks-start` | One agent starting work on several tasks — picked together, or a group a coordinator started with `okena_start_work`. Every task has worktrees of its own on its own branch, and `tasks` lists each with them | `key` (every key as one phrase), `tasks`, `note`, `verify`, `context` |
| `task-verify` | How the agent on a task plans its steps and proves each one through the `okena_test_*` tools. Never sent alone: rendered into `task-start` or `tasks-start` as `verify`, so either can be overridden without the other | `key`, `title` |
| `task-coordinate` | Splitting a task among sub-agents and starting them, from a `coordinate/…` branch of its own | `key`, `title`, `description`, `branch`, `children`, `projects`, `note`, `context` |
| `tasks-coordinate` | Splitting several hand-picked tasks among agents and starting them. It has no worktree: it runs in the project's own checkout, or above several, and `projects` lists the repos its agents work in | `key`, `title`, `tasks`, `projects`, `note`, `context` |
| `break-down` | Splitting a task into sub-tasks over MCP | `key`, `parent_id`, `title`, `kind`, `url`, `description`, `child_kind` |
| `task-create` | Drafting a new task | `title`, `kind`, `container`, `parent`, `description` |
| `task-refine` | Rewriting an existing task's title and description in place, after asking what it would otherwise guess | `key`, `title`, `kind`, `url`, `description` |
| `spec-draft` | Filling in a scaffolded OpenSpec change | `idea`, `change`, `change_dir`, `root_path`, `store_note`, `references`, `context` |
| `knowledge-draft` | Adding to or updating a knowledge root | `request`, `path`, `what`, `commit_note`, `context` |
| `doc-refine` | Changing one open spec, change file or knowledge file, without committing | `request`, `file`, `path`, `root_path`, `what`, `context` |
| `agent-session` | A free-form session against a goal you typed. Break down and Refine send their rendered brief as the goal | `goal`, `projects`, `context` |
| `extension-build` | Building an okena [extension](extensions.md) from a summary, started by **Build an extension** on the Extensions page. The summary is optional, so it is a block | `summary`, `projects`, `context` |
| `project-scan` | Writing or updating a repository's [project map](project-map.md#scanning) | `project`, `path`, `map_root`, `skill`, `start` |
| `projects-scan` | Finding [links](project-map.md#scanning-links) between repositories | `projects`, `skill` |

### Template syntax

Three forms, each about where text comes from — there are no conditionals or
loops, so reading a template tells you what the agent will be told:

| Form | Means |
|---|---|
| `{name}` | A value okena fills. |
| `{>partial}` | The text of `templates/partials/<partial>.md`. |
| `{name\|partial}` | The value, or the partial when the value is empty. |

`{{` and `}}` are literal braces. A placeholder the flow does not fill, or a
partial that does not exist, is left verbatim and reported — never silently
emptied. Partials may include partials, four deep.

### Partials

Every sentence okena says to an agent is in a flow template or a partial; none
is written in code. Where okena has to choose between wordings — a store or a
folder, who commits, a fan-out or a group — the choice is code and the words are
the partial it picks.

| Partial | Used for |
|---|---|
| `reporting` | Every flow sent on its own: report `state` and `suggestions` through `okena_report_status` when stopping to wait |
| `no-description`, `no-description-yet` | A breakdown or draft brief when there is no description |
| `given-projects`, `given-worktrees` | Heading over the `projects` list (`{list}`) |
| `spec-store-note`, `spec-folder-note` | How to use the `openspec` CLI (`{store_id}`, `{change}`) |
| `spec-references`, `spec-reference` | Referenced stores (`{list}`; `{store_id}`, `{path}`) |
| `knowledge-commit-store`, `knowledge-commit-project` | Who commits knowledge |
| `fan-out-note` | Each agent of a fan-out (`{parent}`, `{siblings}`) |
| `group-note` | An agent given several sub-tasks by a coordinator (`{also}`) |
| `picked-fan-out-note` | Each agent when hand-picked tasks start one per agent (`{siblings}`, one `picked-sibling` line each) |
| `picked-sibling` | One other task in that note, with where its agent works (`{key}`, `{branch}`, `{worktrees}`) |
| `picked-group-note` | One agent given several hand-picked tasks (`{also}`) |
| `task-in-group` | One task in `tasks-start`'s list (`{key}`, `{title}`, `{url}`, `{branch}`, `{worktrees}`, `{description}`) |
| `coordinate-child` | One task in a coordinator's list (`{key}`, `{kind}`, `{title}`, `{summary}`) |
| `scan-update`, `scan-repair`, `scan-from-docs`, `scan-from-code` | A project scan's starting point (`{manifest}`; `{list}` of problems or docs) |
| `context` | Heading over the [launch context](#launch-context) listed by path (`{list}`) |
| `context-installed` | The skills and agents loaded into the session instead of listed (`{list}`) |
| `context-lookup` | Every flow sent on its own: look context up with `okena_context_search` and `okena_context_read` |
| `context-more` | The launch context past the brief's 4 KB budget, by title (`{list}`), with the lookup tools |

Some variables are still assembled by okena, because they are lists or
optional blocks: `store_note`, `references`, `projects`, `note`, `children`,
`tasks`, `start`, `verify`, `context`.
Each is either empty or arrives with its own blank line in front, so a template
can place it on its own line without leaving a hole when it is absent. Their
words come from the partials above.

### Resolution

Per flow, per partial and per skill, okena reads every root it can see, in
order, and uses the first one that has the file:

1. Every root, in the order below.
2. okena's built-in, which is compiled in.

Nothing configures *which* root briefs come from — there is no setting naming
one as the source, and a file overrides a default simply by existing at the
same path in a root that comes earlier. `okena-defaults` is never a layer: it
holds a copy of the built-ins for reading, and the built-ins are step 2 already.

#### The order

Stores and project roots form **one ordered list**, saved as
`harness.knowledge.order` — root keys (`store:<id>`, `path:<absolute path>`),
top first:

- A root the list names sits where the list puts it, so a project root can be
  above a store or below it.
- A root the list does not name — one added since the order was last saved —
  goes to the **bottom**, keeping discovery order (registered stores in
  registry order, then project roots by project name) among its peers. It
  overrides nothing until it is moved up.
- **`okena-defaults` is always last and is never in the list.** It cannot be
  moved above the roots meant to override it.
- The order is saved with your settings, so it survives a restart. Keys for
  roots that are no longer discovered — unregistered, or a project that left
  the workspace — drop out of it the next time it is saved. A root whose
  checkout is merely missing keeps its place.

Keys, not paths: the order is a preference, while the checkout paths stay
machine state in the registry, so a synced `settings.json` means the same thing
on every machine. Arrange the list on the [Roots page](#the-roots-page).

A root can override one partial — say, `reporting` — without supplying any
template, and one template without supplying any partial. An empty file is not
an override and falls through to the next root, so a placeholder does not
silence the layer below it. A store that is unregistered, moved or unreadable
degrades to the next root rather than breaking every launch.

Only the kinds okena ships defaults for are layered: templates, partials and
skills. Docs and agents are listed from every root and read from the one you
opened them in.

Every launch is briefed. An agent's permission options and extra arguments
(`harness.agents`, see [configuration](configuration.md#agent-options)) are
passed alongside the rendered brief, before it, and never replace it.

### The `okena-defaults` store

okena's own briefs, partials and skills (the
[`project-map` skill](project-map.md#the-project-map-skill)) are written to
`<profile config dir>/knowledge/okena-defaults` and registered, so they are
readable in Harness → Knowledge like any other store. They are the same bytes
the built-ins render from, at the paths a root of your own would override them
at: `templates/briefs/<flow>.md`, `templates/partials/<name>.md` and
`skills/<name>/SKILL.md`. A file okena used to manage here and no longer does
is removed on the next start, so the store never shows a brief no launch reads.

- It is a knowledge root, not a git repository.
- **It is read-only.** okena rewrites every file in it to match the build on
  each start, so what the Knowledge view shows is always the text a launch
  would send. A file opens as a preview: there is no edit, save, rename, delete
  or refine, `New` and `Write with an agent` cannot target it, and the daemon
  refuses those actions rather than only hiding them. Editing a file here on
  disk does nothing — it is back to the built-in by the next start.
- To change a default, **Override** it: open it and pick one of your own roots.
  okena copies the file there at the same path, ready to edit, and
  [resolution](#resolution) then prefers it. A root that already has the file
  is opened rather than overwritten, and the picker says when a copy would lose
  to a root that comes earlier. Deleting your copy restores okena's. Which
  roots already hold a copy, and which one is applied, is the **Copies** list
  above — on a default and on your own copy alike.

## Limits

| Limit | Value |
|---|---|
| Entries listed per root | 5 000 (`entry_limit` warning past it) |
| Supporting files listed per skill | 200 |
| Bytes read per file when listing | 256 KiB |
| Largest file opened or saved | 2 MiB |
| Context results per launcher search | 50 |
| Context results per `okena_context_search` | 100 |
| Files copied per skill into a session plugin | 200 |
| Largest file `okena_context_read` returns | 2 MiB |

Reading and writing are confined to discovered roots. A root key the daemon did
not discover is refused, and so is a path that resolves outside its root. A
save replaces an existing file only; it never creates one.

Every read carries a `revision` of the text, and a save must hand it back. When
the file has changed on disk since it was opened (an agent or a terminal wrote
to it), the save is refused and nothing is written. The write goes through a
temporary file and a rename, and keeps the file's permissions.
