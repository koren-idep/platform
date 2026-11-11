import { autocompletion, type Completion, type CompletionContext } from '@codemirror/autocomplete';
import type { Extension } from '@codemirror/state';
import { hoverTooltip, keymap, type EditorView } from '@codemirror/view';
import type { KorenClient } from '../services/client';
import type { TextRange } from '../services/protocol';
import { positionAt } from './sync';

export interface LanguageServiceHost {
  documentId: number;
  revision(): number;
  flush(): Promise<void>;
  navigate(uri: string, range: TextRange): Promise<void>;
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

  const definition = async (view: EditorView) => {
    try {
      await host.flush();
      const revision = host.revision();
      const response = await client.call({ method: 'language_definition', params: {
        document_id: host.documentId, expected_revision: revision,
        position: positionAt(view.state.doc.toString(), view.state.selection.main.head),
      } });
      if (response.kind !== 'language_definitions' || response.value.document_revision !== host.revision()) return true;
      const target = response.value.locations[0];
      if (target) await host.navigate(target.uri, target.range);
    } catch { /* no active server or no definition is a normal editor state */ }
    return true;
  };

  return [autocompletion({ override: [completions] }), hover, keymap.of([{ key: 'F12', run: (view) => { void definition(view); return true; } }])];
}
