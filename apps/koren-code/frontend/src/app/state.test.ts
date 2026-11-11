import { describe, expect, it } from 'vitest';
import { ProjectTreeState, TabState } from './state';
import { approvalChoice, conflictChoice } from './dialogs';
import { nextTheme, persistTheme, readTheme } from './theme';
import { taskStatus, visibleOutput } from './taskPresentation';

describe('project tree state', () => {
  it('loads children only when a directory is expanded', () => {
    const tree = new ProjectTreeState();
    tree.setEntries('', [{ name: 'src', path: 'src', kind: 'directory', size_bytes: 0 }]);
    expect(tree.visible()).toHaveLength(1);
    expect(tree.toggle('src')).toBe(true);
    tree.setEntries('src', [{ name: 'main.rs', path: 'src/main.rs', kind: 'file', size_bytes: 10 }]);
    expect(tree.visible().map(({ entry }) => entry.path)).toEqual(['src', 'src/main.rs']);
  });
  it('tracks loading and errors without losing loaded entries', () => {
    const tree = new ProjectTreeState(); tree.setEntries('', [{ name: 'x', path: 'x', kind: 'file', size_bytes: 1 }]);
    tree.setLoading('', true); tree.setError('', 'offline');
    expect(tree.nodes.get('')?.entries).toHaveLength(1); expect(tree.nodes.get('')?.error).toBe('offline');
  });
  it('clears stale directory nodes when the workspace changes', () => {
    const tree = new ProjectTreeState(); tree.child('old'); tree.reset();
    expect([...tree.nodes.keys()]).toEqual(['']);
  });
});

describe('document tabs and shell state', () => {
  it('switches tabs and preserves dirty markers', () => {
    const tabs = new TabState();
    tabs.open({ id: 1, path: 'a.rs', title: 'a.rs', revision: 0, dirty: false });
    tabs.open({ id: 2, path: 'b.md', title: 'b.md', revision: 0, dirty: true });
    tabs.select(1); tabs.update(1, { revision: 2, dirty: true });
    expect(tabs.active?.id).toBe(1); expect(tabs.tabs[0]?.dirty).toBe(true);
    tabs.close(1); expect(tabs.activeId).toBe(2);
  });
  it('only accepts explicit dialog choices and persists theme', () => {
    expect(approvalChoice('')).toBeNull(); expect(approvalChoice('approve')).toBe('approve');
    expect(conflictChoice('cancel')).toBeNull(); expect(conflictChoice('reload')).toBe('reload');
    const storage = new Map<string, string>();
    const adapter = { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => { storage.set(key, value); } };
    expect(nextTheme('dark')).toBe('light'); persistTheme(adapter, 'light'); expect(readTheme(adapter)).toBe('light');
  });
  it('keeps bounded task output and displays terminal state', () => {
    const snapshot = { run_id: 1, task_id: 2, state: 'succeeded' as const, exit_code: 0, error: null, stdout: Array.from({ length: 300 }, (_, i) => `${i}`).join('\n'), stderr: '', output_truncated: false };
    expect(visibleOutput(snapshot).split('\n')).toHaveLength(250);
    expect(taskStatus(snapshot)).toBe('succeeded (exit 0)');
  });
});
