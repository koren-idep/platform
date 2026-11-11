import { describe, expect, it, vi } from 'vitest';
import { DocumentEditPipeline, singleDiff } from './sync';
import type { TextEdit } from '../services/protocol';
import { EditorState } from '@codemirror/state';
import type { ViewUpdate } from '@codemirror/view';
import { editsFromViewUpdate } from './transactions';

describe('document edit sequencing', () => {
  it('sends rapid edits in order against acknowledged revisions', async () => {
    let text = ''; let revision = 0;
    const calls: number[] = [];
    const pipeline = new DocumentEditPipeline(0, {
      edit: async (expected, edits) => { calls.push(expected); expect(expected).toBe(revision); text += edits[0]?.new_text ?? ''; revision++; return { revision, dirty: true }; },
      read: async () => ({ contents: text, revision, dirty: true }),
    }, () => text, () => {}, vi.fn());
    pipeline.enqueue([{ range: { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } }, new_text: 'a' }]);
    pipeline.enqueue([{ range: { start: { line: 0, character: 1 }, end: { line: 0, character: 1 } }, new_text: 'b' }]);
    await pipeline.flush();
    expect(calls).toEqual([0, 1]); expect(text).toBe('ab');
  });
  it('recovers from stale revisions without dropping latest text', async () => {
    let attempts = 0; const current = 'start'; const wanted = 'start🙂 ok';
    const edit = vi.fn(async (revision: number, edits: TextEdit[]) => {
      void revision; void edits;
      attempts++;
      if (attempts === 1) throw new Error('stale_revision');
      return { revision: 8, dirty: true };
    });
    const state: { revision: number; dirty: boolean } = { revision: 7, dirty: true };
    const pipeline = new DocumentEditPipeline(2, { edit, read: async () => ({ contents: current, ...state }) }, () => wanted, () => {}, vi.fn());
    pipeline.enqueue([{ range: { start: { line: 0, character: 5 }, end: { line: 0, character: 5 } }, new_text: '🙂' }]);
    await pipeline.flush();
    expect(edit).toHaveBeenCalledTimes(2);
    expect(edit.mock.calls[1]?.[0]).toBe(7);
    expect(edit.mock.calls[1]?.[1][0]).toMatchObject({ new_text: '🙂 ok' });
  });
  it('builds Unicode-safe minimal edits across lines and preserves CRLF context', () => {
    expect(singleDiff('a😀\r\nb', 'a😀\r\nc')?.range).toEqual({ start: { line: 1, character: 0 }, end: { line: 1, character: 1 } });
    expect(singleDiff('a😀x', 'a🙂x')?.range).toEqual({ start: { line: 0, character: 1 }, end: { line: 0, character: 3 } });
    expect(singleDiff('same', 'same')).toBeNull();
  });
  it('maps CodeMirror UTF-16 transactions to protocol edits', () => {
    const startState = EditorState.create({ doc: 'a😀\r\nb' });
    const update = {
      startState,
      changes: { iterChanges: (callback: (from: number, to: number, fromB: number, toB: number, inserted: { toString(): string }) => void) => callback(1, 3, 1, 2, { toString: () => '🙂' }) },
    } as unknown as ViewUpdate;
    expect(editsFromViewUpdate(update)).toEqual([{ range: { start: { line: 0, character: 1 }, end: { line: 0, character: 3 } }, new_text: '🙂' }]);
    const crlf = EditorState.create({ doc: 'a\r\nb', extensions: [EditorState.lineSeparator.of('\r\n')] });
    expect(crlf.doc.lines).toBe(2);
    expect(crlf.doc.lineAt(3).number).toBe(2);
  });
});
