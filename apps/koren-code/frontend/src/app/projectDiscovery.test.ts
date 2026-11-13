import { describe, expect, it, vi } from 'vitest';
import { discoverWorkspaceProjects, openWorkspaceAndDiscoverProjects, PROJECT_DISCOVERY_LIMITS } from './projectDiscovery';

describe('workspace project discovery', () => {
  it('performs a bounded provider scan, then reloads detected projects and tasks', async () => {
    const call = vi.fn()
      .mockResolvedValueOnce({ kind: 'projects', value: [] })
      .mockResolvedValueOnce({ kind: 'projects', value: [{ project_id: 1, display_name: 'sample', root: '.', provider_id: 'example', configuration_path: null, state: 'registered' }] })
      .mockResolvedValueOnce({ kind: 'tasks', value: [] });

    const result = await discoverWorkspaceProjects({ call });

    expect(call).toHaveBeenNthCalledWith(1, { method: 'discover_registered_projects', params: PROJECT_DISCOVERY_LIMITS });
    expect(call).toHaveBeenNthCalledWith(2, { method: 'list_projects' });
    expect(call).toHaveBeenNthCalledWith(3, { method: 'list_tasks' });
    expect(result.projects[0]?.provider_id).toBe('example');
    expect(result.tasks).toEqual([]);
    expect(PROJECT_DISCOVERY_LIMITS.max_directories).toBeLessThanOrEqual(10_000);
    expect(PROJECT_DISCOVERY_LIMITS.max_projects).toBeLessThanOrEqual(1_000);
    expect(PROJECT_DISCOVERY_LIMITS.max_depth).toBeLessThanOrEqual(32);
  });

  it('automatically discovers projects after opening a workspace', async () => {
    const call = vi.fn()
      .mockResolvedValueOnce({ kind: 'workspace_opened', value: { root: '/workspace/demo' } })
      .mockResolvedValueOnce({ kind: 'projects', value: [] })
      .mockResolvedValueOnce({ kind: 'projects', value: [{ project_id: 2, display_name: 'demo', root: '.', provider_id: 'example', configuration_path: null, state: 'registered' }] })
      .mockResolvedValueOnce({ kind: 'tasks', value: [] });

    const result = await openWorkspaceAndDiscoverProjects({ call }, '/workspace/demo');

    expect(call).toHaveBeenNthCalledWith(1, { method: 'open_workspace', params: { path: '/workspace/demo' } });
    expect(call).toHaveBeenNthCalledWith(2, { method: 'discover_registered_projects', params: PROJECT_DISCOVERY_LIMITS });
    expect(result.root).toBe('/workspace/demo');
    expect(result.discovery?.projects[0]?.display_name).toBe('demo');
    expect(result.discoveryError).toBeNull();
  });

  it('keeps the workspace open and returns a visible error when automatic discovery fails', async () => {
    const call = vi.fn()
      .mockResolvedValueOnce({ kind: 'workspace_opened', value: { root: '/workspace/demo' } })
      .mockRejectedValueOnce(new Error('project discovery failed: directory limit exceeded'));

    const result = await openWorkspaceAndDiscoverProjects({ call }, '/workspace/demo');

    expect(result.root).toBe('/workspace/demo');
    expect(result.discovery).toBeNull();
    expect(result.discoveryError).toContain('directory limit exceeded');
  });

  it('surfaces a failed provider scan for the caller to report', async () => {
    const call = vi.fn().mockRejectedValue(new Error('project discovery failed: traversal limit reached'));
    await expect(discoverWorkspaceProjects({ call })).rejects.toThrow('traversal limit reached');
  });
});
