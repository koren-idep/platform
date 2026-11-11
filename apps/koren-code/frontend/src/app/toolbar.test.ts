import { describe, expect, it } from 'vitest';
import type { TaskSummary } from '../services/protocol';
import { runConfigurationTasks, selectedRunTask } from './toolbar';

const task = (task_id: number, category: string): TaskSummary => ({
  task_id, key: `task.${task_id}`, display_name: `Task ${task_id}`, category,
  executable: 'cargo', arguments: [], working_directory: '/workspace', timeout_ms: null, project_id: null,
});

describe('run toolbar configurations', () => {
  it('offers only runnable tasks and resolves the selected task id', () => {
    const tasks = [task(1, 'build'), task(2, 'run'), task(3, 'Run')];
    expect(runConfigurationTasks(tasks).map(({ task_id }) => task_id)).toEqual([2, 3]);
    expect(selectedRunTask(tasks, '3')?.display_name).toBe('Task 3');
  });

  it('rejects empty, malformed, and non-run selections', () => {
    const tasks = [task(1, 'build'), task(2, 'run')];
    expect(selectedRunTask(tasks, '')).toBeUndefined();
    expect(selectedRunTask(tasks, 'abc')).toBeUndefined();
    expect(selectedRunTask(tasks, '1')).toBeUndefined();
  });
});
