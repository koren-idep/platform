import { describe, expect, it } from 'vitest';
import { effectiveSetting, loadSettings, resetSetting, resetSettings, searchSettings, writeSetting } from './settings';
import { automaticProjectColor, hasReadableProjectAccent, PROJECT_COLORS } from './projectIdentity';

function memoryStorage(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial));
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    values,
  };
}

describe('typed application settings', () => {
  it('migrates the existing theme preference and resolves workspace overrides predictably', () => {
    const storage = memoryStorage({ 'koren-code:theme': 'light' });
    let snapshot = loadSettings(storage).snapshot;
    expect(effectiveSetting(snapshot, 'appearance.theme', null)).toEqual({ value: 'light', origin: 'user' });
    snapshot = writeSetting(storage, 'workbench.density', 'comfortable', 'user', null);
    snapshot = writeSetting(storage, 'workbench.density', 'compact', 'workspace', '/work');
    expect(effectiveSetting(snapshot, 'workbench.density', '/work')).toEqual({ value: 'compact', origin: 'workspace' });
    expect(effectiveSetting(snapshot, 'workbench.density', '/other')).toEqual({ value: 'comfortable', origin: 'user' });
  });

  it('rejects invalid values and unsupported scopes without writing', () => {
    const storage = memoryStorage();
    expect(() => writeSetting(storage, 'editor.fontSize', 99, 'user', null)).toThrow('Invalid value');
    expect(() => writeSetting(storage, 'appearance.theme', 'system', 'user', null)).toThrow('Invalid value');
    expect(() => writeSetting(storage, 'appearance.theme', 'dark', 'workspace', '/work')).toThrow('cannot be set');
    expect(storage.values.has('koren-code:settings:v1')).toBe(false);
  });

  it('searches metadata and filters to values changed at either scope', () => {
    const storage = memoryStorage();
    const snapshot = writeSetting(storage, 'editor.wordWrap', true, 'workspace', '/work');
    expect(searchSettings(snapshot, 'wrap long', false, '/work').map(({ id }) => id)).toEqual(['editor.wordWrap']);
    expect(searchSettings(snapshot, 'editor.', true, '/work').map(({ id }) => id)).toEqual(['editor.wordWrap']);
    expect(searchSettings(snapshot, 'editor.', true, '/elsewhere')).toEqual([]);
  });

  it('does not overwrite corrupt settings unless the user explicitly resets all settings', () => {
    const storage = memoryStorage({ 'koren-code:settings:v1': '{broken' });
    expect(loadSettings(storage).invalid).toBe(true);
    expect(() => writeSetting(storage, 'editor.fontSize', 16, 'user', null)).toThrow('Reset it explicitly');
    expect(() => resetSetting(storage, 'editor.fontSize', 'user', null)).toThrow('Reset All Settings');
    resetSettings(storage);
    expect(loadSettings(storage)).toEqual({ snapshot: { schema: 1, user: {}, workspaces: {} }, invalid: false });
  });

  it('leaves known settings with invalid stored values untouched and reports them for repair', () => {
    const storage = memoryStorage({ 'koren-code:settings:v1': JSON.stringify({ schema: 1, user: { 'editor.fontSize': 99 }, workspaces: {} }) });
    expect(loadSettings(storage).invalid).toBe(true);
    expect(() => writeSetting(storage, 'editor.fontSize', 16, 'user', null)).toThrow('invalid');
    expect(storage.values.get('koren-code:settings:v1')).toContain('99');
  });

  it('restores one setting to its inherited value without deleting other preferences', () => {
    const storage = memoryStorage();
    let snapshot = writeSetting(storage, 'editor.fontSize', 18, 'user', null);
    snapshot = writeSetting(storage, 'editor.fontSize', 16, 'workspace', '/work');
    snapshot = writeSetting(storage, 'editor.wordWrap', true, 'workspace', '/work');
    snapshot = resetSetting(storage, 'editor.fontSize', 'workspace', '/work');
    expect(effectiveSetting(snapshot, 'editor.fontSize', '/work')).toEqual({ value: 18, origin: 'user' });
    expect(effectiveSetting(snapshot, 'editor.wordWrap', '/work')).toEqual({ value: true, origin: 'workspace' });
  });

  it('keeps project identities isolated per workspace and independent of the global color toggle', () => {
    const storage = memoryStorage();
    let snapshot = writeSetting(storage, 'appearance.projectIdentity.color', '#3794ff', 'workspace', '/a');
    snapshot = writeSetting(storage, 'appearance.projectIdentity.color', '#7967e8', 'workspace', '/b');
    snapshot = writeSetting(storage, 'appearance.projectColors.enabled', false, 'user', null);
    expect(effectiveSetting(snapshot, 'appearance.projectIdentity.color', '/a').value).toBe('#3794ff');
    expect(effectiveSetting(snapshot, 'appearance.projectIdentity.color', '/b').value).toBe('#7967e8');
    expect(effectiveSetting(snapshot, 'appearance.projectColors.enabled', null).value).toBe(false);
    expect(automaticProjectColor('/a')).toBe(automaticProjectColor('/a'));
    expect(automaticProjectColor('/a')).not.toBe(automaticProjectColor('/b'));
  });

  it('keeps workspace identity when changing the application theme', () => {
    const storage = memoryStorage();
    let snapshot = writeSetting(storage, 'appearance.projectIdentity.color', '#7967e8', 'workspace', '/work');
    snapshot = writeSetting(storage, 'appearance.theme', 'light', 'user', null);
    expect(effectiveSetting(snapshot, 'appearance.theme', '/work').value).toBe('light');
    expect(effectiveSetting(snapshot, 'appearance.projectIdentity.color', '/work').value).toBe('#7967e8');
  });

  it('validates workspace color input and keeps palette colors readable', () => {
    const storage = memoryStorage();
    expect(() => writeSetting(storage, 'appearance.projectIdentity.color', 'blue', 'workspace', '/a')).toThrow('Invalid value');
    expect(PROJECT_COLORS.every(({ value }) => hasReadableProjectAccent(value))).toBe(true);
    expect(hasReadableProjectAccent('#zzzzzz')).toBe(false);
    expect(searchSettings(loadSettings(storage).snapshot, 'hex', false, '/a').map(({ id }) => id)).toContain('appearance.projectIdentity.color');
  });
});
