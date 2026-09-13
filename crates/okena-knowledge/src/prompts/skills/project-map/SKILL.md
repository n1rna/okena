---
name: project-map
description: Map a repository for people and agents — its areas, concepts, what it exposes and consumes, and how it is built and run — as docs plus a project-map.yaml that okena reads.
---
# Project map

Use this skill to write or update a repository's project map. A map is two things, committed with the repository:

- a short set of docs describing what the repository is, for people and agents to read;
- one manifest, `project-map.yaml`, that okena parses to show the project and to connect it to the projects it talks to.

Your output is reviewed like any other change. Do not commit or push unless you are asked to.

## Where it goes

The map lives in the repository's knowledge root:

- the folder `.okena/knowledge.yaml` names with `root:`, relative to the repository root, when that file sets one;
- otherwise `.okena/knowledge/`. Create it if it does not exist.

```text
<knowledge root>/
├── project-map.yaml
└── docs/project/
    ├── overview.md
    ├── areas.md
    ├── areas/<area-id>.md     only for an area too large for areas.md
    ├── concepts.md
    └── delivery.md            CI/CD and infrastructure
```

Leave everything else in the knowledge root as it is.

## Where to start

Check what already exists, in this order, and say which case applied when you finish.

1. **A map exists** (`project-map.yaml` is there). Bring it up to date rather than rebuilding it. Read the manifest and the docs, compare them with the code as it is now, and edit them in place: add what is new, correct what changed, remove what is gone. Keep the ids of areas and concepts that still describe the same thing, because links and references point at them. Never start a second map beside the first.
2. **No map, but the repository describes itself**: `CLAUDE.md`, `AGENTS.md`, `README.md`, a `docs/` folder, architecture notes, decision records. Read those first and use them as your starting point. Check each claim against the code, because such docs drift. Link to them from the map docs instead of copying them.
3. **Neither.** Work it out from the code: the directory layout, build manifests (`Cargo.toml`, `package.json`, `go.mod`, `pyproject.toml` and the like), entry points, route and schema definitions, `.proto` and OpenAPI files, CI config, Dockerfiles, compose files and deployment manifests.

In every case, CI config, compose files and deployment manifests are the evidence for the delivery doc and for most of what the project consumes.

## What to extract

- **Overview.** What the project is for and who uses it, plus what someone needs before touching it: language and framework, how to run it locally, how to test it.
- **Areas.** The parts the code is divided into — modules, packages, services, crates, apps — each with the paths that belong to it and a sentence on what it owns. Areas are boundaries: say what each depends on and what must not reach into it. Aim for the level someone would name in a task ("the billing module"), usually 3 to 15 areas, not every folder.
- **Concepts and features.** The domain ideas and user-facing features the code implements (an invoice, a tenant, sync, export), each tied to the areas that implement it.
- **Exposes.** What other projects can use from this one: HTTP, gRPC and GraphQL APIs it serves, packages it publishes, queues and topics it publishes to, databases it owns that others use, infrastructure it provides.
- **Consumes.** What this project uses from elsewhere: APIs it calls, your organisation's own packages it depends on (not every third-party library), queues and topics it reads, databases it uses but does not own, infrastructure it runs on.
- **CI/CD and infrastructure.** Its pipelines and what triggers them, how and where it is deployed, and the resources it needs to run, each with the files that define it.

Only record what you found evidence for. When something is ambiguous — which of two services owns a database, whether a folder is its own area — ask rather than guess.

## The docs

Ordinary Markdown, each with frontmatter: a `title`, a one-line `description` (what people and agents pick a doc by) and `tags`. Keep them short and specific, and link to source paths and existing docs rather than restating them.

- `overview.md` — the overview.
- `areas.md` — a section per area, headed with its id: its paths, what it owns, what it depends on, and its boundary rules. An area too large for one section gets its own `areas/<area-id>.md`.
- `concepts.md` — a section per concept or feature, naming the areas that implement it.
- `delivery.md` — pipelines, deployment and infrastructure.

## The manifest

`project-map.yaml` holds the same map as facts. okena shows the project from it and links projects by matching one project's `consumes` against another's `exposes`, so it has to be exact. okena never reads the docs for any of this.

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
  - id: http
    name: HTTP layer
    description: Routes, request validation and auth middleware.
    paths: [src/http/**]
  - id: billing
    name: Billing
    description: Invoices, payment runs and the jobs that send them.
    paths: [src/billing/**, migrations/billing/**]
    doc: docs/project/areas/billing.md
concepts:
  - id: invoice
    name: Invoice
    description: A bill for one tenant's usage in one period.
    areas: [billing, http]
exposes:
  - type: http
    name: api.acme.com/v1
    description: Public REST API.
    areas: [http]
  - type: topic
    name: billing.invoice-issued
    areas: [billing]
consumes:
  - type: grpc
    name: acme.accounts.v1.AccountService
    description: Looks up tenants.
    areas: [http, billing]
  - type: package
    name: "@acme/money"
ci:
  - name: test
    provider: github-actions
    description: Lint and tests on every pull request.
    files: [.github/workflows/test.yml]
infrastructure:
  - name: postgres
    kind: database
    description: Primary database, owned by this project.
    files: [docker-compose.yml]
links:
  - project: accounts
    direction: uses
    type: grpc
    name: acme.accounts.v1.AccountService
```

| Field | Required | What to write |
|---|---|---|
| `version` | yes | `1` |
| `project.name`, `project.description` | yes | The project's name, and what it is for in one line |
| `project.doc` | no | The overview doc |
| `scanned.commit`, `scanned.at` | when `scanned` is present | The commit you mapped (`git rev-parse HEAD`) and when, as an ISO 8601 time, both quoted. Write them every time you create or update the map. |
| `areas[].id` | yes | Kebab-case, unique among areas |
| `areas[].name` | no | A display name |
| `areas[].description` | yes | What the area owns, in one line |
| `areas[].paths` | yes, at least one | Paths or globs, relative to the repository root |
| `areas[].doc` | no | The area's own doc |
| `concepts[].id` | yes | Kebab-case, unique among concepts |
| `concepts[].name` | no | A display name |
| `concepts[].description` | yes | One line |
| `concepts[].areas` | yes, at least one | Ids of the areas that implement it |
| `concepts[].doc` | no | Its doc |
| `exposes[]`, `consumes[]` `.type` | yes | One of the types below |
| `exposes[]`, `consumes[]` `.name` | yes | The identifier, as below |
| `exposes[]`, `consumes[]` `.description` | no | One line |
| `exposes[]`, `consumes[]` `.areas` | no | Ids of the areas that serve or use it |
| `ci[].name` | yes | The pipeline's name |
| `ci[].provider` | no | e.g. `github-actions`, `gitlab-ci` |
| `ci[].description` | no | What it does and what triggers it |
| `ci[].files` | yes, at least one | The files that define it |
| `infrastructure[].name` | yes | The resource's name |
| `infrastructure[].kind` | no | e.g. `database`, `cache`, `bucket`, `cluster` |
| `infrastructure[].description` | no | One line |
| `infrastructure[].files` | no | The files that define or configure it |
| `links[].project` | yes | The other project, by the `project.name` of its map |
| `links[].direction` | yes | `uses` when this project uses the other, `used_by` when the other uses this one |
| `links[].type`, `links[].name` | yes | The interface the link runs through, named as under `exposes` and `consumes` |
| `links[].description` | no | One line |

Every `doc` is a path relative to the knowledge root, to a `.md` file under `docs/`. Every other path is relative to the repository root and never uses `..`.

### Interface types and names

A link is found only when both projects write the same `type` and `name`. Use the name the *provider* publishes, never a local alias or an environment variable:

| `type` | `name` |
|---|---|
| `http` | Host and base path, without the scheme: `api.acme.com/v1`. For an internal service with no public host, its service name as deployed plus the base path: `billing-api/v1`. |
| `grpc` | The fully qualified service from the `.proto`: `acme.accounts.v1.AccountService` |
| `graphql` | Host and path of the endpoint: `api.acme.com/graphql` |
| `package` | The name in its registry: `@acme/money` |
| `queue` | The queue's name as the broker knows it |
| `topic` | The topic's or exchange's name as the broker knows it |
| `database` | The logical database name: `billing` |
| `infra` | A shared resource, by the name it is provisioned under: `acme-prod-cluster` |

List each interface once in a list, with every area that uses it in its `areas`.

### Links

`links` records this project's connections to other projects, and every link is written in both projects' maps: `direction: uses` in the map of the project that uses the other, `direction: used_by` in the other's, with the same `type` and `name` in both. `project` names the other project by the `project.name` of its map.

Only add a link when you have seen both ends: the call, dependency or subscription in one repository, and what serves it in the other. When you are mapping a single repository you usually cannot, so leave `links` alone: okena already connects projects whose `consumes` and `exposes` match. A scan over several repositories is where links get written.

## Before you finish

- Every area and concept id is kebab-case and unique in its list, and every `areas` entry names an area.
- Every path is relative, and every `doc` points at a doc you wrote under `docs/`.
- `scanned.commit` is the commit you mapped.
- Every link you wrote is in both projects' maps, with matching `type` and `name`.
- The docs and the manifest say the same things.

Then summarise what you mapped, which starting case applied, and anything you were unsure of.
