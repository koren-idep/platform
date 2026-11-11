import type { Extension } from '@codemirror/state';

export interface LanguageIntegration { id: string; label: string; extensions: string[]; create(): Extension | Promise<Extension> }
const integrations = new Map<string, LanguageIntegration>();
export function registerLanguage(integration: LanguageIntegration): void { integrations.set(integration.id, integration); }
export function languageNameForPath(path: string): string {
  return findLanguage(path)?.label ?? 'Plain Text';
}
export async function languageForPath(path: string): Promise<Extension[]> {
  const match = findLanguage(path);
  return match ? [await match.create()] : [];
}
function findLanguage(path: string): LanguageIntegration | undefined {
  const suffix = path.split('.').at(-1)?.toLowerCase();
  return [...integrations.values()].find((item) => item.extensions.includes(suffix ?? ''));
}
registerLanguage({
  id: 'rust-basic',
  label: 'Rust',
  extensions: ['rs'],
  async create() {
    return (await import('@codemirror/lang-rust')).rust();
  },
});
