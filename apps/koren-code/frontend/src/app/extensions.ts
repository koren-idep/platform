import type { ExtensionSummary, LanguageServerSummary } from '../services/protocol';

export function extensionToggleLabel(extension: ExtensionSummary): 'Enable' | 'Disable' {
  return extension.enabled ? 'Disable' : 'Enable';
}

export function extensionCanActivate(extension: ExtensionSummary): boolean {
  return extension.enabled && extension.state !== 'active';
}

export function languageServerEnabled(extension: ExtensionSummary, server: LanguageServerSummary): boolean {
  return extension.enabled && server.extension_id === extension.id;
}
