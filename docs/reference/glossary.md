# Glossary

Domain terms used across Okena. Implementation lives in code; this is for shared language.

## Space

A named, separate set of projects, agents, tasks and Knowledge/Specs roots inside one profile. Switched from the selector at the top of the sidebar; there is always a **Default** space, which cannot be renamed or deleted. See [`spaces.md`](spaces.md).

Not a **Workspace** — see below. A space is the switch *above* the workspace: each one has its own projects and folders. Profiles separate whole config directories; a space separates what is inside one.

## Workspace

The unit of persisted state: all known projects, folders, and their layouts. One workspace per install (file: `workspace.json`). Each project and folder in it records the **space** it belongs to; a sidebar shows one space's at a time.

## Project

A directory tracked by Okena. Displayed as a vertical **project column** containing the terminal tree for that directory. Identified by a stable `id`. May be local or remote.

## Worktree (project)

A project flagged as a git worktree of a **parent project**. Renders as a child column grouped under its parent. Created from the parent's git repo.

## Folder

A user-defined grouping of projects in the sidebar. Has its own ordering of child projects. Pure UX organization — does not affect persistence ownership. A folder belongs to one **space**, and groups only that space's projects.

## Connection (task backend)

One named login to a task manager — a Linear account, an Azure DevOps organization. okena holds any number of them, of either kind, and each **space** reads exactly one. Managed in Settings → Tasks; the credential lives in `tasks_credentials.json`, keyed by connection id.

## Layout

The split/tabs/terminals tree inside a single project column. Lives on `ProjectData.layout`.

## Window

A viewport onto the Workspace. Each window owns its own **view state**:

- folder filter (which folder is selected, if any)
- hidden project set (per-window show/hide overrides)
- focus zoom (which project, if any, is in single-project mode)
- column widths in the projects grid
- folder collapsed states in the sidebar
- OS bounds

Windows are not user-named; they are addressed positionally (main, then auto-numbered extras).

The underlying Workspace (projects, folders, ordering, layouts, hooks) is shared across windows. Sidebar renders in every window but reflects that window's filter state. Closing a window does not remove anything from the Workspace. Terminals, PTYs, git watchers, and settings are shared across all windows.

When a project is added from a given window, it becomes visible only in that window — hidden by default in all others.
