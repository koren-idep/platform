import { describe, expect, it, vi } from 'vitest';
import { Text } from '@codemirror/state';
import { installDefinitionShortcut, positionForRange, workspaceRelativePath } from './definitionNavigation';

describe('definition target resolution', () => {
  it('routes F12 in the capture phase and leaves the command palette alone', () => {
    const run = vi.fn();
    let bubbled = false;
    const onBubble = () => { bubbled = true; };
    const dispose = installDefinitionShortcut(document, () => false, run);
    document.addEventListener('keydown', onBubble);
    const event = new KeyboardEvent('keydown', { key: 'F12', bubbles: true, cancelable: true });
    document.body.dispatchEvent(event);
    expect(run).toHaveBeenCalledOnce();
    expect(event.defaultPrevented).toBe(true);
    expect(bubbled).toBe(false);
    dispose();
    document.removeEventListener('keydown', onBubble);

    const paletteRun = vi.fn();
    const disposePalette = installDefinitionShortcut(document, () => true, paletteRun);
    document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'F12', bubbles: true, cancelable: true }));
    expect(paletteRun).not.toHaveBeenCalled();
    disposePalette();
  });

  it('decodes percent-encoded and Unicode file names', () => {
    expect(workspaceRelativePath('file:///work/src/caf%C3%A9%20test.rs', '/work')).toBe('src/café test.rs');
  });

  it('rejects non-file, remote, and out-of-workspace locations', () => {
    expect(workspaceRelativePath('https://example.test/src/lib.rs', '/work')).toBeNull();
    expect(workspaceRelativePath('file://server/work/src/lib.rs', '/work')).toBeNull();
    expect(workspaceRelativePath('file:///workspace-other/src/lib.rs', '/workspace')).toBeNull();
    expect(workspaceRelativePath('file:///work/../outside.rs', '/work')).toBeNull();
    expect(workspaceRelativePath('file:///%E0%A4%A', '/work')).toBeNull();
  });

  it('matches Windows drive roots without case sensitivity', () => {
    expect(workspaceRelativePath('file:///c:/work/src/lib.rs', 'C:\\work')).toBe('src/lib.rs');
    expect(workspaceRelativePath('file:///c:/src/lib.rs', 'C:\\')).toBe('src/lib.rs');
  });

  it('clamps LSP UTF-16 columns to the target line', () => {
    const doc = Text.of(['a😀b', 'last']);
    expect(positionForRange(doc, { start: { line: 0, character: 3 }, end: { line: 0, character: 4 } })).toBe(3);
    expect(positionForRange(doc, { start: { line: 1, character: 100 }, end: { line: 1, character: 100 } })).toBe(9);
    expect(positionForRange(doc, { start: { line: 99, character: 0 }, end: { line: 99, character: 0 } })).toBe(5);
  });
});
