import { describe, expect, it } from 'vitest';
import { activePanelLayout, diagnosticDetailMatches, extensionCanActivate, extensionMenusAt, extensionToggleLabel, languageServerEnabled, safeThemeVariables, validExtensionSettingValue } from './extensions';
import type { ExtensionDiagnosticDetailSummary, ExtensionMenuSummary, ExtensionSummary, LanguageServerSummary } from '../services/protocol';

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

  it('prunes stale extension panels and keeps the order of supported panels', () => {
    expect(activePanelLayout(['org.sample.one', 'org.missing.panel', 'org.sample.two', 'org.sample.one'], ['org.sample.one', 'org.sample.two']))
      .toEqual(['org.sample.one', 'org.sample.two']);
  });

  it('validates setting values against typed extension schemas', () => {
    expect(validExtensionSettingValue({ type: 'integer', default: 3, min: 1, max: 5 }, 5)).toBe(true);
    expect(validExtensionSettingValue({ type: 'integer', default: 3, min: 1, max: 5 }, 6)).toBe(false);
    expect(validExtensionSettingValue({ type: 'choice', default: 'a', options: ['a', 'b'] }, 'b')).toBe(true);
    expect(validExtensionSettingValue({ type: 'boolean', default: true }, 'true')).toBe(false);
  });

  it('matches diagnostic details without replacing source or code attribution', () => {
    const detail: ExtensionDiagnosticDetailSummary = { id: 'org.example.help', source: 'rust-analyzer', code: 'E0308', title: 'Type help', explanation: 'Additional information', suggested_command: null, related: [] };
    expect(diagnosticDetailMatches(detail, 'rust-analyzer', 'E0308', 'provider')).toBe(true);
    expect(diagnosticDetailMatches(detail, 'rust-analyzer', 'E0505', 'provider')).toBe(false);
  });

  it('renders only matching extension menu locations', () => {
    const menus: ExtensionMenuSummary[] = [
      { command_id: 'org.example.run', label: 'Run', location: 'tools', enabled: true },
      { command_id: 'org.example.open', label: 'Open', location: 'editor_context', enabled: false },
    ];
    expect(extensionMenusAt(menus, 'tools').map((item) => item.command_id)).toEqual(['org.example.run']);
  });

  it('limits extension themes to known tokens and valid colors', () => {
    expect(safeThemeVariables({ accent: '#123abc', unknown: '#000000', text: 'url(https://example.invalid)' }))
      .toEqual({ '--accent': '#123abc' });
  });
});
