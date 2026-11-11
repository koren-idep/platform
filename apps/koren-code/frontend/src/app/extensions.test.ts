import { describe, expect, it } from 'vitest';
import { extensionCanActivate, extensionToggleLabel, languageServerEnabled } from './extensions';
import type { ExtensionSummary, LanguageServerSummary } from '../services/protocol';

const extension = (enabled: boolean, state: ExtensionSummary['state']): ExtensionSummary => ({
  id: 'org.example.test', name: 'Example', publisher: 'Koren', version: '1.0.0', description: 'Test extension',
  enabled, state, error: null,
});

describe('extension lifecycle presentation', () => {
  it('offers enable for disabled extensions and activation only after enabling', () => {
    const disabled = extension(false, 'disabled');
    expect(extensionToggleLabel(disabled)).toBe('Enable');
    expect(extensionCanActivate(disabled)).toBe(false);
    const installed = extension(true, 'installed');
    expect(extensionToggleLabel(installed)).toBe('Disable');
    expect(extensionCanActivate(installed)).toBe(true);
  });

  it('does not offer activation twice and lets failed extensions retry', () => {
    expect(extensionCanActivate(extension(true, 'active'))).toBe(false);
    expect(extensionCanActivate(extension(true, 'failed'))).toBe(true);
  });

  it('exposes a server control only for its enabled declaring extension', () => {
    const enabled = extension(true, 'installed');
    const server: LanguageServerSummary = {
      extension_id: enabled.id, configuration_id: 'example', command: 'server', arguments: [],
      languages: ['example'], file_extensions: ['ex'], environment_keys: [],
    };
    expect(languageServerEnabled(enabled, server)).toBe(true);
    expect(languageServerEnabled(extension(false, 'disabled'), server)).toBe(false);
  });
});
