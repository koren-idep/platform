import type { KorenClient } from '../services/client';
import type { ProjectSummary, TaskSummary } from '../services/protocol';

export const PROJECT_DISCOVERY_LIMITS = Object.freeze({
  max_directories: 6_000,
  max_projects: 256,
  max_depth: 32,
});

export interface ProjectDiscoverySnapshot { projects: ProjectSummary[]; tasks: TaskSummary[] }

export async function openWorkspaceAndDiscoverProjects(
  client: Pick<KorenClient, 'call'>,
  path: string,
): Promise<{ root: string; discovery: ProjectDiscoverySnapshot | null; discoveryError: string | null }> {
  const opened = await client.call({ method: 'open_workspace', params: { path } });
  if (opened.kind !== 'workspace_opened') throw new Error(`Unexpected workspace response: ${opened.kind}`);
  try {
    return { root: opened.value.root, discovery: await discoverWorkspaceProjects(client), discoveryError: null };
  } catch (error) {
    return { root: opened.value.root, discovery: null, discoveryError: error instanceof Error ? error.message : String(error) };
  }
}

export async function discoverWorkspaceProjects(client: Pick<KorenClient, 'call'>): Promise<ProjectDiscoverySnapshot> {
  const discovery = await client.call({ method: 'discover_registered_projects', params: PROJECT_DISCOVERY_LIMITS });
  if (discovery.kind !== 'projects') throw new Error(`Unexpected project discovery response: ${discovery.kind}`);
  const [projects, tasks] = await Promise.all([
    client.call({ method: 'list_projects' }),
    client.call({ method: 'list_tasks' }),
  ]);
  if (projects.kind !== 'projects') throw new Error(`Unexpected project list response: ${projects.kind}`);
  if (tasks.kind !== 'tasks') throw new Error(`Unexpected task list response: ${tasks.kind}`);
  return { projects: projects.value, tasks: tasks.value };
}
