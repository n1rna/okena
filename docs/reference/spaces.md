# Spaces

A **space** is a named, separate set of the things okena shows you. One profile
holds several, and you move between them with the selector at the top of the
sidebar.

Each space has its own:

- projects, with their worktrees
- agents, the closed-agent history included (an agent session *is* a project)
- folders, which group projects inside its list
- tasks: one task backend connection, and the filters it is scoped to
- Knowledge and Specs roots, including their order

**A space is not a workspace.** okena already uses "workspace" for the one set
of projects and layouts saved per profile — `WorkspaceData`, `workspace.json`,
the `okena-workspace` crate, the **Delete workspace…** button on an agent
session panel. A space is the switch *above* that. Profiles
(`crates/okena-core/src/profiles.rs`) separate whole config directories; a space
separates what is inside one.

## Default

Every profile has a **Default** space. It cannot be renamed and it cannot be
deleted, and the selector does not offer either on it. Everything a profile had
before spaces belongs to it: on the first load after the update, `spaces` is
built from the harness config that was there, and every project and folder whose
row records no space reads as Default.

## Where a space lives

A space spans two files, which is why only the daemon writes one.

| What | Where |
|------|-------|
| The space list, its name, connection, filters and roots | `settings.json` → `spaces` |
| Which space is showing | `settings.json` → `active_space` |
| Which space a project or folder is in | `workspace.json` → each row's `space_id` |
| Task backend connections and their credentials | `<profile>/tasks_credentials.json` |

```jsonc
// settings.json
{
  "active_space": "client-a",
  "spaces": [
    { "id": "default", "name": "Default", "connection": "linear" },
    {
      "id": "client-a",
      "name": "Client A",
      "connection": "linear-2",
      "tasks": { "groups": { "project": ["alpha"] } },
      "specs": { "registry": false, "projects": true, "folders": ["~/acme/specs"] },
      "knowledge": { "projects": true, "stores": ["acme-eng"] }
    }
  ]
}
```

The active space belongs to the **profile**, not to a window or a client:
switching it in any window, on the web client or on the phone switches it
everywhere, and it is the same after a restart.

`harness.task_provider`, `harness.specs`, `harness.knowledge` and the legacy
`harness.spec_repo` are read once, folded into Default, and gone from the file
on the next save.

## What a switch changes

Switching spaces changes the Projects list, the Agents list, both overviews,
Tasks, Knowledge and Specs to that space's own. It does not stop anything:
**agents and terminals in a space you have left keep running**, because the
daemon owns them and a space is a view onto its projects, not a lifecycle.

A space's dot is marked while one of its live agent sessions is waiting on you,
so a space you are not looking at can still ask for you.

Any folder filter is cleared on a switch: a folder belongs to one space, and a
filter pointing at another space's folder would leave the list empty with no
visible reason why.

## The selector

One dot per space, at the top of the sidebar. The active one is highlighted,
hovering names it, and **+** adds one. When the sidebar is too narrow for every
dot, the ones that do not fit go into a **+N** chip whose menu lists them by
name — the active space is always drawn as a dot, even when its position would
have put it in the menu (`okena_core::spaces::fit_selector`).

Right-clicking a dot opens that space's menu: its task connection and filters,
and — on every space but Default — rename and delete.

Deleting a space names the projects and agents in it first. Confirming removes
them from okena and stops their agents; **files on disk are not touched**, and
the connection the space used is left alone.

### Shortcuts

| Action | Default binding |
|--------|-----------------|
| `NextSpace` | Cmd/Ctrl+Shift+] |
| `PreviousSpace` | Cmd/Ctrl+Shift+[ |
| `SwitchToSpace1` … `SwitchToSpace9` | Cmd/Ctrl+1 … Cmd/Ctrl+9 |

Next and previous wrap around. A number past the end does nothing rather than
landing somewhere arbitrary. All eleven are listed under **Spaces** in the
keybindings help and can be rebound like any other action. `FocusSidebar` moved
to Cmd/Ctrl+Shift+1 to make room for the number row.

## Tasks

A space reads **exactly one** task backend connection. Tasks from two
connections are never merged.

A **connection** is one named login. okena holds any number of them, of either
kind: a second Linear account or a second Azure DevOps organization is a new
connection with its own login, and several spaces may share one. They are listed
in Settings → Tasks, where they are added, renamed, signed in and removed.

Connection ids are what a space stores and what `okena_tasks::provider_for`
resolves. The first connection to a backend takes that backend's own id
(`linear`, `azure_devops`), which is what the credential file already used
before spaces — so a profile that had signed in keeps its login without signing
in again.

A connection a space still reads **cannot** be removed; the refusal names the
spaces in the way. Deleting a space never removes a connection.

### Filters are a hard scope

A space's filters are chosen from what its connection reports: the groupings
(team, project, iteration), labels and status. They combine the way the Tasks
filter bar's do — **any** of the values picked within one filter, **all** of the
filters together.

The scope is applied by the daemon before any task reaches a client
(`TaskScope::apply` in `okena_core::tasks`). The Tasks filter bar therefore
narrows *inside* a list that never held the rest, which is what makes "the
filter bar cannot show anything outside the scope" true for the Tasks view, the
CLI and an agent over MCP alike.

A space's connection and its filters can be edited at any time; Tasks follows
without a restart.

Creating a task is not blocked by the scope. The container picker offers the
teams and projects the scope names, so a new task defaults somewhere the space
can see it — but a create naming a container outside the scope still goes
through. Refusing it would leave a filtered space unable to file the very work
that moves a task into it.

## Agents and MCP

An agent belongs to the space its session is in, not to the space showing. Its
MCP calls resolve the backend from its own session's space and carry that
space's filters, so a coordinator in Client A keeps filing into Client A's
account while you look at Default.

A session an agent starts inherits the space of the repositories it was handed
(`Workspace::place_in_space_of`), for the same reason.

## Clients

Every client is space-aware. The daemon sends the spaces, which one is showing,
and each project's and folder's `space_id` in the state snapshot
(`StateResponse::{spaces, active_space}`, `ApiSpace`).

- **Desktop** — the full selector: dots, **+**, the **+N** menu, the per-space
  menu, and the forms.
- **Web** — the dot row above the project list; lists only the active space's
  projects and switches with `space_activate`.
- **Mobile** — the dot row at the top of the project drawer. `get_projects` and
  `get_folders` filter in Rust, so another space's project never reaches the RN
  layer.
- **TUI** — lists only the active space's terminals.

Spaces are added, renamed and deleted on the desktop; the other clients show
them and move between them.

A project mirrored from a *remote connection* (another machine) is shown in
whatever space you are in: its `space_id` names a space on that machine's
profile, which means nothing here.

## Compatibility

Every piece of this reads as Default when it is absent, so an older
`settings.json`, an older `workspace.json` and an older daemon's snapshot all
keep working:

- a project or folder row with no `space_id` → Default
- a settings file with no `spaces` → one Default space built from `harness.*`
- a snapshot with no `active_space` → Default, and every project in it reads as
  Default too, so a client lists the whole workspace
- an auth status with no `kind` → its connection id is the backend's id, which
  it was before connections existed

## Where the code is

| Piece | Where |
|-------|-------|
| `SpaceData`, id minting, cycling, `fit_selector` | `crates/okena-core/src/spaces.rs` |
| `TaskScope` and its matching rules | `crates/okena-core/src/tasks.rs` |
| `Connection`, id minting | `crates/okena-core/src/connections.rs` |
| The connection registry and credentials | `crates/okena-tasks/src/store.rs` |
| create / rename / delete / activate, and what a space holds | `crates/okena-workspace/src/spaces.rs` |
| `spaces` / `active_space` in settings, and the migration | `crates/okena-workspace/src/settings.rs` |
| Filtering the sidebar and the overviews | `crates/okena-workspace/src/visibility.rs` |
| The shared list the selector draws | `crates/okena-workspace/src/spaces_state.rs` |
| The wire types and actions | `crates/okena-core/src/api.rs` |
| The daemon's handlers | `crates/okena-daemon-core/src/command_loop.rs` |
| The selector | `crates/okena-views-sidebar/src/sidebar/space_selector.rs` |
| The forms | `crates/okena-app/src/views/overlays/space_dialog.rs` |
