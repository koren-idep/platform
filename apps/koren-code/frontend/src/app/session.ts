export interface EditorSession {
  workspace: string;
  openFiles: string[];
  activeFile: string | null;
  activePanel: string;
}

const KEY = 'koren-code:session';
const PANELS = new Set(['output', 'problems', 'tasks', 'projects', 'search', 'source-control', 'terminal', 'debug']);

export function readSession(storage: Pick<Storage, 'getItem'>): EditorSession | null {
  const raw = storage.getItem(KEY);
  if (!raw) return null;
  try {
    const value = JSON.parse(raw) as Partial<EditorSession> & { schema?: unknown };
    if (value.schema !== 1 || typeof value.workspace !== 'string' || !value.workspace || value.workspace.length > 4096 ||
        !Array.isArray(value.openFiles) || value.openFiles.length > 200 ||
        !value.openFiles.every(isWorkspacePath) ||
        !(value.activeFile === null || (typeof value.activeFile === 'string' && isWorkspacePath(value.activeFile))) ||
        typeof value.activePanel !== 'string' || !PANELS.has(value.activePanel) ||
        (value.activeFile !== null && !value.openFiles.includes(value.activeFile))) return null;
    return { workspace: value.workspace, openFiles: [...new Set(value.openFiles)], activeFile: value.activeFile, activePanel: value.activePanel };
  } catch {
    return null;
  }
}

export function persistSession(storage: Pick<Storage, 'setItem'>, session: EditorSession): void {
  storage.setItem(KEY, JSON.stringify({ schema: 1, ...session }));
}

function isWorkspacePath(path: unknown): path is string {
  return typeof path === 'string' && path.length > 0 && path.length <= 4096 && !path.startsWith('/') &&
    !path.includes('\\') && !path.split('/').some((part) => !part || part === '.' || part === '..');
}
