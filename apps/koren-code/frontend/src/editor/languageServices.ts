import { autocompletion, type Completion, type CompletionContext } from '@codemirror/autocomplete';
import type { Extension } from '@codemirror/state';
import { hoverTooltip, keymap, type EditorView } from '@codemirror/view';
import type { KorenClient } from '../services/client';
import type { LanguageDiagnosticContext, TextRange } from '../services/protocol';
import { positionAt } from './sync';

export interface LanguageServiceHost {
  documentId: number;
  revision(): number;
  flush(): Promise<void>;
  diagnostics(): LanguageDiagnosticContext[];
  navigate(uri: string, range: TextRange): Promise<void>;
  message?(message: string): void;
}

export function selectedLocation<T>(locations: T[], selection: string | null): T | undefined {
  if (locations.length === 1) return locations[0];
  if (!selection || !/^[1-9]\d*$/.test(selection)) return undefined;
  return locations[Number(selection) - 1];
}

export async function goToDefinition(client: KorenClient, host: LanguageServiceHost, view: EditorView): Promise<void> {
  try {
    await host.flush();
    const revision = host.revision();
    const response = await client.call({ method: 'language_definition', params: {
      document_id: host.documentId, expected_revision: revision,
      position: positionAt(view.state.doc.toString(), view.state.selection.main.head),
    } });
    if (response.kind !== 'language_definitions' || response.value.document_revision !== host.revision()) return;
    const locations = response.value.locations;
    if (!locations.length) { host.message?.('No definition found at this location.'); return; }
    const choice = locations.length === 1 ? '1' : window.prompt(locations.map((location, index) =>
      `${index + 1}. ${location.uri}:${location.range.start.line + 1}:${location.range.start.character + 1}`,
    ).join('\n'), '1');
    const target = selectedLocation(locations, choice);
    if (!target) return;
    await host.navigate(target.uri, target.range);
  } catch (error) { host.message?.(error instanceof Error ? error.message : String(error)); }
}

function editorChanges(view: EditorView, edits: Array<{ range: TextRange; new_text: string }>) {
  const doc = view.state.doc;
  const changes = edits.map((edit) => {
    const startLine = doc.line(edit.range.start.line + 1);
    const endLine = doc.line(edit.range.end.line + 1);
    return { from: startLine.from + edit.range.start.character, to: endLine.from + edit.range.end.character, insert: edit.new_text };
  }).sort((left, right) => right.from - left.from);
  if (changes.some((change) => change.from < 0 || change.to < change.from || change.to > doc.length)) return null;
  if (changes.some((change, index) => index > 0 && change.to > (changes[index - 1]?.from ?? 0))) return null;
  return changes;
}

function previewChanges(view: EditorView, changes: NonNullable<ReturnType<typeof editorChanges>>, title: string): boolean {
  const before = view.state.doc.toString();
  const after = changes.reduce((text, change) => text.slice(0, change.from) + change.insert + text.slice(change.to), before);
  return window.confirm(`${title}\n\nBefore:\n${before.slice(0, 800)}\n\nAfter:\n${after.slice(0, 800)}`);
}

export function languageServiceExtensions(client: KorenClient, host: LanguageServiceHost): Extension[] {
  const completions = async (context: CompletionContext) => {
    const match = context.matchBefore(/[\p{L}\p{N}_$]*/u);
    if (!context.explicit && (!match || match.from === match.to)) return null;
    try {
      await host.flush();
      const revision = host.revision();
      const response = await client.call({ method: 'language_completion', params: {
        document_id: host.documentId, expected_revision: revision,
        position: positionAt(context.state.doc.toString(), context.pos),
      } });
      if (response.kind !== 'language_completions' || response.value.document_revision !== host.revision()) return null;
      const options: Completion[] = response.value.items.map((item) => ({
        label: item.label,
        detail: item.detail ?? undefined,
        apply: item.insert_text ?? item.label,
        info: item.documentation ?? undefined,
      }));
      return { from: match?.from ?? context.pos, options, validFor: /[\p{L}\p{N}_$]*/u };
    } catch { return null; }
  };

  const hover = hoverTooltip(async (view, position) => {
    try {
      await host.flush();
      const revision = host.revision();
      const response = await client.call({ method: 'language_hover', params: {
        document_id: host.documentId, expected_revision: revision,
        position: positionAt(view.state.doc.toString(), position),
      } });
      if (response.kind !== 'language_hover' || response.value.document_revision !== host.revision() || !response.value.contents) return null;
      return { pos: position, end: position, above: true, create: () => {
        const dom = document.createElement('div'); dom.className = 'language-hover'; dom.textContent = response.value.contents; return { dom };
      } };
    } catch { return null; }
  }, { hoverTime: 350, hideOnChange: true });

  const definition = async (view: EditorView) => { await goToDefinition(client, host, view); return true; };

  const references = async (view: EditorView) => {
    try {
      await host.flush();
      const revision = host.revision();
      const response = await client.call({ method: 'language_references', params: {
        document_id: host.documentId, expected_revision: revision,
        position: positionAt(view.state.doc.toString(), view.state.selection.main.head),
        include_declaration: true,
      } });
      if (response.kind === 'language_references' && response.value.document_revision === host.revision()) {
        const locations = response.value.locations;
        if (locations.length) {
          const choice = locations.length === 1 ? '1' : window.prompt(locations.map((location, index) =>
            `${index + 1}. ${location.uri}:${location.range.start.line + 1}:${location.range.start.character + 1}`,
          ).join('\n'), '1');
          const selected = selectedLocation(locations, choice);
          if (selected) await host.navigate(selected.uri, selected.range);
        }
      }
    } catch { /* no active server or references is a normal editor state */ }
    return true;
  };

  const format = async (view: EditorView) => {
    try {
      await host.flush();
      const revision = host.revision();
      const response = await client.call({ method: 'language_format', params: { document_id: host.documentId, expected_revision: revision } });
      if (response.kind !== 'language_formatting' || response.value.document_revision !== host.revision()) return true;
      const edits = response.value.edits;
      if (!edits.length) return true;
      const changes = editorChanges(view, edits);
      if (!changes || !previewChanges(view, changes, 'Preview formatting')) return true;
      if (host.revision() !== revision) return true;
      view.dispatch({ changes });
    } catch { /* no active server or formatting is a normal editor state */ }
    return true;
  };

  const codeActions = async (view: EditorView) => {
    try {
      await host.flush();
      const revision = host.revision();
      const text = view.state.doc.toString();
      const startOffset = Math.min(view.state.selection.main.anchor, view.state.selection.main.head);
      const endOffset = Math.max(view.state.selection.main.anchor, view.state.selection.main.head);
      const response = await client.call({ method: 'language_code_actions', params: {
        document_id: host.documentId, expected_revision: revision,
        range: { start: positionAt(text, startOffset), end: positionAt(text, endOffset) },
        diagnostics: host.diagnostics(),
      } });
      if (response.kind !== 'language_code_actions' || response.value.document_revision !== host.revision()) return true;
      const actions = response.value.actions;
      if (!actions.length) return true;
      const selection = window.prompt(actions.map((action, index) => `${index + 1}. ${action.title}${action.disabled_reason ? ` — ${action.disabled_reason}` : action.unsupported_reason ? ` — ${action.unsupported_reason}` : ''}`).join('\n'), '1');
      if (selection === null) return true;
      const action = actions[Number(selection) - 1];
      if (!action || action.disabled_reason || action.unsupported_reason || !action.edits?.length) return true;
      const changes = editorChanges(view, action.edits);
      if (!changes || !previewChanges(view, changes, `Preview: ${action.title}`)) return true;
      if (host.revision() !== revision) return true;
      view.dispatch({ changes });
    } catch { /* no active server or available code actions is a normal editor state */ }
    return true;
  };

  const rename = async (view: EditorView) => {
    try {
      await host.flush();
      const revision = host.revision();
      const position = positionAt(view.state.doc.toString(), view.state.selection.main.head);
      const newName = window.prompt('Rename symbol to:');
      if (!newName?.trim()) return true;
      const response = await client.call({ method: 'language_rename', params: {
        document_id: host.documentId, expected_revision: revision, position, new_name: newName.trim(),
      } });
      if (response.kind !== 'language_rename' || response.value.document_revision !== host.revision()) return true;
      const changes = editorChanges(view, response.value.edits);
      if (!changes?.length || !previewChanges(view, changes, 'Preview rename')) return true;
      if (host.revision() !== revision) return true;
      view.dispatch({ changes });
    } catch { /* no active server or rename target is a normal editor state */ }
    return true;
  };

  return [autocompletion({ override: [completions] }), hover, keymap.of([
    { key: 'F12', run: (view) => { void definition(view); return true; } },
    { key: 'Shift-F12', run: (view) => { void references(view); return true; } },
    { key: 'Mod-Shift-f', run: (view) => { void format(view); return true; } },
    { key: 'Mod-.', run: (view) => { void codeActions(view); return true; } },
    { key: 'F2', run: (view) => { void rename(view); return true; } },
  ])];
}
