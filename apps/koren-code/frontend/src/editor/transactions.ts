import type { ViewUpdate } from '@codemirror/view';
import type { TextEdit } from '../services/protocol';

export function editsFromViewUpdate(update: ViewUpdate): TextEdit[] {
  const edits: TextEdit[] = [];
  update.changes.iterChanges((from, to, _fromB, _toB, inserted) => {
    const start = update.startState.doc.lineAt(from);
    const end = update.startState.doc.lineAt(to);
    edits.push({
      range: {
        start: { line: start.number - 1, character: from - start.from },
        end: { line: end.number - 1, character: to - end.from },
      },
      new_text: inserted.toString(),
    });
  });
  return edits;
}
