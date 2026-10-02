# Library

The **Library** is the harness section that holds what a team writes down for
people and agents: knowledge, specs and plain markdown. Each folder it lists is
an **origin**, and every origin has a **type** that says what okena expects
inside it.

| Type | What it holds | Format | Reference |
|---|---|---|---|
| `knowledge` | Docs, skills, agents and prompt templates | okena's knowledge layout | [knowledge.md](knowledge.md) |
| `spec` | Capabilities and changes | OpenSpec, as the `openspec` CLI reads it | [specs.md](specs.md) |
| `freeform` | Any markdown: workflows, decisions, notes | No layout | [Freeform origins](#freeform-origins) |

The types share one page, one origin list, one Settings category and one set of
actions. They differ in what an origin's tree looks like and in two behaviours:

- **Layering** applies to `knowledge` origins only. They form one ordered list,
  and a template, partial or skill in an earlier origin overrides the same file
  in a later one ([resolution](knowledge.md#resolution)). `spec` and `freeform`
  origins are not layered: they have no order and no overrides.
- **Drafting with an agent** is briefed per type. **New** in a `knowledge`
  origin opens **Write with an agent**, briefed on the knowledge layout; in a
  `spec` origin it opens **New change**, which scaffolds the change first; in
  a `freeform` origin it opens **Write with an agent**, briefed to follow what
  the folder already does, since there is no layout to state.

**Refine with agent**, the card under an open document, is the same for every
type: say what to change, and an agent session opens in the origin, told to
edit only that file and not to commit. A file with unsaved edits has to be
saved first, and a file of `okena-defaults` has no card.

## Origins

An origin is a folder. How it was found is its **kind**:

| Kind | What it is |
|---|---|
| `store` | A checkout registered on this machine: a knowledge store in okena's registry, or an OpenSpec store in OpenSpec's |
| `project` | A folder inside one of the space's projects: its knowledge root, or its `openspec/` tree |
| `folder` | A folder named in settings: an OpenSpec folder, or a freeform origin |

Each type finds its origins its own way; [knowledge.md](knowledge.md#registry)
and [specs.md](specs.md#discovery) describe theirs. A freeform origin is always
a `folder`.

### Keys

An origin is named by a **Library key**: its type, then the key its own
discovery gives it.

```text
knowledge:store:acme-eng
knowledge:path:/Users/me/acme/api/.okena/knowledge
spec:store:team-plans
spec:path:/Users/me/acme/api
freeform:path:/Users/me/notes
```

Every Library action that names an origin takes this key, and so do the launch
context refs of an item in one. The type is part of the key, so a knowledge
store and an OpenSpec store with the same id are two origins.

The saved layering order holds the part after the type (`store:acme-eng`),
since only knowledge origins are in it.

## The Library page

**Harness → Library** has two columns. The left one lists the origins and what
the open one holds; the right one shows the open document, or the open origin's
overview when no document is open.

- **Origin list:** every origin of the space, grouped under **Knowledge**,
  **Specs** and **Freeform**. A type with no origins has no heading. Knowledge
  origins are listed in the order they layer in. Each row carries the origin's
  health and, for a git checkout, a sync badge (`↑` commits to push, `↓`
  commits to pull, `•` uncommitted changes). `+` beside the ORIGINS heading
  opens the [Origins page](#the-origins-page).
- **Which origin opens:** the one last open; else the first healthy store that
  can be written in; else the first origin that can be written in; else the
  first healthy one; else the first.
- **Tree:** below the list, what the open origin holds, in its type's own
  shape: knowledge entries by kind, an OpenSpec root's changes and
  capabilities, or a freeform origin's documents under their folders.
- **Followed, not here:** projects that follow a knowledge store which is not
  on this machine.

### Search

A bar floats at the bottom of the page with a search box and a **Filters**
button. `Cmd+F` (`Ctrl+F`) puts the cursor in the box.

- **Text** is matched, trimmed and without case, against every document's
  title, name, path and content, in **every** origin, not just the open one. A
  folder name matches, because it is part of the path.
- **Filters** holds three: **Origin**, **Type** (knowledge, spec, freeform) and
  **Kind** (doc, skill, agent, template, partial, brief). The button shows how
  many values are picked. Values within a filter widen, and the filters narrow
  each other and the text. **Kind** applies to knowledge entries, so picking
  one leaves only knowledge origins in the results.
- **Results** replace the tree while anything narrows the page. They are listed
  under the origin each is in, with "N of M", and clicking one opens it in its
  origin. **Clear**, or `Esc` in the box, brings the tree back.

The daemon does the matching (`library_search`), since a client holds only file
names until a file is opened.

### Editing

An opened file can be edited and saved with `cmd-s` (`ctrl-s`). A Markdown file
toggles between **Edit** (the source) and **Preview**; any other file opens
straight in the editor. A file with unsaved edits is marked `●` in the tree and
keeps its edits while another file is open. **Revert** drops them and reloads
the file.

Every read carries a `revision` of the text, and a save must hand it back. When
the file has changed on disk since it was opened, the save is refused and
nothing is written. A save replaces an existing file only; it never creates
one.

### Files

`+` beside a tree heading creates a document; what it offers depends on the
origin's type:

| Type | `+` creates |
|---|---|
| `knowledge` | An entry of a chosen kind, starting from that kind's frontmatter |
| `spec` | A change folder, a capability (`openspec/specs/<name>/spec.md`), or a document inside a change |
| `freeform` | A markdown file |

A name with slashes (`ci/pipeline`) makes the folders. The open file's
**Rename** moves it, and the open document follows with any unsaved edits.
**Delete…** asks first, then removes the file and closes it. Paths are checked
the way reads are: nothing outside the origin, no hidden names, and never over
an existing file.

### Origin overview

With no document open, the right column shows the open origin: its type and
kind, its path, what it holds and its problems. A git checkout also shows the
branch, the last fetch, and **Fetch**, **Pull** and **Push** buttons, then the
uncommitted files and a commit box. Pull and Push are offered only when they
can succeed, and a line says why not while there is something to pull or push.
Leaving the commit message blank commits with a default that names the file, or
the number of files.

Git behaves the same for every type and is described under
[Sync](knowledge.md#sync). A `project` origin is synced with its project's own
git, so it has no git panel.

### The Origins page

`+` beside the ORIGINS heading opens the Origins page in the right-hand column,
where a document would be, so the origins being changed stay on screen beside
it. An empty Library offers it from its empty state.

The page:

- **Lists every origin** under its type, with its kind, health, path and what
  it holds, problems included, so a broken origin is fixed from the same place
  it is listed.
- **Adds an origin.** Pick the type, then one of three ways:

  | Way | `knowledge` | `spec` | `freeform` |
  |---|---|---|---|
  | **Clone a repository** | Clones and registers the store | Clones and registers the store | Clones and lists the folder |
  | **Add an existing folder** | Registers the checkout | Registers the checkout, with an optional store id | Lists the folder |
  | **Create a new store** / **Create a new folder** | Writes the identity and kind folders | What `openspec store setup` writes | Writes a `README.md` |

  With no destination, a clone goes into the type's clone folder, in the folder
  `git clone` would name. Creating can run `git init` and make one initial
  commit.
- **Removes one.** The folder always stays on disk.
  - A `store` is unregistered.
  - A `spec` folder and a `freeform` origin are dropped from the space's
    settings.
  - A `project` origin has no Remove: it belongs to its repository.
  - `okena-defaults` has none either; it is rewritten on every start.
- **Reorders knowledge origins by dragging**, which saves at once and changes
  which copy of a template the next agent launch uses. The drop line sits along
  the top of the row you are over. `okena-defaults` is shown last without a
  handle. `spec` and `freeform` origins have no handles.

### Settings → Library

One Settings category covers every type. It lists the origins by type with the
same add form the Origins page uses, and holds what the page does not:

- **Discovery:** whether to find knowledge in projects, whether to list every
  registered OpenSpec store, and whether to find OpenSpec roots and `store:`
  pointers in projects.
- **OpenSpec folders:** extra folders shown as `spec` origins.
- **Clone folders:** one per type.
- **OpenSpec directories:** the data and config directories, when they are not
  where the CLI looks by default.
- **Machine default:** **Make default** and **Clear default** on an OpenSpec
  store set OpenSpec's `defaultStore`.

The keys are listed in [configuration.md](configuration.md#library).

## Freeform origins

A freeform origin is a folder of markdown with no layout. It is listed because
its path is in the space's `library.freeform.folders`.

- **Documents:** every `.md`, `.markdown` and `.mdx` file under the folder, at
  any depth up to 12 folders, sorted by path.
- **Skipped:** hidden names, symlinks, and folders named `node_modules`,
  `target`, `dist`, `build` or `vendor`.
- **Title:** the frontmatter `title`, else the first `# ` heading, else the
  file name without its extension.
- **Limit:** 2 000 documents per origin; past it the origin carries a warning
  and lists the first 2 000.
- **Missing folder:** the origin stays listed, unhealthy, with
  `freeform_folder_missing`.

Adding one:

- **Clone** clones the URL and adds the checkout. A failed clone removes a
  folder okena created.
- **Add an existing folder** lists the folder. Adding a folder that is already
  listed changes nothing.
- **Create a new folder** takes an empty folder, writes a `README.md` headed
  with the name given (the folder name when none is), and with git makes one
  commit, `Initialize <name>`. A folder that is not empty is refused; add it as
  an existing folder instead.

A freeform origin can be browsed, edited, searched, committed and pushed, and
its documents can be attached as [launch context](knowledge.md#launch-context).
It takes no overrides and is not in the layering order.

Agents in a freeform origin:

- **Write with an agent** (**New**) opens an agent session in the folder with
  the `freeform-draft` brief: read what is there, follow its folders, file
  names and conventions, start each new file with a `# ` heading, and do not
  commit. The session shows as a **Drafting** row above the documents until a
  document that was not there before is listed.
- **Refine with agent** on an open document uses the `doc-refine` brief, as in
  every other origin.
- `+` beside DOCUMENTS still creates a file to write yourself.

## Actions

The daemon serves the Library through one set of actions. `root` is a
[Library key](#keys); `type` is `knowledge`, `spec` or `freeform`.

| Action | What it does |
|---|---|
| `library_origins` | Every origin of the space, with health, git state and counts |
| `library_tree` | What one origin holds |
| `library_read` | One document, with its `revision` |
| `library_search` | Documents matching `query`, narrowed by `roots`, `types` and `kinds` |
| `library_write` | Save a document, given the `revision` it was read at |
| `library_file_create`, `library_folder_create` | Create a document or a folder |
| `library_file_rename`, `library_file_delete` | Move or remove a document |
| `library_overrides` | For one file of `okena-defaults`: the knowledge origins that could hold a copy, which do, and which copy wins |
| `library_layering` | For every layered file: the knowledge origins holding a copy, and the one applied |
| `library_override` | Copy a default into a knowledge origin, at the same path |
| `library_store_clone` | Clone a repository as an origin of `type` |
| `library_store_register` | Add an existing folder as an origin of `type` |
| `library_store_setup` | Create a new origin of `type` |
| `library_store_unregister` | Remove an origin; its folder stays |
| `library_set_default_store` | Set or clear OpenSpec's machine `defaultStore` |
| `library_store_fetch`, `library_store_pull`, `library_store_commit`, `library_store_push` | Git on an origin's checkout |
| `library_draft` | Start an agent drafting in an origin, with its type's brief |
| `library_refine_document` | Start an agent refining one document of an origin |

Some actions belong to one type. `library_overrides` and `library_layering`
answer about knowledge origins only, and `library_override` refuses an origin
of another type. `library_set_default_store` names an OpenSpec store. The
order knowledge origins layer in is a setting,
`spaces[].library.knowledge.order`, not an action.

Reading and writing are confined to discovered origins. A key the daemon did
not discover is refused, and so is a path that resolves outside its origin.

## Carried over

A profile saved when Specs and Knowledge were two harness sections, with two
Settings categories, is read as follows:

- A saved harness section of `specs` or `knowledge` opens the Library.
- `spaces[].specs` and `spaces[].knowledge` in `settings.json` are read into
  `spaces[].library.spec` and `spaces[].library.knowledge`, the saved knowledge
  order included, and written in the new shape on the next save
  ([configuration.md](configuration.md#library)).
- A root key without a type, held by an agent session started before the
  Library, is read as a key of that session's type.
