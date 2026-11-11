import { describe, expect, it } from 'vitest';
import { filterCommands, nextCommandSelection } from './commands';
import { DEFAULT_LAYOUT, persistLayout, readLayout } from './layout';
import { formatCursorPosition, lineEndingFor } from './status';

describe('desktop presentation helpers', () => {
  it('filters registered commands and wraps palette selection', () => {
    expect(filterCommands(['core.ping', 'editor.save'], 'SAVE')).toEqual(['editor.save']);
    expect(nextCommandSelection(0, 2, -1)).toBe(1);
    expect(nextCommandSelection(0, 0, 1)).toBe(-1);
  });

  it('restores bounded layout preferences from storage', () => {
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => values.set(key, value),
    };
    persistLayout(storage, { ...DEFAULT_LAYOUT, explorerWidth: 300, bottomCollapsed: true });
    expect(readLayout(storage)).toMatchObject({ explorerWidth: 300, bottomCollapsed: true });
    values.set('koren-code:layout', '{"explorerWidth":9000}');
    expect(readLayout(storage).explorerWidth).toBe(480);
  });

  it('shows cursor and line-ending details without deriving them from language', () => {
    expect(formatCursorPosition(4, 12)).toBe('Ln 4, Col 12');
    expect(lineEndingFor('first\r\nsecond')).toBe('CRLF');
    expect(lineEndingFor('first\nsecond')).toBe('LF');
  });
});
