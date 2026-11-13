export function absoluteDebugSourcePath(workspaceRoot: string, documentPath: string): string {
  const root = workspaceRoot.replaceAll('\\', '/').replace(/\/+$/, '');
  const path = documentPath.replaceAll('\\', '/');
  if (path.startsWith('/') || /^[A-Za-z]:\//.test(path)) return path;
  if (!root.startsWith('/') && !/^[A-Za-z]:\//.test(root)) {
    throw new Error('Debug source requires an absolute workspace root');
  }
  return `${root}/${path.replace(/^(\.\/)+/, '')}`;
}
