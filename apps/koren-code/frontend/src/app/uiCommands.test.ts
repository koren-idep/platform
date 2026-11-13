import { describe, expect, it } from 'vitest';
import { UI_COMMANDS } from './uiCommands';

describe('built-in workbench command catalog', () => {
  it('uses unique stable IDs and default shortcuts', () => {
    const ids = UI_COMMANDS.map(({ id }) => id);
    const shortcuts = UI_COMMANDS.map(({ shortcut }) => shortcut).filter(Boolean);
    expect(new Set(ids).size).toBe(ids.length);
    expect(new Set(shortcuts).size).toBe(shortcuts.length);
    expect(UI_COMMANDS.find(({ id }) => id === 'ui.toggle_outline')).toMatchObject({ action: 'toggle-secondary', shortcut: '' });
    expect(UI_COMMANDS.find(({ id }) => id === 'ui.search_workspace')).toMatchObject({ action: 'search-workspace', shortcut: 'Ctrl+Shift+F' });
  });
});
