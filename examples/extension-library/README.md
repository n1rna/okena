# Example extension library

A library of okena extensions laid out the way a team's own repo would be: one
folder per extension under `extensions/`, each with its own `extension.toml`.
Install one from okena with this repository's URL and the extension's folder,
e.g. `examples/extension-library/extensions/cli-table`.

| Extension | Shows | Needs |
|---|---|---|
| [`cli-table`](extensions/cli-table) | Jobs per tenant, computed by `jq`: a grouped table with a row action (Unblock), a form action (Skip), a destructive bulk action (Delete), and two agent actions on a row — *Investigate…* fills in okena's launcher, *Investigate now* starts an agent at once. Ships a prebuilt `extension.wasm`. | `jq` 1.6+ |
| [`git-tree`](extensions/git-tree) | A repository's files as a tree with a detail pane, stat tiles, a bar chart and a line chart — from a configured local clone, or a GitHub remote through `gh`. Built from source at install. | `gh` 2.0+ |

Build both: `cargo build --release --target wasm32-wasip2` here. To refresh
cli-table's prebuilt component, copy
`target/wasm32-wasip2/release/okena_ext_cli_table.wasm` to
`extensions/cli-table/extension.wasm`.

The SDK is a path dependency on this okena checkout; a library of your own
depends on `okena-extension-api` by git (see `examples/extension-template`).
See `docs/reference/extensions.md`.
