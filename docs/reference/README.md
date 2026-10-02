# reference

How the system works **now** — architecture, conventions, runbooks. Flat files,
`kebab-case.md`.

Rules (see [`../CLAUDE.md`](../CLAUDE.md)): describe the current state only — no
status updates, no TODOs (file those in `../backlog/`), no design rationale (that's
a `../decisions/` ADR). Update reference in the **same change** that alters
behaviour.

<!-- index the reference docs here, one line each -->

- [`glossary.md`](glossary.md) — domain terms: space, workspace, project, worktree, folder, layout, window.
- [`spaces.md`](spaces.md) — spaces: the selector, what a switch changes, per-space task connections and filters, per-space Library origins, and what every client shows.
- [`configuration.md`](configuration.md) — settings file, keybindings, per-project config.
- [`hooks.md`](hooks.md) — lifecycle hooks: events, config shape, execution.
- [`services.md`](services.md) — Docker Compose integration and port detection.
- [`worktrees.md`](worktrees.md) — git worktree projects: create, close, parent linkage.
- [`extensions.md`](extensions.md) — extensions from git: manifest, permissions, dependency check, UI components, actions, agent launch, MCP, building, library layout.
- [`library.md`](library.md) — the harness Library: origins and their three types (knowledge, spec, freeform), the Library and Origins pages, freeform origins, the `library_*` actions.
- [`knowledge.md`](knowledge.md) — knowledge stores: layout, frontmatter, registry, project config, sync, launch prompts and context.
- [`specs.md`](specs.md) — spec origins: the OpenSpec layout, discovery, stores and the CLI's registry, new changes.
- [`project-map.md`](project-map.md) — project maps: docs layout, the `project-map.yaml` manifest, validation, the `project-map` skill.
- [`remote.md`](remote.md) — remote control server: pairing, HTTP/WS API, TLS.
- [`mobile.md`](mobile.md) — React Native mobile client architecture (uniffi over `okena-mobile-ffi`).
- [`testing.md`](testing.md) — repo-wide test-selection rules + the GPUI test harness setup.

For crate-level and module-level detail, read the `CLAUDE.md` next to the code
(`crates/*/CLAUDE.md`, `crates/okena-app/src/**/CLAUDE.md`, `web/`, `mobile/rn/`).
