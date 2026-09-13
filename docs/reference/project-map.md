# Project maps

A **project map** describes one repository: the areas its code is divided into,
the concepts and features those areas implement, what the repository exposes to
other projects and consumes from them, and how it is built and run. It has two
halves, both committed in the repository:

- **Docs** under `docs/project/`, for people and agents to read.
- **One manifest**, `project-map.yaml`, which okena parses.

An agent writes the map by following okena's [`project-map` skill](#the-project-map-skill).
okena reads the map and never writes it
([ADR-0005](../decisions/0005-project-maps.md)).

## Layout

The map lives in the repository's knowledge root: the folder
`.okena/knowledge.yaml` names with `root:`, else `.okena/knowledge/` (see
[Projects](knowledge.md#projects)).

```text
<knowledge root>/
├── project-map.yaml           the manifest
└── docs/project/
    ├── overview.md            what the project is, how to run and test it
    ├── areas.md               one section per area
    ├── areas/<area-id>.md     an area too large for a section
    ├── concepts.md            one section per concept or feature
    └── delivery.md            CI/CD and infrastructure
```

The docs are ordinary knowledge docs, with `title`, `description` and `tags`
frontmatter, so they list in Harness → Knowledge. The manifest sits outside the
kind folders, so it is never listed as a doc.

## The manifest

```yaml
version: 1
project:
  name: api
  description: The public HTTP API and the billing jobs behind it.
  doc: docs/project/overview.md
scanned:
  commit: "3f9a2c1d8e"
  at: "2026-09-13T10:00:00Z"
areas:
  - id: billing
    name: Billing
    description: Invoices, payment runs and the jobs that send them.
    paths: [src/billing/**, migrations/billing/**]
    doc: docs/project/areas/billing.md
concepts:
  - id: invoice
    description: A bill for one tenant's usage in one period.
    areas: [billing]
exposes:
  - type: http
    name: api.acme.com/v1
    areas: [billing]
consumes:
  - type: package
    name: "@acme/money"
ci:
  - name: test
    provider: github-actions
    files: [.github/workflows/test.yml]
infrastructure:
  - name: postgres
    kind: database
    files: [docker-compose.yml]
```

### Fields

| Field | Required | Meaning |
|---|---|---|
| `version` | yes | The manifest format; `1` |
| `project.name` | yes | The project's name |
| `project.description` | yes | What it is for, in one line |
| `project.doc` | no | The overview doc |
| `scanned.commit` | when `scanned` is present | The commit the map describes: 7 to 40 hex digits |
| `scanned.at` | when `scanned` is present | When it was written, as the agent wrote it (ISO 8601) |
| `areas[].id` | yes | Kebab-case, unique among areas |
| `areas[].name` | no | Display name; the id when unset |
| `areas[].description` | yes | What the area owns |
| `areas[].paths` | yes, at least one | Paths or globs relative to the repository root |
| `areas[].doc` | no | The area's doc |
| `concepts[].id` | yes | Kebab-case, unique among concepts |
| `concepts[].name` | no | Display name; the id when unset |
| `concepts[].description` | yes | One line |
| `concepts[].areas` | yes, at least one | Ids of the areas that implement it |
| `concepts[].doc` | no | The concept's doc |
| `exposes[]`, `consumes[]` `.type` | yes | An [interface type](#interface-types) |
| `exposes[]`, `consumes[]` `.name` | yes | The identifier the provider publishes |
| `exposes[]`, `consumes[]` `.description` | no | One line |
| `exposes[]`, `consumes[]` `.areas` | no | Ids of the areas that serve or use it |
| `ci[].name` | yes | The pipeline's name |
| `ci[].provider` | no | e.g. `github-actions` |
| `ci[].description` | no | One line |
| `ci[].files` | yes, at least one | The files that define it |
| `infrastructure[].name` | yes | The resource's name |
| `infrastructure[].kind` | no | Free text, e.g. `database`, `cache`, `bucket` |
| `infrastructure[].description` | no | One line |
| `infrastructure[].files` | no | The files that define or configure it |

Paths:

- **Docs:** every `doc` is relative to the knowledge root and names a `.md`
  file under `docs/`.
- **Everything else:** relative to the repository root, and `..` is not allowed.

Reading:

- Unknown keys are ignored, so a manifest with fields a newer okena added still
  reads.
- A `version` newer than okena understands is refused.
- Without `scanned`, whether the map is stale is unknown. The map is still
  valid.

### Interface types

Two projects are linked when one's `consumes` and the other's `exposes` carry
the same `type` and `name`. The `name` is therefore the identifier the provider
publishes, never a local alias.

| `type` | `name` |
|---|---|
| `http` | Host and base path without the scheme (`api.acme.com/v1`); for an internal service, its deployed service name and base path (`billing-api/v1`) |
| `grpc` | Fully qualified service (`acme.accounts.v1.AccountService`) |
| `graphql` | Host and path of the endpoint (`api.acme.com/graphql`) |
| `package` | Registry name (`@acme/money`) |
| `queue` | Queue name as the broker knows it |
| `topic` | Topic or exchange name as the broker knows it |
| `database` | Logical database name (`billing`) |
| `infra` | A shared resource, by the name it is provisioned under |

The same `type` and `name` may appear only once in `exposes`, and once in
`consumes`.

## Map status

| Status | When |
|---|---|
| Not scanned | The project has no knowledge root, or its root has no `project-map.yaml` |
| Scanned | The manifest parses and passes every rule below |
| Invalid | The manifest exists and cannot be used; every problem found is reported, each with a fix |

An invalid `.okena/knowledge.yaml` also makes the map invalid, since the map's
location cannot be known.

### Problems

| Code | Meaning |
|---|---|
| `project_map_unreadable` | The manifest could not be read, or is not a file |
| `project_map_outside` | The manifest is a symlink that resolves outside the knowledge root |
| `project_map_too_large` | The manifest is over 1 MiB |
| `project_map_invalid` | Not YAML, not a mapping, no `version`, a missing required field, or an unknown `type` |
| `project_map_version` | A `version` newer than okena reads |
| `project_map_missing_field` | A required text is blank or a required list is empty |
| `project_map_invalid_id` | An area or concept id is not kebab-case |
| `project_map_duplicate_id` | Two areas, or two concepts, share an id |
| `project_map_unknown_area` | An `areas` entry names no area |
| `project_map_path_outside` | A path is absolute or uses `..` |
| `project_map_doc_path` | A `doc` is not a `.md` file under `docs/` |
| `project_map_invalid_commit` | `scanned.commit` is not 7 to 40 hex digits |
| `project_map_duplicate_interface` | The same `type` and `name` appears twice in one list |

## Scanning

**Scan** in a repository's project info panel opens an agent session in the
repository, briefed from the `project-scan` template
([launch prompts](knowledge.md#flows)) and pointed at the `project-map` skill
by its absolute path.

- **Repositories only:** the panel offers Scan on a repository, never on a
  worktree, and the daemon refuses a worktree or an agent session.
- **Where it writes:** the repository's knowledge root, `root:` in
  `.okena/knowledge.yaml`, else `.okena/knowledge/`, which the agent creates if
  needed. A `.okena/knowledge.yaml` that is invalid, or whose `root:` leaves the
  repository, refuses the scan.
- **Label:** **Rescan** once a manifest exists, valid or not; **Scan** before.
- **Committing:** nothing. The map is left in the checkout to review.
- **Agent:** the one you pick, else `harness.agent_command`. Without an agent
  it refuses.

okena checks which starting point applies, and the brief says it through a
partial:

| Case | When | Partial |
|---|---|---|
| Update | The manifest is valid | `scan-update` |
| Repair | The manifest is invalid; its problems are listed | `scan-repair` |
| From docs | No manifest; `CLAUDE.md`, `AGENTS.md` or `docs/` is at the repository root | `scan-from-docs` |
| From code | None of the above | `scan-from-code` |

## In the info panel

A repository's project info panel has a **MAP** section; a worktree's does
not.

- **Status:** Not scanned, Scanned (with the short commit when the map records
  one), or Invalid with the first problem and its fix.
- **Scan / Rescan:** the launcher described under [Scanning](#scanning).
- **Contents**, from the manifest only:
  - the project's name and description;
  - **Areas**, with their paths;
  - **Concepts**, with the areas that implement them;
  - **Exposes** and **Consumes**, grouped by type, with the areas that serve or
    use each;
  - **CI/CD** pipelines, with provider and files;
  - **Infrastructure**, with kind and files.
- **Docs:** the project, an area or a concept with a `doc` opens that doc in
  Harness → Knowledge, in the repository's knowledge root.
- **Refresh:** an open panel reads the map again every 3 seconds, so a scan's
  result shows up without reopening it.

The read runs in the daemon and replies with the map's state and the key of the
knowledge root it came from, so a remote project's panel reads the same way as
a local one.

## The `project-map` skill

What a map contains, and how an agent finds it, is prose in an Agent Skill
rather than code. A team can change it without an okena release.

- **Built in:** okena ships the skill at `skills/project-map/SKILL.md` and
  writes it into the [`okena-defaults` store](knowledge.md#the-okena-defaults-store)
  with the templates.
- **Resolution:** the store named by `harness.knowledge.prompts`, when it has
  `skills/project-map/SKILL.md`, else the built-in, the same way
  [launch prompts resolve](knowledge.md#resolution). A scan names the copy it
  resolved: the store's file, or the one in `okena-defaults`.
- **Override:** the whole file is replaced, frontmatter included. An empty file
  falls back to the built-in.
- **Not rendered:** unlike a template, a skill has no `{placeholder}`s.

The skill tells the agent:

- **Starting points:** update an existing map in place. With no map, start
  from `CLAUDE.md`, `AGENTS.md`, READMEs and `docs/`. With neither, work from
  the code.
- **Output:** the layout and manifest above.
- **Committing:** not unless asked.

## Limits

| Limit | Value |
|---|---|
| Largest manifest read | 1 MiB |
