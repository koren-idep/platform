import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { describe, expect, it, vi } from 'vitest';
import { debugGutter } from './debugGutter';

describe('CodeMirror debug gutter', () => {
  it('renders execution and breakpoint states and toggles using zero-based lines', () => {
    document.body.innerHTML = '<div id="editor"></div>';
    const toggle = vi.fn();
    const gutter = debugGutter(
      '/workspace/main.c',
      (_source, line) => line === 1 ? 'disabled' : line === 0 ? 'unverified' : null,
      () => 2,
      toggle,
    );
    const view = new EditorView({
      state: EditorState.create({ doc: 'one\ntwo\nthree', extensions: [gutter.extension] }),
      parent: document.querySelector('#editor')!,
    });

    expect(document.querySelector('.cm-debug-marker-unverified')).not.toBeNull();
    expect(document.querySelector('.cm-debug-marker-disabled')).not.toBeNull();
    expect(document.querySelector('.cm-debug-marker-execution')).not.toBeNull();
    document.querySelectorAll<HTMLElement>('.cm-debug-gutter .cm-gutterElement')[1]!
      .dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true, button: 0, clientY: 20 }));
    expect(toggle).toHaveBeenCalledWith('/workspace/main.c', 0);
    view.destroy();
  });
});
