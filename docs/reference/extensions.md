# Extensions from git

An extension is a WASM component that okena's **daemon** runs, installed from a
git repository. It reads the machine only through calls the user approved at
install, returns a declarative view that okena draws natively (no web views),
offers actions and queries, and can launch okena agent sessions. Because it runs
in the daemon, local and remote clients show the same thing.

The four extensions compiled into okena (claude, codex, github, updater) are a
separate, older mechanism (`okena-extensions`); they share only the
`enabled_extensions` setting and Settings → Extensions page.

Why this shape: [ADR-0007](../decisions/0007-wasm-extensions.md).

| Piece | Where |
|---|---|
| WIT interface (`okena:extension@0.1.0`) | `crates/okena-extension-api/wit/extension.wit` |
| Rust SDK | `crates/okena-extension-api` |
| Host: manifest, permissions, dependency check, wasmtime, install | `crates/okena-extension-host` |
| Daemon wiring (`Extension*` actions, snapshot, update checks) | `crates/okena-daemon-core/src/extensions.rs` |
| Wire types (what clients see) | `crates/okena-core/src/extension.rs` |
| Native views, Settings → Extensions | `crates/okena-views-extensions` |
| Example library | `examples/extension-library` |
| Template | `examples/extension-template` |
| The `extension-build` brief the Build an extension button launches with | `crates/okena-knowledge/src/prompts/templates/briefs/extension-build.md` |

## A library repo

One repository can hold many extensions, each in its own folder with its own
`extension.toml`. Installing names the repo's URL, an optional ref (branch, tag
or commit; the default branch when empty) and the folder:

```text
my-extensions/
├── Cargo.toml              [workspace] members = ["extensions/*"]
├── rust-toolchain.toml     channel + targets = ["wasm32-wasip2"]
├── Cargo.lock
└── extensions/
    ├── cli-table/
    │   ├── extension.toml
    │   ├── extension.wasm  optional prebuilt component
    │   ├── Cargo.toml      crate-type = ["cdylib"]
    │   └── src/lib.rs
    └── git-tree/ …
```

Pointing an install at the repo's root, which has no manifest, fails with the
list of folders that do.

## `extension.toml`

```toml
id = "cli-table"            # 1-64 of a-z 0-9 -, unique on the machine
name = "CLI table"
version = "0.1.0"           # semver
description = "…"
api = "0.1"                 # the WIT interface version it was built against
refresh_interval_secs = 30  # 0: only on demand; below 5 is raised to 5

[view]                      # present when it has a view of its own
title = "Jobs"              # its entry under HARNESS

[permissions]
commands = ["jq"]           # program names it may run (no shell, no arguments)
paths = ["{config.jobs_file}", "~/.cache/jobs"]  # read access, and everything under
start_agents = true         # may start agent sessions without the user pressing Start

[[requires]]
name = "jq"
check = ["jq", "--version"] # run by okena; must exit 0
min_version = "1.6"         # optional; the first N.N[.N] in its output is compared
install_hint = "brew install jq"

[[config]]
key = "jobs_file"           # a-z 0-9 _
label = "Jobs file"
type = "path"               # string | path | number | bool | select
description = "…"
required = false            # a required empty field holds the extension back
default = ""                # any JSON value
options = []                # the choices of a `select`
```

`{config.<key>}` in a path permission is filled in from the configuration when
the call is checked, so an extension can be granted "the folder you configure"
without knowing it at install. An empty field grants nothing.

## Permissions

Declared in the manifest, shown before install, approved as a whole, and
enforced on every call. What is enforced is what the user approved, not what the
manifest says now.

- **Commands**: the program name must be in `commands` exactly; it is looked up
  on the daemon's extended PATH and run without a shell. A working directory must
  be under an approved path. Once approved, a program runs with the daemon's
  environment: approving `aws` approves what `aws` can do.
- **Paths**: `read-file` and `read-dir` resolve the path (following symlinks)
  and allow it only under an approved root. `..` and symlinks cannot climb out.
- **Starting agents**: needed for actions whose agent mode is `start`.

A refused call returns an error to the extension, is logged, and is shown on
its view in a red "okena refused…" banner (the latest ten are kept in the
snapshot as `refusals`).

An update whose manifest asks for more than was approved is not installed until
the user approves the new set (Settings shows "Update (asks for more)" and the
approval card lists what is new).

## The sandbox

An extension has no WASI access of its own: no filesystem preopens, no
environment, no network. Its `stderr` is kept (last 16 KB) for crash messages.
Per call it may compute for 10 s (time in host calls does not count) and grow to
256 MB. A panic, running over, or running out of memory fails that call only;
the next call starts a fresh instance. Its storage survives, its in-memory state
does not. A crash never takes the daemon down.

Commands time out after 60 s by default (an extension may ask for up to 5 min);
output past 16 MB is dropped. Files over 32 MB are refused. Storage holds at
most 8 MB.

## Host calls

From `interface host` in the WIT; the SDK wraps each.

| Call | SDK |
|---|---|
| `run-command(command)` | `Command::new("jq").args([…]).stdin(…).run()` / `.output()` |
| `read-file(path)`, `read-dir(path)` | `host::read_to_string`, `host::read_file`, `host::read_dir` |
| `kv-get/set/delete/keys` | `host::storage::{get, set, delete, keys, get_json, set_json}` |
| `log(level, message)` | `host::info`, `host::warn`, `host::error` |
| `config()` | `host::config::<T>()` (defaults filled in), `host::config_json()` |
| `projects()` | `host::projects()` — okena's projects, to pick where agents work |

Storage lives in `<profile>/extensions/data/<id>/kv.json` and is deleted with
the extension.

## What the extension exports

```rust
impl Extension for MyExt {
    fn new() -> Self;
    fn describe(&self) -> Info;                     // actions and queries, asked once per load
    fn refresh(&mut self) -> Result<Refresh>;       // the view and the status bar label
    fn run_action(&mut self, request: ActionRequest) -> Result<ActionOutcome>;
    fn query(&mut self, id: &str, args: Value) -> Result<Value>;
}
okena_extension_api::register_extension!(MyExt);
```

`refresh` runs when the extension starts, on its interval, on the view's
Refresh button, after its configuration changes, and after an action whose
outcome asks for it (`.and_refresh()`).

## The view

A view is a flat list of nodes; containers refer to their children by index
(WIT types cannot be recursive). The SDK builds it bottom-up: add children, then
the container that holds them. okena drops child indices that point at the node
itself or a later one, so a view cannot loop.

| Component | SDK |
|---|---|
| Text: body, heading, muted, code, toned | `ui::text`, `heading`, `muted`, `code`, `toned` |
| Badge(s) | `ui::badge`, `ui::badges` |
| Table | `ui::Table::new(id).column(…).rows(…).group_by(…).sort_by(…).row_actions([…]).bulk_actions([…])` |
| Detail pane (key-value) | `ui::detail(title, fields)`; rows and tree items carry their own `.detail(Field)` |
| Collapsible tree | `ui::Tree::new(id)`, `tree.add(parent, TreeItem)`, `.with_detail_pane()` |
| Bar / line chart | `ui::Chart::new().series(label, points).bar()` / `.line()` |
| Stat tiles | `ui::stats([ui::Stat::new(label, value).tone(…).hint(…)])` |
| Action buttons | `ui::actions(["reset"])` |
| Loading, empty, error | `ui::loading`, `ui::empty`, `ui::error` |
| Layout | `view.stack(…)`, `view.columns(…)`, `view.section(title, …, collapsible)` |

Tables: okena groups (by any `groupable` column), sorts (click a header:
ascending, descending, off; numbers sort by the cell's number), filters (every
word must appear in a cell, the row id or its detail) and selects rows itself,
so none of it needs a refresh. A row's `id` must be stable: selection, actions
and agent badges are keyed by it. okena draws the first 500 rows of a
table; the host keeps at most 5 000 rows and 2 000 nodes.

The status bar widget is `Refresh::status(label, tone)`, with an optional
tooltip; clicking it opens the view.

**Versions.** Components are `interface ui-v1`. New components come in a new
interface and world version; an extension built against `0.1` keeps working.
A component a client cannot draw shows as "needs a newer okena".

## Actions

```rust
Action::new("delete", "Delete")
    .description("Remove the jobs for good.")
    .destructive()                          // okena asks the user first, whoever asks
    .input(Input::text("reason", "Reason").required())   // a small form first
    .agent_callable()                       // agents it started may run it
```

A table lists actions by id as `row_actions` (one row) and `bulk_actions` (the
selected rows); `ui::actions` puts view-level buttons anywhere. When clicked,
okena shows the form (if any), then asks to confirm a destructive action, runs
it while showing "Running…", and reports the outcome's message as a toast and
a banner (errors in red).

## Agent sessions

An action declared with `.launches_agent(mode)` returns
`ActionOutcome::launch(AgentLaunch::new(brief).name(…).root(…).project(id).context(ref).item(row_id, label))`.
The extension writes the brief itself.

- `AgentMode::Prefill`: okena's New agent dialog opens with the brief, name,
  root, projects and context filled in; the user edits and starts it.
- `AgentMode::Start`: the daemon starts the session at once with the default
  agent (`harness.agent_command`). Needs the `start_agents` permission; like any
  session it needs a root, a project, or `harness.agent_root`.

Sessions are tagged `AgentPurpose::Extension { extension, item, item_label }`.
The item's row shows a badge with the session's state (working, needs you,
done…) that opens it, across refreshes; the session panel shows "from
<extension> · <item>".

## okena's MCP, for the agent an extension started

| Tool | Does |
|---|---|
| `okena_extension_tools` | The extension that started this session: its agent-callable actions, its queries, the item. |
| `okena_extension_query` | `{query, args}` — the extension's JSON answer. |
| `okena_extension_action` | `{action, items, inputs}` — runs an agent-callable action. |

The calling terminal must belong to a session that extension started. Actions
not marked agent-callable, and actions that launch agents, are refused. A
destructive action waits (up to ten minutes) for the user: okena shows a toast
with Run / Decline and the same request as a banner on the extension's view,
and the agent gets the outcome or "the user did not confirm".

## The dependency check

Before loading, the daemon runs each `[[requires]]` check on its extended PATH.
While any tool is missing, too old, or its check fails, the extension runs
nothing and its view lists what is wrong with the install hint and a
**Re-check** button (`ExtensionRecheck`). Refresh also re-runs the checks when
the extension is not ready.

## Install, update, remove

From Settings → Extensions (or the daemon's `Extension*` actions):

1. **Review…** fetches the ref into `<profile>/extensions/cache/repos/…`
   (`ExtensionPreview`) and shows name, version, source and commit, the
   permissions, the required tools, and whether it uses a prebuilt
   `extension.wasm` or builds from source. When building needs Rust or the
   `wasm32-wasip2` target and this machine lacks it, the card says so and
   Approve is disabled.
2. **Approve and install** installs exactly the reviewed commit
   (`ExtensionInstall`); the approved permissions must equal the manifest's.
   Building runs `cargo build --release --target wasm32-wasip2` in the folder
   (the repo's `rust-toolchain.toml` applies), into a shared target dir. The
   component must compile before anything is replaced. The extension is enabled.
3. The registry `<profile>/extensions/installed.json` records the source (URL,
   ref, path, resolved commit) and the approved permissions; files live in
   `installed/<id>/`.
4. Every six hours, and on **Check for updates**, the daemon asks each remote
   (`git ls-remote`) whether the ref moved. A moved ref shows "0.2.0 available";
   **Update** is manual and asks again for any new permission. A ref that is a
   commit never updates.
5. **Remove…** deletes the files, the data, and the extension's settings.

Ids of the built-in extensions cannot be installed over.

## Configuration

One configuration per installed extension, in `extension_settings.<id>`, edited
in the extension's row under Settings → Extensions (a form drawn from its
`[[config]]` schema). Saving applies without a restart: the extension reads the
new values, path permissions follow them, and the view refreshes. Enabling and
disabling is the `enabled_extensions` set, as for built-in extensions. A remote
daemon's extensions are configured on that daemon.

## Building and the dev loop

```sh
rustup target add wasm32-wasip2        # once; a rust-toolchain.toml with
                                       # targets = ["wasm32-wasip2"] does it for you
cargo build --release --target wasm32-wasip2
# → target/wasm32-wasip2/release/<crate_name>.wasm
```

For development, install **From a local folder** (the folder holding
`extension.toml`). okena builds it in place; after editing, **Rebuild & reload**
(`ExtensionReload`) rebuilds and swaps it in, keeping its storage and
configuration. A reload that asks for more permissions asks for approval.

To ship a prebuilt component, copy the built `.wasm` to `extension.wasm` next to
the manifest and commit it; installs then skip building.

The template in `examples/extension-template` is a starting point.

## Build an extension with an agent

Writing one is a doc-reading job — this page, the template, the
`wasm32-wasip2` target — so the **Extensions** page's header has a **Build an
extension** button that hands it to an agent instead.

It opens the agent launcher: you type a summary of the extension (optional —
without one the agent asks), pick the agent, model, working directory,
projects and context as usual, and start. The working directory is where the
agent creates the extension's folder.

The brief comes from the `extension-build`
[launch flow](knowledge.md#flows), so the launcher's chip reads
`extension-build · <model>` and a knowledge root can replace it with its own
`templates/briefs/extension-build.md`. okena's built-in tells the agent to read this
page, start from `examples/extension-template`, read
`examples/extension-library` for worked examples, and build for
`wasm32-wasip2`.

The agent stops at a folder that is ready to install; installing stays with
you, because it means approving the extension's permissions. Install it from
Settings → Extensions → Install an extension → **From a local folder**, as
above.

## Wire and API

`StateResponse.extensions: Vec<ApiExtension>` carries every installed
extension: manifest facts, run state (`disabled`, `starting`, `missing_tools`,
`needs_config`, `ready`, `failed`), tool statuses, the view and status, actions
and queries, refresh state and error, refusals, pending confirmations, and an
available update. Actions: `ExtensionPreview`, `ExtensionInstall`,
`ExtensionPreviewUpdate`, `ExtensionUpdate`, `ExtensionCheckUpdates`,
`ExtensionReload`, `ExtensionRemove`, `ExtensionRefresh`, `ExtensionRecheck`,
`ExtensionRunAction`, `ExtensionQuery`, `ExtensionAgentTools`,
`ExtensionAgentCall`, `ExtensionConfirm`. All run off the daemon's command queue.
