import { describe, expect, it } from 'vitest';
import { commandShortcutMatches, filterCommands, nextCommandSelection } from './commands';

describe('command palette', () => {
  it('searches command ids and extension display labels', () => {
    const labels = new Map([['org.example.refresh', 'Refresh Workspace']]);
    const descriptions = new Map([['core.ping', 'Check that the platform is running']]);
    expect(filterCommands(['core.ping', 'org.example.refresh'], 'workspace', labels)).toEqual(['org.example.refresh']);
    expect(filterCommands(['core.ping', 'org.example.refresh'], 'platform is running', labels, descriptions)).toEqual(['core.ping']);
    expect(filterCommands(['core.ping', 'org.example.refresh'], 'CORE')).toEqual(['core.ping']);
  });

  it('moves selection cyclically and handles an empty result set', () => {
    expect(nextCommandSelection(1, 2, 1)).toBe(0);
    expect(nextCommandSelection(0, 0, 1)).toBe(-1);
    expect(nextCommandSelection(-1, 3, 1, [false, true, false])).toBe(1);
    expect(nextCommandSelection(1, 3, 1, [false, false, false])).toBe(-1);
  });

  it('matches contributed shortcuts with platform modifier aliases', () => {
    expect(commandShortcutMatches({ key: 'H', ctrlKey: true, metaKey: false, altKey: true, shiftKey: false }, 'Ctrl+Alt+H')).toBe(true);
    expect(commandShortcutMatches({ key: 'H', ctrlKey: false, metaKey: false, altKey: true, shiftKey: false }, 'Ctrl+Alt+H')).toBe(false);
  });
});
