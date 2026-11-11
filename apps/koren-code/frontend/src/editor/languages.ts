import type { Extension } from '@codemirror/state';
import { rust } from '@codemirror/lang-rust';

export interface LanguageIntegration { id: string; extensions: string[]; create(): Extension }
const integrations = new Map<string, LanguageIntegration>();
export function registerLanguage(integration: LanguageIntegration): void { integrations.set(integration.id, integration); }
export function languageForPath(path: string): Extension[] {
  const suffix = path.split('.').at(-1)?.toLowerCase();
  const match = [...integrations.values()].find((item) => item.extensions.includes(suffix ?? ''));
  return match ? [match.create()] : [];
}
registerLanguage({ id: 'rust-basic', extensions: ['rs'], create: rust });
