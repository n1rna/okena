---
id: 0007
title: Extensions from git run as sandboxed WASM in the daemon and return declarative views
status: accepted
date: 2026-09-18
---

# 0007 — Extensions from git run as sandboxed WASM in the daemon and return declarative views

## Context

okena's extensions (claude, codex, github, updater) are crates compiled into the
binary, registered by hand, and running in the GPUI app — so remote and mobile
clients see none of them, and a team cannot add its own tooling (a work-items
table over DynamoDB, a tenant-config browser over a git repo) without forking
okena. Such tools need different CLIs and configuration, must be installable
from a team's own repository, and must stay native (no web views). They run code
someone else wrote on the user's machine, so what they may touch has to be
visible up front and enforced.

## Decision

We will run extensions installed from git as WebAssembly components
(wasmtime, component model, `okena:extension` WIT) **in the daemon**.

- An extension reaches the machine only through host calls — run a declared
  command, read a declared path, its own key-value store, its configuration,
  logging — each checked against the permissions the user approved at install.
  It gets no WASI filesystem, environment or network of its own, and a compute
  and memory budget per call; a trap fails that call, not the daemon.
- It returns a **declarative view** (tables, trees, detail panes, charts, stats,
  text, badges, actions) that okena draws natively in each client. Extensions
  never get GPUI views. Components are versioned per WIT interface so new ones
  can be added without breaking installed extensions.
- Install is from a git URL, ref and folder (many extensions per library repo),
  pinned to the resolved commit and updated by hand, using a prebuilt
  `extension.wasm` when the ref has one and building from source otherwise.
  Updates that ask for more permissions need approval again.
- Actions can launch native agent sessions, either prefilling okena's launcher
  or (with a `start_agents` permission) starting at once; those sessions can call
  back into the extension through okena's MCP, with destructive actions held
  for the user's confirmation.
- The SDK is Rust only for now (`okena-extension-api`).

## Consequences

- Remote, web-capable and mobile clients get extensions through the snapshot
  with no extension code on the client; the GPUI client is the only renderer
  today.
- The daemon grows a wasmtime/cranelift dependency (binary size, build time).
- Permissions are per program and per path: approving a CLI approves what that
  CLI can do with the user's credentials. The approval card says so plainly.
- What an extension can draw is limited to okena's component set; new needs mean
  a new `ui-vN` interface, not arbitrary UI.
- Building from source needs Rust and the `wasm32-wasip2` target on the daemon's
  machine; prebuilt components avoid it.
- The built-in extensions keep their own in-process mechanism until ported.

## Alternatives considered

- **Dynamic libraries (dylibs) or rebuilding okena with extra crates.** Native
  speed and full GPUI access, but no sandbox, ABI breakage on every okena
  release, and per-platform builds; a crash takes the process down.
- **Web views / embedded browser.** Arbitrary UI, but explicitly not wanted
  (non-native look, heavy runtime, no mobile/remote story through the snapshot).
- **Running extensions in the GUI process.** Simpler wiring, but remote and
  mobile clients would never see them — the problem this set out to fix.
- **Scripts (shell/Lua/JS) instead of WASM.** Easy to write, but no real
  sandbox for a script runtime we would embed and maintain, and no typed
  interface to version.
