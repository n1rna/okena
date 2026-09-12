---
id: 0004
title: okena commits and pushes store checkouts through one shared store-git module
status: accepted
date: 2026-09-12
---

# 0004 — okena commits and pushes store checkouts through one shared store-git module

## Context

ADR-0003 kept okena to reading knowledge checkouts and fast-forwarding them:
"Commit and push are left to the user or an agent in a terminal." OpenSpec
roots had no sync surface at all — `okena-openspec`'s `git.rs` only reads the
origin URL and makes a store's initial commit.

Editing documents in the harness (QBL-361) breaks that arrangement. A save
leaves the checkout dirty, and a dirty or ahead checkout cannot be
fast-forwarded. Editing in okena would then leave a store that quietly stops
syncing until someone opens a terminal.

Both store types need the same git: a status that names the changed files,
fetch, fast-forward pull, commit and push. Neither `okena-knowledge` nor
`okena-openspec` should own the other's copy.

## Decision

**okena commits and pushes a store checkout when a person asks** from the Specs
or Knowledge view. Nothing is committed automatically.

**The store git lives in `okena_git::store`**, over the `git` binary and the
non-interactive network environment `okena-git` already uses.

- The wire shape is `okena_core::store_git::StoreGitStatus`.
  `KnowledgeGitStatus` is an alias for it, and `SpecRoot.git` carries it too.
- `okena-knowledge` wraps its errors into `KnowledgeError`, keeping the codes.
- The specs executor in `okena-app-core` calls it directly, so
  `okena-openspec` stays a CLI-compatible filesystem crate.

**Status lists the changed files.** It runs `git status --porcelain=v1 -z
--untracked-files=all --no-renames` and caps the list at 1 000. A status git
cannot produce still reads as dirty.

**A commit names its files.** The actions take explicit `paths` and a
`message`.

- The daemon accepts only paths its own status lists as changed and not
  conflicted. A client cannot commit a file it was not shown.
- Paths are committed with literal pathspecs.
- Anything else already staged stays out of the commit and stays staged.

**Push is its own action**, to the upstream's remote and branch
(`HEAD:<upstream ref>`). A failed push leaves the commit in place.

**Pull also refuses a dirty checkout** (`uncommitted_changes`). The view states
why Pull or Push is unavailable, using the same rules the daemon enforces
(`pull_blocker`, `push_blocker`).

**Only store and folder roots sync.** A project root stays with its project's
own git, as in ADR-0003.

## Consequences

- An edit made in okena, or by an agent in a terminal, shows up as changed
  files and can be committed and pushed from the view that shows it. A remote
  client does exactly the same, because every step goes through the daemon.
- This supersedes ADR-0003's "Commit and push are left to the user or an agent
  in a terminal", and its "pushes stay out of okena's UI". Merge conflicts and
  rebases still stay out: a conflicted file is refused, and a diverged branch
  gets a pointer to a terminal.
- Pull with unrelated local edits used to succeed when git allowed it. It is
  now refused until the edits are committed or stashed, so a pull never lands
  on top of unsaved work.
- `SpecStores` now runs `git status` in every store. The daemon therefore runs
  it off the workspace lock, as it already did for `KnowledgeStores`.
- The commit actions are reachable by any paired client and by agents over
  okena's MCP server. The changed-paths guard is what keeps them from becoming
  a way to commit arbitrary files.

## Alternatives considered

- **Commit logic in `okena-knowledge` or in `okena-openspec`.** Either one would
  make one store crate depend on the other for git it has no business owning.
- **Commit and push as one action.** A push error in a commit's reply reads as
  a failed commit, and the reply would need two outcomes. Separate actions keep
  each reply a single sync state, with the commit safely made before the
  network is touched.
- **Commit everything** (`git add -A`). This would sweep in files the person
  never saw, including whatever an agent had staged for its own commit.
