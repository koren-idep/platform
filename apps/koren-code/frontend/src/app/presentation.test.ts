import { describe, expect, it } from 'vitest';
import { filterCommands, nextCommandSelection } from './commands';
import { DEFAULT_LAYOUT, fitLayoutToViewport, persistLayout, readLayout } from './layout';
import { persistSession, readSession } from './session';
import { formatCursorPosition, lineEndingFor } from './status';
import { workbenchMarkup } from '../workbench-v2/shell';

describe('desktop presentation helpers', () => {
  it('starts a fresh layout with the bottom tool window collapsed', () => {
    expect(readLayout({ getItem: () => null }).bottomCollapsed).toBe(true);
    expect(DEFAULT_LAYOUT.explorerWidth).toBe(380);
    expect(DEFAULT_LAYOUT.bottomHeight).toBe(220);
  });

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
    persistLayout(storage, { ...DEFAULT_LAYOUT, explorerWidth: 380, bottomCollapsed: true });
    expect(readLayout(storage)).toMatchObject({ explorerWidth: 380, bottomCollapsed: true });
    values.set('koren-code:layout', '{"explorerWidth":9000}');
    expect(readLayout(storage).explorerWidth).toBe(480);
  });

  it('fits saved dimensions to a smaller current window', () => {
    expect(fitLayoutToViewport({ ...DEFAULT_LAYOUT, explorerWidth: 480, bottomHeight: 480 }, 800, 600))
      .toMatchObject({ explorerWidth: 336, bottomHeight: 300 });
  });

  it('restores a supported session and rejects corrupt, outdated, or unsafe state', () => {
    const values = new Map<string, string>();
    const storage = { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => values.set(key, value) };
    const session = { workspace: '/work/project', openFiles: ['src/main.rs', 'README.md'], activeFile: 'README.md', activePanel: 'problems' };
    persistSession(storage, session);
    expect(readSession(storage)).toEqual(session);
    values.set('koren-code:session', '{broken');
    expect(readSession(storage)).toBeNull();
    values.set('koren-code:session', JSON.stringify({ schema: 2, ...session }));
    expect(readSession(storage)).toBeNull();
    values.set('koren-code:session', JSON.stringify({ schema: 1, ...session, openFiles: ['../outside.rs'] }));
    expect(readSession(storage)).toBeNull();
  });

  it('shows cursor and line-ending details without deriving them from language', () => {
    expect(formatCursorPosition(4, 12)).toBe('Ln 4, Col 12');
    expect(lineEndingFor('first\r\nsecond')).toBe('CRLF');
    expect(lineEndingFor('first\nsecond')).toBe('LF');
  });

  it('hosts Settings as a main-view tab instead of an application-wide overlay', () => {
    const viewerStart = workbenchMarkup.indexOf('<section class="editor-frame"');
    const viewerEnd = workbenchMarkup.indexOf('<div class="wb-bottom-area">', viewerStart);
    const viewerMarkup = workbenchMarkup.slice(viewerStart, viewerEnd);
    expect(viewerMarkup).toContain('id="tabs"');
    expect(viewerMarkup).toContain('id="settings-view"');
    expect(workbenchMarkup.slice(viewerEnd)).not.toContain('id="settings-view"');
    expect(viewerMarkup).toContain('aria-labelledby="tab-settings"');
  });
});
