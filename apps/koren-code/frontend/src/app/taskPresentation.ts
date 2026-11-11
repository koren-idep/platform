import type { TaskSnapshot } from '../services/protocol';
export function visibleOutput(snapshot: TaskSnapshot, maxLines = 250): string {
  const text = `${snapshot.stdout}${snapshot.stderr ? `\n${snapshot.stderr}` : ''}`;
  return text.split('\n').slice(-maxLines).join('\n');
}
export function taskStatus(snapshot: TaskSnapshot): string {
  return snapshot.state === 'running' ? 'Running…' : `${snapshot.state}${snapshot.exit_code === null ? '' : ` (exit ${snapshot.exit_code})`}`;
}
