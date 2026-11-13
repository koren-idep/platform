import type { Text } from '@codemirror/state';
import type { TextRange } from '../services/protocol';

export function installDefinitionShortcut(target: Document, paletteOpen: () => boolean, run: () => void): () => void {
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key !== 'F12' || paletteOpen()) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    run();
  };
  target.addEventListener('keydown', onKeyDown, true);
  return () => target.removeEventListener('keydown', onKeyDown, true);
}

export function workspaceRelativePath(uri: string, workspaceRoot: string): string | null {
  try {
    const target = new URL(uri);
    if (target.protocol !== 'file:' || (target.hostname && target.hostname !== 'localhost')) return null;
    let filePath = decodeURIComponent(target.pathname).replaceAll('\\', '/');
    if (/^\/[A-Za-z]:\//.test(filePath)) filePath = filePath.slice(1);
    let root = workspaceRoot.replaceAll('\\', '/').replace(/\/+$/, '');
    if (/^[A-Za-z]:$/.test(root)) root += '/';
    const windowsPath = /^[A-Za-z]:\//.test(root);
    const compare = (value: string) => windowsPath ? value.toLocaleLowerCase('en-US') : value;
    const comparedPath = compare(filePath);
    const comparedRoot = compare(root);
    const rootPrefix = root.endsWith('/') ? comparedRoot : `${comparedRoot}/`;
    if (!comparedPath.startsWith(rootPrefix)) return null;
    const relative = filePath.slice(rootPrefix.length);
    if (!relative || relative.split('/').some((part) => !part || part === '.' || part === '..')) return null;
    return relative;
  } catch {
    return null;
  }
}

export function positionForRange(doc: Text, range: TextRange): number {
  const lineNumber = Math.max(1, Math.min(doc.lines, range.start.line + 1));
  const line = doc.line(lineNumber);
  return Math.max(line.from, Math.min(line.to, line.from + range.start.character));
}
