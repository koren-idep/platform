export type WorkspaceEntryKind = 'directory' | 'file' | 'symlink' | 'other';

interface IconDefinition { name: string; color: string; paths: string[] }

export function fileIconMarkup(path: string, kind: WorkspaceEntryKind, expanded = false): string {
  if (kind === 'symlink') return '<svg viewBox="0 0 16 16" aria-hidden="true" fill="none" stroke="#80b7ca" stroke-width="1.25" stroke-linecap="round"><path d="M6.1 5.1 8 3.2a3 3 0 0 1 4.2 4.2l-1.5 1.5M9.9 10.9 8 12.8a3 3 0 0 1-4.2-4.2l1.5-1.5M5.8 10.2l4.4-4.4"/></svg>';
  if (kind === 'directory') {
    const folder = expanded
      ? '<path d="M1.5 4.5h4.7l1.3 1.4h7v6.5h-13z"/>'
      : '<path d="M1.5 4.5h4.7l1.3 1.4h7v6.5h-13z"/>';
    return `<svg viewBox="0 0 16 16" aria-hidden="true" fill="none" stroke="#c8a66b" stroke-width="1.2" stroke-linejoin="round">${folder}${expanded ? '<path d="M1.7 6h12.6" opacity=".5"/>' : ''}</svg>`;
  }
  const name = path.replaceAll('\\', '/').split('/').at(-1)?.toLocaleLowerCase() ?? '';
  const extension = name.split('.').at(-1) ?? '';
  const definition = iconFor(name, extension);
  return `<svg viewBox="0 0 16 16" aria-hidden="true" fill="none" stroke="${definition.color}" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" data-file-icon="${definition.name}">${definition.paths.join('')}</svg>`;
}

export function fileIconName(path: string): string {
  const name = path.replaceAll('\\', '/').split('/').at(-1)?.toLocaleLowerCase() ?? '';
  return iconFor(name, name.split('.').at(-1) ?? '').name;
}

function iconFor(name: string, extension: string): IconDefinition {
  if (extension === 'rs') return { name: 'rust', color: '#83b7ed', paths: ['<path d="m5.8 4.7-3 3.3 3 3.3M10.2 4.7l3 3.3-3 3.3M9 3.6l-2 8.8"/>'] };
  if (extension === 'lua') return { name: 'lua', color: '#83a6e8', paths: ['<circle cx="8" cy="8" r="5.7"/>', '<path d="M10 2.7a5.7 5.7 0 1 0 3.4 9.9A5.8 5.8 0 0 1 10 2.7z"/>'] };
  if (extension === 'json' || extension === 'jsonc') return { name: 'json', color: '#dfbd68', paths: ['<path d="M6 3.5c-1.4 0-1.5 1-1.5 2.2v.6c0 .9-.4 1.5-1.4 1.7 1 .2 1.4.8 1.4 1.7v.6c0 1.2.1 2.2 1.5 2.2M10 3.5c1.4 0 1.5 1 1.5 2.2v.6c0 .9.4 1.5 1.4 1.7-1 .2-1.4.8-1.4 1.7v.6c0 1.2-.1 2.2-1.5 2.2'] };
  if (extension === 'toml' || name === 'cargo.toml') return { name: 'toml', color: '#bb9ce6', paths: ['<path d="M2.5 3.5h11v9h-11z"/>', '<path d="M5 6h6M5 8h4M5 10h5"/>'] };
  if (['yaml', 'yml', 'ini', 'conf', 'config'].includes(extension) || name.startsWith('.')) return { name: 'config', color: '#90a6be', paths: ['<path d="M3 3h10v10H3z"/>', '<path d="M5.5 6h5M5.5 8h5M5.5 10h3"/>'] };
  if (['md', 'txt', 'log'].includes(extension)) return { name: 'text', color: '#aeb5bf', paths: ['<path d="M4 2.5h5l3 3v8H4z"/>', '<path d="M9 2.5v3h3M5.8 8h4.4M5.8 10h4.4"/>'] };
  if (['ts', 'tsx', 'js', 'jsx', 'mjs', 'cjs'].includes(extension)) return { name: 'script', color: '#7fa8df', paths: ['<path d="M3 3h10v10H3z"/>', '<path d="M6 6v4.7a1.3 1.3 0 1 1-2.6 0M8 10.5c.7 1.1 3.1 1.2 3.1-.1 0-1.5-3-1-3-2.7 0-1.2 1.9-1.6 3-0.6"/>'] };
  return { name: 'file', color: '#aeb5bf', paths: ['<path d="M4 2.5h5l3 3v8H4z"/>', '<path d="M9 2.5v3h3"/>'] };
}
