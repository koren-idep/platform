export function filterCommands(commands: string[], query: string): string[] {
  const normalized = query.trim().toLocaleLowerCase();
  return commands.filter((command) => command.toLocaleLowerCase().includes(normalized));
}

export function nextCommandSelection(index: number, count: number, direction: -1 | 1): number {
  if (count === 0) return -1;
  return (index + direction + count) % count;
}
