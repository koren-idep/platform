import { describe, expect, it, vi } from 'vitest';
import { DocumentEditPipeline } from './sync';
import { flushAndSave } from './save';

describe('document save handling', () => {
  it('waits for queued edits and saves the acknowledged revision', async () => {
    let release: ((value: { revision: number; dirty: boolean }) => void) | undefined;
    const edit = new Promise<{ revision: number; dirty: boolean }>((resolve) => { release = resolve; });
    const pipeline = new DocumentEditPipeline(4, { edit: () => edit, read: async () => ({ contents: 'x', revision: 4, dirty: false }) }, () => 'x', () => {}, vi.fn());
    pipeline.enqueue([{ range: { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } }, new_text: 'x' }]);
    const save = vi.fn(async (revision: number) => ({ revision, dirty: false }));
    let finished = false;
    const saving = flushAndSave(pipeline, save).then((result) => { finished = true; return result; });
    await Promise.resolve();
    expect(finished).toBe(false);
    if (!release) throw new Error('edit request did not start');
    release({ revision: 5, dirty: true });
    await expect(saving).resolves.toEqual({ revision: 5, dirty: false });
    expect(save).toHaveBeenCalledWith(5);
  });
});
