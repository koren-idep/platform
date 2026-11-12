import { StateEffect, StateField, type Extension } from '@codemirror/state';
import { GutterMarker, gutter, type EditorView } from '@codemirror/view';

export type BreakpointIndicator = 'active' | 'disabled' | 'unverified' | null;

class DebugMarker extends GutterMarker {
  constructor(readonly kind: 'active' | 'disabled' | 'unverified' | 'execution') { super(); }
  eq(other: DebugMarker): boolean { return this.kind === other.kind; }
  toDOM(): HTMLElement {
    const element = document.createElement('span');
    element.className = `cm-debug-marker cm-debug-marker-${this.kind}`;
    element.textContent = this.kind === 'execution' ? '▶' : '●';
    element.setAttribute('aria-hidden', 'true');
    return element;
  }
}

const redraw = StateEffect.define<null>();

export function debugGutter(
  source: string,
  breakpointAt: (source: string, zeroBasedLine: number) => BreakpointIndicator,
  executionLine: (source: string) => number | null,
  toggle: (source: string, zeroBasedLine: number) => void,
): { extension: Extension; refresh: (view: EditorView) => void } {
  const version = StateField.define({
    create: () => 0,
    update(value, transaction) {
      for (const effect of transaction.effects) if (effect.is(redraw)) value += 1;
      return value;
    },
  });
  const extension = [version, gutter({
    class: 'cm-debug-gutter',
    renderEmptyElements: true,
    lineMarker(view, line) {
      const zeroLine = view.state.doc.lineAt(line.from).number - 1;
      if (executionLine(source) === zeroLine) return new DebugMarker('execution');
      const state = breakpointAt(source, zeroLine);
      return state ? new DebugMarker(state) : null;
    },
    lineMarkerChange(update) { return update.docChanged || update.transactions.some((transaction) => transaction.effects.some((effect) => effect.is(redraw))); },
    domEventHandlers: {
      mousedown(view, line, event) {
        event.preventDefault(); event.stopPropagation();
        toggle(source, view.state.doc.lineAt(line.from).number - 1);
        return true;
      },
    },
  })];
  return {
    extension,
    refresh(view) { view.dispatch({ effects: redraw.of(null) }); },
  };
}
