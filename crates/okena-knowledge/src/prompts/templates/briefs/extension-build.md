---
name: extension-build
description: Brief an agent to build an okena extension from a summary
for: extension-build
model: opus
---
Build an okena extension.{summary}

An extension is a WASM component okena's daemon runs: it returns a declarative view okena draws natively, and can offer actions, queries and agent launches. Before you write any of it:

- Read `docs/reference/extensions.md` in okena's own repository. It is the reference for the manifest, the permissions, the sandbox, the host calls, what the extension exports, and the dev loop. If you cannot find a checkout of okena, ask me where it is rather than guessing at the API.
- Start from `examples/extension-template` there: copy that folder into a folder of its own under my working directory, then rename `id`, `name` and `description` in `extension.toml` and the crate in `Cargo.toml`.
- Read `examples/extension-library` for worked examples of a view, actions and queries.

Build it for `wasm32-wasip2` — `cargo build --release --target wasm32-wasip2`, which the template's `rust-toolchain.toml` installs the target for — and keep going until it compiles.

Finish with an extension folder that is ready to install: an `extension.toml` beside the crate, only the permissions it actually needs, and a component that builds. Do not install it — installing means approving its permissions, which is mine to do. Tell me where the folder is and to install it from Settings → Extensions → Install an extension → From a local folder.

Ask me about anything the summary does not settle rather than inventing it.{projects}{context}

{>context-lookup}

{>reporting}
