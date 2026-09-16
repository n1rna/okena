---
id: 0005
title: A project map is agent-written docs plus one validated manifest in the repo's knowledge root
status: accepted
date: 2026-09-13
---

# 0005 — A project map is agent-written docs plus one validated manifest in the repo's knowledge root

## Context

okena knows where a project is on disk, and its branch, diff, PR and pipeline.
It knows nothing about what the project *is*. Knowledge stores
([ADR-0003](0003-knowledge-stores.md)) hold organisation-wide docs, but nothing
describes one repository's modules, concepts, boundaries or CI/infra, or which
other projects it talks to. So every agent re-learns a repository from scratch,
and there is no way to see how projects fit together.

Much of this is already written down, but scattered across `CLAUDE.md`,
`AGENTS.md`, READMEs, CI config and compose files. Later work (QBL-366 to
QBL-369) scans a repository with an agent, shows the result in the project info
panel, and links projects by what one exposes and another consumes. All of it
needs one format, fixed first.

This ADR extends ADR-0003. It does not change it.

## Decision

**The map is written by an agent and committed in the repository.** It lives
in the repository's knowledge root (`.okena/knowledge/`, or `root:` in
`.okena/knowledge.yaml`). Everyone with a clone has it, it is reviewed like any
other change, and okena still never writes to checkouts.

**Prose for reading, one manifest for parsing.**

- **Docs:** `docs/project/*.md` in the knowledge root, ordinary knowledge docs
  that list and render with no extra work.
- **Manifest:** `project-map.yaml` at the top of the knowledge root, outside
  the kind folders, so the tree never lists it.
- **What okena reads:** the manifest only. Anything okena shows, and link
  matching, come from it and never from the docs.

**The manifest is versioned and validated.**

- **Schema:** `version: 1`, `project`, `scanned?`, `areas`, `concepts`,
  `exposes`, `consumes`, `ci`, `infrastructure`.
- **Rules:** required fields, kebab-case ids unique per list, area references
  that resolve, paths inside the repository, docs under `docs/`.
- **Unknown keys:** ignored, as in `store.yaml`.
- **Newer version:** refused.
- **Problems:** every problem is reported with a code and a fix. A broken
  manifest is a status to show, not an error.

**Interfaces are typed and matched by type and name.** `type` is a closed set:
`http`, `grpc`, `graphql`, `package`, `queue`, `topic`, `database`, `infra`.
`name` is the identifier the provider publishes.

**Staleness is recorded, but optional.** `scanned: { commit, at }` says which
commit a map describes. Without it, staleness is unknown, and the map is still
valid.

**What to extract is a skill, not code.** okena ships a default Agent Skill at
`skills/project-map/SKILL.md`. It is compiled in, written into
`okena-defaults`, and resolved from the store named by
`harness.knowledge.prompts` before the built-in, like launch templates. okena
owns only the manifest schema.

> **Superseded in part (QBL-415).** The skill still resolves before the
> built-in, but not from one configured store: `harness.knowledge.prompts` is
> gone, and it resolves across every knowledge root in discovery order. See
> [knowledge.md](../reference/knowledge.md#resolution).

## Consequences

- **What okena needs to read:** a project's map needs only its checkout, so a
  remote project's map can go through the same daemon actions as a local one.
- **Changing the extraction:** a team edits a skill file, not okena.
- **Stable identifiers:** matching depends on both sides writing the same
  `type` and `name`. The skill spells out a naming convention per type, and a
  mismatch shows up as an unmatched `consumes`, not a wrong link.
- **Closed interface types:** a new type needs an okena release, and a
  manifest using it is invalid in older okena until then.
- **Additive changes:** ignoring unknown keys means a new optional field
  needs no version bump. The cost is that a misspelled optional key is
  silently ignored, not reported.
- **Where multi-project links are written:** still open. It belongs to QBL-368
  and is not fixed by this schema.

## Alternatives considered

- **Manifest in `.okena/` at the repository root**, beside `knowledge.yaml.`
  Keeping it next to its docs in the knowledge root means one folder holds the
  whole map, and a repository that moves its knowledge root moves both.
- **Frontmatter on the docs instead of a manifest.** Every fact okena matches
  on would be spread over several files and mixed with prose, and a
  half-edited doc could break the links.
- **okena extracts the map itself** by parsing code and config. That needs
  per-language analysis okena doesn't have, and it would miss what only a
  reader can judge, such as boundaries and concepts. The agent already reads
  code.
- **Rejecting unknown keys.** This catches typos, but it forces a version bump
  for every added optional field, and older okena then refuses the whole map.
- **A required `scanned` block.** A hand-written or partially migrated map
  would be invalid until it named a commit, for information that only drives a
  hint.
