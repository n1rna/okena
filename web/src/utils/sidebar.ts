import type { ApiFolder, ApiProject, StateResponse } from "../api/types";

export type SidebarProjectNode = {
  type: "project";
  project: ApiProject;
  worktrees: ApiProject[];
};

export type SidebarFolderNode = {
  type: "folder";
  folder: ApiFolder;
  projects: SidebarProjectNode[];
};

export type SidebarItem = SidebarProjectNode | SidebarFolderNode;

/** The space an item with none recorded belongs to. */
export const DEFAULT_SPACE_ID = "default";

/** Which space is showing, with the pre-spaces fallback applied. */
export function activeSpaceId(workspace: StateResponse | null): string {
  return workspace?.active_space || DEFAULT_SPACE_ID;
}

/** Whether `id` names `space`, treating a missing space as Default. */
function inSpace(space: string, id: string | undefined): boolean {
  return (id || DEFAULT_SPACE_ID) === space;
}

export function buildSidebarItems(workspace: StateResponse | null): SidebarItem[] {
  if (!workspace) return [];

  // Only the active space's rows. `projects` carries every space's, so this is
  // not a filter the user applied — a project in another space does not exist
  // here at all.
  const space = activeSpaceId(workspace);
  const visibleProjects = workspace.projects.filter((project) => inSpace(space, project.space_id));
  const visibleFolders = (workspace.folders ?? []).filter((folder) =>
    inSpace(space, folder.space_id),
  );

  const projectsById = new Map(visibleProjects.map((project) => [project.id, project]));
  const foldersById = new Map(visibleFolders.map((folder) => [folder.id, folder]));
  const folderProjectIds = new Set(visibleFolders.flatMap((folder) => folder.project_ids));
  const worktreeIds = new Set<string>();

  for (const project of visibleProjects) {
    for (const id of project.worktree_ids ?? []) {
      worktreeIds.add(id);
    }
    if (project.worktree_info) {
      worktreeIds.add(project.id);
    }
  }

  const toProjectNode = (project: ApiProject): SidebarProjectNode => ({
    type: "project",
    project,
    worktrees: (project.worktree_ids ?? [])
      .map((id) => projectsById.get(id))
      .filter((child): child is ApiProject => Boolean(child)),
  });

  const topLevelProject = (id: string): SidebarProjectNode | null => {
    const project = projectsById.get(id);
    if (!project || worktreeIds.has(project.id)) return null;
    return toProjectNode(project);
  };

  const items: SidebarItem[] = [];
  const consumed = new Set<string>();
  const order = workspace.project_order?.length
    ? workspace.project_order
    : [
        ...visibleFolders.map((folder) => folder.id),
        ...visibleProjects.map((project) => project.id),
      ];

  for (const id of order) {
    const folder = foldersById.get(id);
    if (folder) {
      const projects = folder.project_ids
        .map(topLevelProject)
        .filter((project): project is SidebarProjectNode => Boolean(project));
      items.push({ type: "folder", folder, projects });
      consumed.add(folder.id);
      for (const project of projects) {
        consumed.add(project.project.id);
      }
      continue;
    }

    const project = topLevelProject(id);
    if (project) {
      items.push(project);
      consumed.add(project.project.id);
    }
  }

  const unorderedProjects = visibleProjects
    .filter((project) => !consumed.has(project.id))
    .filter((project) => !folderProjectIds.has(project.id))
    .filter((project) => !worktreeIds.has(project.id))
    .sort(compareProjectsByActivity)
    .map(toProjectNode);

  return [...items, ...unorderedProjects];
}

function compareProjectsByActivity(a: ApiProject, b: ApiProject): number {
  if (Boolean(a.pinned) !== Boolean(b.pinned)) {
    return a.pinned ? -1 : 1;
  }
  const aActivity = a.last_activity_at ?? 0;
  const bActivity = b.last_activity_at ?? 0;
  if (aActivity !== bActivity) {
    return bActivity - aActivity;
  }
  return a.name.localeCompare(b.name);
}
