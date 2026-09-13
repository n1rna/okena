---
id: 0006
title: Links between projects are written into both projects' maps
status: accepted
date: 2026-09-13
---

# 0006 — Links between projects are written into both projects' maps

## Context

[ADR-0005](0005-project-maps.md) puts each repository's map in that
repository, committed, and leaves one question open: where a link between two
projects is written. A link belongs to two repositories. okena can match most
links itself, one project's `consumes` against another's `exposes`, but a
multi-project scan finds links that the manifests' names do not line up on.
Those need to be recorded somewhere.

The options were:

- both repositories' maps;
- only the consuming repository's map;
- a shared knowledge store above the projects.

## Decision

**A link is written into both projects' maps, and nowhere else.** A map gains a
`links` list. Each entry names:

- the other project, by its map's `project.name`
- a `direction`, `uses` or `used_by`, seen from this project
- the interface, as `type` plus `name`

The multi-project scan writes the `uses` entry in one map and the matching
`used_by` entry in the other.

**okena reads both sources together.** Links matched from `exposes` and
`consumes` are merged with listed links, and each link carries its source:

- *matched:* from the interfaces only
- *confirmed:* matched and listed
- *found by scan:* listed only

A link listed on only one side is reported as such.

**The manifest stays `version: 1`.** `links` is an optional list, and okena
already ignores unknown keys, so an older okena reads a map with links and
skips them.

## Consequences

- **Reading one project:** its map says what it is connected to, without
  opening any other repository. Anyone with that one clone sees its links.
- **Working on several projects:** the same entries, read across maps, give the
  graph. The cross-project view needs no store of its own.
- **Drift:** every link is two edits in two repositories, which can drift or be
  committed on one side only. okena flags a one-sided link rather than hiding
  it.
- **Renames:** links refer to projects by map name, so renaming a project's
  `project.name` breaks the links pointing at it. Those show as unresolved.
- **Checkouts:** a link to a project that is not checked out and scanned on this
  machine cannot be resolved there.

## Alternatives considered

- **Only the consumer's map.** One edit per link, but the provider's map could
  not say who uses it without reading every other repository. The provider's
  owners are the ones who most need that when changing an interface.
- **A shared knowledge store.** One place for the graph, but it is invisible
  from the repository alone, needs a store every team can write to, and adds a
  third thing to keep in step with two codebases.
- **Matching only, no listed links.** No extra writing, but it misses every link
  the two manifests name differently. Those are the links a multi-project scan
  is for.
