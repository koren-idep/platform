import { describe, expect, it, vi } from 'vitest';
import { EditorState } from '@codemirror/state';
import { goToDefinition, selectedLocation } from './languageServices';

describe('language definition location selection', () => {
  it('uses the only result without prompting', () => {
    expect(selectedLocation(['one'], null)).toBe('one');
  });

  it('accepts a valid one-based choice and ignores invalid or canceled choices', () => {
    expect(selectedLocation(['one', 'two'], '2')).toBe('two');
    expect(selectedLocation(['one', 'two'], null)).toBeUndefined();
    expect(selectedLocation(['one', 'two'], '3')).toBeUndefined();
    expect(selectedLocation(['one', 'two'], '1.5')).toBeUndefined();
  });

  it('flushes the active document and navigates to a cross-file target', async () => {
    const calls: unknown[] = [];
    const client = { call: async (request: unknown) => { calls.push(request); return {
      kind: 'language_definitions', value: { document_revision: 4, locations: [
        { uri: 'file:///work/src/helper.rs', range: { start: { line: 2, character: 6 }, end: { line: 2, character: 12 } } },
      ] },
    }; } } as never;
    let flushed = false;
    const navigated: unknown[] = [];
    const host = {
      documentId: 7, revision: () => 4, flush: async () => { flushed = true; }, diagnostics: () => [],
      navigate: async (uri: string, range: unknown) => { navigated.push({ uri, range }); },
    };
    const view = { state: EditorState.create({ doc: 'helper::answer()' }), dispatch() {}, focus() {} } as never;
    await goToDefinition(client, host, view);
    expect(flushed).toBe(true);
    expect(calls).toHaveLength(1);
    expect(navigated).toEqual([{ uri: 'file:///work/src/helper.rs', range: { start: { line: 2, character: 6 }, end: { line: 2, character: 12 } } }]);
  });

  it('ignores a stale definition response without navigating', async () => {
    const client = { call: async () => ({ kind: 'language_definitions', value: { document_revision: 3, locations: [{ uri: 'file:///work/lib.rs', range: { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } } }] } }) } as never;
    const navigated: unknown[] = [];
    const host = { documentId: 1, revision: () => 4, flush: async () => {}, diagnostics: () => [], navigate: async (...args: unknown[]) => { navigated.push(args); } };
    const view = { state: EditorState.create({ doc: 'name' }) } as never;
    await goToDefinition(client, host, view);
    expect(navigated).toEqual([]);
  });

  it('reports empty and unsupported definition responses without navigating', async () => {
    const messages: string[] = [];
    const host = { documentId: 1, revision: () => 2, flush: async () => {}, diagnostics: () => [], navigate: async () => {}, message: (message: string) => { messages.push(message); } };
    const view = { state: EditorState.create({ doc: 'keyword' }) } as never;
    await goToDefinition({ call: async () => ({ kind: 'language_definitions', value: { document_revision: 2, locations: [] } }) } as never, host, view);
    await goToDefinition({ call: async () => { throw new Error('The active language server does not support Go to Definition.'); } } as never, host, view);
    expect(messages).toEqual(['No definition found at this location.', 'The active language server does not support Go to Definition.']);
  });

  it('prompts for multiple destinations and navigates to the chosen one', async () => {
    vi.spyOn(window, 'prompt').mockReturnValue('2');
    const navigated: string[] = [];
    const client = { call: async () => ({ kind: 'language_definitions', value: { document_revision: 2, locations: [
      { uri: 'file:///work/first.rs', range: { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } } },
      { uri: 'file:///work/second.rs', range: { start: { line: 1, character: 0 }, end: { line: 1, character: 1 } } },
    ] } }) } as never;
    const host = { documentId: 1, revision: () => 2, flush: async () => {}, diagnostics: () => [], navigate: async (uri: string) => { navigated.push(uri); } };
    await goToDefinition(client, host, { state: EditorState.create({ doc: 'name' }) } as never);
    expect(navigated).toEqual(['file:///work/second.rs']);
  });
});
