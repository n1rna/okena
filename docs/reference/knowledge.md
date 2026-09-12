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
└── templates/**/*.md             prompt templates
```

Every folder is optional. A root needs at least one kind folder or an identity.
Anything outside the four kind folders (a `README.md`, CI config) is ignored.

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
  - **Root list:** every root, with its health and a sync badge (`↑` commits to
    push, `↓` commits to pull, `•` uncommitted changes).
  - **Entry list:** the open root's entries grouped by kind, with docs nested
    by folder, and a filter over titles, names, paths, descriptions and tags.
    Markdown entries render formatted, and a skill lists its supporting files.
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
- **Settings → Knowledge** clones, adds and creates stores. `prompts` names the
  store launch briefs are read from; unset is okena's built-ins. It removes a store
  from the registry while leaving the checkout on disk. It also switches
  project discovery on or off and sets the clone folder.

## Launch prompts

Every brief okena opens an agent with comes from a template, so an
organisation can change how its agents are briefed without waiting for a
release.

### Flows

A **flow** is a point at which okena briefs an agent. A store overrides one by
putting a file at `templates/<flow>.md`; its frontmatter should say
`for: <flow>`.

| Flow | When | Variables |
|---|---|---|
| `task-start` | Starting work on a task, in its worktrees | `key`, `title`, `url`, `branch`, `description`, `projects`, `note` |
| `task-coordinate` | Splitting a task among sub-agents and starting them | `key`, `title`, `description`, `children`, `projects`, `note` |
| `break-down` | Splitting a task into sub-tasks over MCP | `key`, `parent_id`, `title`, `kind`, `url`, `description`, `child_kind` |
| `task-create` | Drafting a new task | `title`, `kind`, `container`, `parent`, `description` |
| `spec-draft` | Filling in a scaffolded OpenSpec change | `idea`, `change`, `change_dir`, `root_path`, `store_note`, `references` |
| `knowledge-draft` | Adding to or updating a knowledge root | `request`, `path`, `what`, `commit_note` |
| `agent-session` | A free-form session against a goal you typed | `goal`, `projects` |

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
| `reporting` | Every flow: report `state` and `suggestions` through `okena_report_status` when stopping to wait |
| `no-description`, `no-description-yet` | A breakdown or draft brief when there is no description |
| `given-projects`, `given-worktrees` | Heading over the `projects` list (`{list}`) |
| `spec-store-note`, `spec-folder-note` | How to use the `openspec` CLI (`{store_id}`, `{change}`) |
| `spec-references`, `spec-reference` | Referenced stores (`{list}`; `{store_id}`, `{path}`) |
| `knowledge-commit-store`, `knowledge-commit-project` | Who commits knowledge |
| `fan-out-note` | Each agent of a fan-out (`{parent}`, `{siblings}`) |
| `group-note` | An agent given several sub-tasks by a coordinator (`{also}`) |
| `coordinate-child` | One sub-task in a coordinator's list (`{key}`, `{kind}`, `{title}`, `{summary}`) |

Some variables are still assembled by okena, because they are lists or
optional blocks: `store_note`, `references`, `projects`, `note`, `children`.
Each is either empty or arrives with its own blank line in front, so a template
can place it on its own line without leaving a hole when it is absent. Their
words come from the partials above.

### Resolution

Per flow and per partial:

1. The store named by `harness.knowledge.prompts`, if it has the file.
2. okena's built-in.

A store can override one partial — say, `reporting` — without supplying any
template, and one template without supplying any partial. A store that is
unregistered, moved or unreadable degrades to the built-ins rather than
breaking every launch.

`harness.agent_args` is the exception. Where it is set it still wins for
`task-start`: it is an explicit instruction about how to launch that agent, and
more specific than any template.

### The `okena-defaults` store

okena's own templates and partials are written to
`<profile config dir>/knowledge/okena-defaults` and registered, so they are
readable in Harness → Knowledge like any other store. They are the same bytes
the built-ins render from.

- It is a knowledge root, not a git repository.
- okena keeps it current. `.okena-knowledge/defaults.lock` records a hash of
  each file as okena writes it. On start, a file still matching its hash is
  updated when the built-in changes; a file you have edited no longer matches
  and is left alone. A file with no record is only adopted if it already
  matches the built-in, since okena cannot tell an old default from an edit.
- To override a file for everyone, copy it into your own store at the same path
  and point `harness.knowledge.prompts` at that store.

## Limits

| Limit | Value |
|---|---|
| Entries listed per root | 5 000 (`entry_limit` warning past it) |
| Supporting files listed per skill | 200 |
| Bytes read per file when listing | 256 KiB |
| Largest file opened or saved | 2 MiB |

Reading and writing are confined to discovered roots. A root key the daemon did
not discover is refused, and so is a path that resolves outside its root. A
save replaces an existing file only; it never creates one.

Every read carries a `revision` of the text, and a save must hand it back. When
the file has changed on disk since it was opened (an agent or a terminal wrote
to it), the save is refused and nothing is written. The write goes through a
temporary file and a rename, and keeps the file's permissions.
