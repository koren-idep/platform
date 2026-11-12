export function filterCommands(commands: string[], query: string, labels: ReadonlyMap<string, string> = new Map(), descriptions: ReadonlyMap<string, string> = new Map()): string[] {
  const normalized = query.trim().toLocaleLowerCase();
  return commands.filter((command) => command.toLocaleLowerCase().includes(normalized)
    || (labels.get(command) ?? '').toLocaleLowerCase().includes(normalized)
    || (descriptions.get(command) ?? '').toLocaleLowerCase().includes(normalized));
}

export function nextCommandSelection(index: number, count: number, direction: -1 | 1, enabled?: boolean[]): number {
  if (count === 0 || (enabled && !enabled.some(Boolean))) return -1;
  for (let step = 1; step <= count; step += 1) {
    const candidate = ((index + direction * step) % count + count) % count;
    if (!enabled || enabled[candidate] !== false) return candidate;
  }
  return -1;
}

export function commandShortcutMatches(event: Pick<KeyboardEvent, 'key' | 'ctrlKey' | 'metaKey' | 'altKey' | 'shiftKey'>, shortcut: string): boolean {
  const parts = shortcut.toLowerCase().split('+').map((part) => part.trim());
  const key = parts.pop();
  if (!key || parts.some((part) => !['ctrl', 'cmd', 'meta', 'alt', 'shift'].includes(part))) return false;
  const wantsCtrl = parts.includes('ctrl') || parts.includes('cmd') || parts.includes('meta');
  return event.key.toLowerCase() === key
    && (event.ctrlKey || event.metaKey) === wantsCtrl
    && event.altKey === parts.includes('alt')
    && event.shiftKey === parts.includes('shift');
}
