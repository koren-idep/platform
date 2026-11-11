import type { TaskSummary } from '../services/protocol';

export function runConfigurationTasks(tasks: TaskSummary[]): TaskSummary[] {
  return tasks.filter((task) => task.category.toLowerCase() === 'run');
}

export function selectedRunTask(tasks: TaskSummary[], selectedId: string): TaskSummary | undefined {
  if (!/^\d+$/.test(selectedId)) return undefined;
  return runConfigurationTasks(tasks).find((task) => String(task.task_id) === selectedId);
}
