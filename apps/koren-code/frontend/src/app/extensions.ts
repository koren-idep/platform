import type { ExtensionDiagnosticDetailSummary, ExtensionMenuSummary, ExtensionSettingKind, ExtensionSettingValue, ExtensionSummary, LanguageServerSummary } from '../services/protocol';

export function extensionToggleLabel(extension: ExtensionSummary): 'Enable' | 'Disable' {
  return extension.enabled ? 'Disable' : 'Enable';
}

export function extensionCanActivate(extension: ExtensionSummary): boolean {
  return extension.enabled && extension.state !== 'active';
}

export function languageServerEnabled(extension: ExtensionSummary, server: LanguageServerSummary): boolean {
  return extension.enabled && server.extension_id === extension.id;
}

export function activePanelLayout(open: string[], known: string[]): string[] {
  const available = new Set(known);
  return [...new Set(open)].filter((id) => available.has(id));
}

export function validExtensionSettingValue(kind: ExtensionSettingKind, value: ExtensionSettingValue): boolean {
  switch (kind.type) {
    case 'boolean': return typeof value === 'boolean';
    case 'integer': return typeof value === 'number' && Number.isInteger(value) && value >= kind.min && value <= kind.max;
    case 'text': return typeof value === 'string' && value.length <= kind.max_length;
    case 'choice': return typeof value === 'string' && kind.options.includes(value);
  }
}

export function diagnosticDetailMatches(detail: ExtensionDiagnosticDetailSummary, source: string | null, code: string | number | null, provider: string): boolean {
  return (!detail.source || detail.source === source || detail.source === provider)
    && (!detail.code || detail.code === String(code ?? ''));
}

export function extensionMenusAt(items: ExtensionMenuSummary[], location: string): ExtensionMenuSummary[] {
  return items.filter((item) => item.location === location);
}

export function safeThemeVariables(tokens: Record<string, string>): Record<string, string> {
  const properties: Record<string, string> = { background: '--bg', panel: '--panel', raised: '--raised', text: '--text', muted: '--muted', border: '--border', accent: '--accent', selection: '--selected' };
  return Object.fromEntries(Object.entries(tokens).filter(([name, color]) => properties[name] && /^#[0-9a-f]{6}$/i.test(color)).map(([name, color]) => [properties[name]!, color]));
}
