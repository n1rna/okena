# Specs (OpenSpec)

A **spec origin** is an OpenSpec root: a folder holding an `openspec/` tree of
capabilities and changes ([OpenSpec](https://github.com/Fission-AI/OpenSpec)).
Spec origins are one of the three origin types of the
[Library](library.md), listed there under **Specs**.

The format belongs to OpenSpec. okena reads and writes the files the `openspec`
CLI does, in the shapes it does, so okena and the CLI work on one machine's
stores side by side: a store okena registers is listed by `openspec store
list`, and a store the CLI registers is listed in the Library. The CLI does not
need to be installed.

## Layout

```text
<root>/
├── .openspec-store/store.yaml    a store's identity (stores only)
└── openspec/
    ├── config.yaml               schema, `store:` pointer, `references:`
    ├── specs/**/spec.md          one capability per directory holding a spec.md
    └── changes/
        ├── <change>/             an active change
        │   ├── .openspec.yaml    change metadata
        │   └── proposal.md, …    its artifacts
        └── archive/<change>/     a completed change
```

What counts, following the CLI's discovery rules:

- **Capability:** any `spec.md` under `openspec/specs/`, one folder deep or
  more. Its id is the directory path, so `specs/platform/session/spec.md` is
  `platform/session`.
- **Change:** any directory under `openspec/changes/` other than `archive`,
  even one holding only `.openspec.yaml`, which is what `openspec new change`
  scaffolds. A change with no artifacts is listed with "no artifacts yet".
- **Archived change:** a directory under `openspec/changes/archive/`.
- **Skipped:** names starting with a dot, and symlinked directories. A
  symlinked `spec.md` counts only when it resolves inside the specs folder or
  its own capability directory.

## Discovery

okena follows OpenSpec's store model
([openspec.dev/docs/stores](https://openspec.dev/docs/stores)) and finds spec
origins in three places, deduplicated by resolved path:

| Kind | Found in | Library key |
|---|---|---|
| `store` | OpenSpec's machine registry, `<data>/stores/registry.yaml`: what `openspec store list` shows | `spec:store:<id>` |
| `project` | A project of the space whose repository holds an `openspec/` tree | `spec:path:<absolute path>` |
| `folder` | `spaces[].library.spec.folders` | `spec:path:<absolute path>` |

- **Pointers:** a project whose `openspec/config.yaml` only says `store: <id>`
  resolves to that store, as the CLI does. It adds the project to the store's
  "used by" list and no origin of its own.
- **References:** each root's `references:` are resolved against the registry
  and listed on the origin.
- **Worktrees and agent sessions** are never searched.
- **Problems** are reported on the origin with OpenSpec's own diagnostic codes,
  so they read the same as what `openspec doctor` says. One broken store never
  hides the others.

Listing registered stores and searching projects can each be switched off per
space ([configuration.md](configuration.md#library)). With the registry
listing off, a store is still shown when a project points at it.

## Stores

A store is a root registered on the machine under an id, which
`--store <id>` selects in the CLI. The Library adds and removes them through
OpenSpec's own files:

| In the Library | Same as |
|---|---|
| **Clone a repository** | `git clone`, then `openspec store register <path>` |
| **Add an existing folder** | `openspec store register <path> [--id <id>]` |
| **Create a new store** | `openspec store setup <id> --path <path> [--remote <url>]`, with or without `--init-git` |
| **Remove** on a store | `openspec store unregister <id>` |
| **Make default** / **Clear default** | `openspec config set defaultStore <id>` / `openspec config unset defaultStore` |

The rules are the CLI's:

- **Registry lock:** every change to `registry.yaml` takes the CLI's
  `registry.yaml.lock`, so okena and a running `openspec` never write over each
  other.
- **One-to-one:** a store id has one checkout, and a checkout has one id.
- **Identity:** a store's id comes from `.openspec-store/store.yaml`.
  Registering a root that has none writes one; commit it so every clone carries
  the same id.
- **Create** refuses a file, a non-empty folder that is not a root, and a
  folder inside another git repository. It makes one initial commit, and if a
  step fails before that commit everything it created is removed.
- **Clone:** with no destination, the clone goes into
  `spaces[].library.spec.clone_dir` (default `~/openspec`), in the folder
  `git clone` would name. A repository that turns out not to be an OpenSpec
  root stays on disk, and the error says where it is.
- **Remove** takes the store out of the registry and leaves the checkout on
  disk. Removing a `folder` origin drops it from
  `spaces[].library.spec.folders` instead. A `project` origin has no Remove.

The store marked **machine default** is OpenSpec's `defaultStore`, read from
`<config>/config.json`. A `defaultStore` naming a store that is not registered
is reported as a problem.

The data and config directories resolve as the CLI resolves them:
`$XDG_DATA_HOME/openspec`, else `~/.local/share/openspec`
(`%LOCALAPPDATA%\openspec` on Windows), and `$XDG_CONFIG_HOME/openspec`, else
`~/.config/openspec` (`%APPDATA%\openspec` on Windows). Both can be set per
space.

## In the Library

Opening a spec origin shows its tree: **Changes**, then **Specs**, then the
archive. The page around it (search, editing, the origin overview with git, the
Origins page) is the same for every origin type and is described in
[library.md](library.md#the-library-page).

- **Files:** `+` creates a change folder, a capability
  (`openspec/specs/<name>/spec.md`) or a document inside a change. The open
  document has **Rename** and **Delete…**. Deleting a change's last document
  leaves the change listed.
- **New change** writes the `.openspec.yaml` that `openspec new change` writes,
  plus a stub `proposal.md` holding the idea, then starts an agent in the root
  to fill the change in. An existing change of that name is never overwritten.
  When the root is a store, the agent is told to pass `--store <id>` to the
  CLI; a folder or project root is reached from its own directory.
- **Refine with agent** starts an agent on the open document.
- **CLI hint:** the overview shows how to reach the origin from a terminal:
  `openspec list --store <id>` for a store, `cd <path> && openspec list`
  otherwise.
- **Launch context:** spec documents and active change folders are offered in
  every launcher's [context search](knowledge.md#launch-context).

Spec origins are not layered: they have no order, and nothing in one overrides
anything in another.

## Limits

Reading and writing are confined to discovered origins. A key the daemon did
not discover is refused, and so is a path that resolves outside its origin.
