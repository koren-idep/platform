import type { TextEdit, TextPosition } from '../services/protocol';

export interface DocumentPort {
  edit(revision: number, edits: TextEdit[]): Promise<{ revision: number; dirty: boolean }>;
  read(): Promise<{ contents: string; revision: number; dirty: boolean }>;
}

export function positionAt(text: string, offset: number): TextPosition {
  const before = text.slice(0, offset);
  const lines = before.split('\n');
  return { line: lines.length - 1, character: lines.at(-1)?.length ?? 0 };
}

export function singleDiff(before: string, after: string): TextEdit | null {
  const left = Array.from(before);
  const right = Array.from(after);
  let prefix = 0;
  while (prefix < left.length && prefix < right.length && left[prefix] === right[prefix]) prefix++;
  let suffix = 0;
  while (suffix < left.length - prefix && suffix < right.length - prefix && left[left.length - 1 - suffix] === right[right.length - 1 - suffix]) suffix++;
  if (prefix === left.length && prefix === right.length) return null;
  const start = left.slice(0, prefix).join('').length;
  const end = before.length - left.slice(left.length - suffix).join('').length;
  const insert = right.slice(prefix, right.length - suffix).join('');
  return { range: { start: positionAt(before, start), end: positionAt(before, end) }, new_text: insert };
}

export class DocumentEditPipeline {
  private revision: number;
  private generation = 0;
  private syncedGeneration = 0;
  private tail: Promise<void> = Promise.resolve();
  constructor(revision: number, private readonly port: DocumentPort, private readonly latestText: () => string, private readonly onState: (revision: number, dirty: boolean) => void, private readonly onError: (error: unknown) => void) {
    this.revision = revision;
  }
  enqueue(edits: TextEdit[]): void {
    if (!edits.length) return;
    const generation = ++this.generation;
    this.tail = this.tail.then(async () => {
      if (generation <= this.syncedGeneration) return;
      try {
        const result = await this.port.edit(this.revision, edits);
        this.revision = result.revision;
        this.syncedGeneration = generation;
        this.onState(this.revision, result.dirty);
      } catch (error) {
        try { await this.reconcile(); }
        catch (reconcileError) { this.onError(reconcileError); throw reconcileError; }
        if (this.syncedGeneration < generation) this.onError(error);
      }
    }).catch((error: unknown) => { this.onError(error); });
  }
  async flush(): Promise<void> { await this.tail; }
  get currentRevision(): number { return this.revision; }
  private async reconcile(): Promise<void> {
    const snapshot = await this.port.read();
    const edit = singleDiff(snapshot.contents, this.latestText());
    this.revision = snapshot.revision;
    if (edit) {
      const result = await this.port.edit(this.revision, [edit]);
      this.revision = result.revision;
      this.onState(result.revision, result.dirty);
    } else {
      this.onState(snapshot.revision, snapshot.dirty);
    }
    this.syncedGeneration = this.generation;
  }
}
