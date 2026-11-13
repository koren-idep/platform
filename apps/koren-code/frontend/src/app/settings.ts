export type SettingsScope = 'user' | 'workspace';
export type SettingPrimitive = boolean | number | string;
export type BuiltInSettingId =
  | 'appearance.theme'
  | 'workbench.density'
  | 'workbench.toolbarVisible'
  | 'editor.fontSize'
  | 'editor.lineNumbers'
  | 'editor.wordWrap';

interface SettingBase {
  id: BuiltInSettingId;
  category: 'Appearance' | 'Editor' | 'Workbench';
  title: string;
  description: string;
  scopes: readonly SettingsScope[];
  defaultValue: SettingPrimitive;
}
export type SettingDescriptor =
  | (SettingBase & { type: 'boolean' })
  | (SettingBase & { type: 'choice'; choices: readonly string[] })
  | (SettingBase & { type: 'integer'; minimum: number; maximum: number });

export const SETTING_DESCRIPTORS: readonly SettingDescriptor[] = [
  { id: 'appearance.theme', category: 'Appearance', title: 'Color theme', description: 'Choose Koren Dark or the light theme.', type: 'choice', choices: ['dark', 'light'], defaultValue: 'dark', scopes: ['user'] },
  { id: 'workbench.density', category: 'Workbench', title: 'Interface density', description: 'Use compact rows or larger comfortable targets.', type: 'choice', choices: ['compact', 'comfortable'], defaultValue: 'compact', scopes: ['user', 'workspace'] },
  { id: 'workbench.toolbarVisible', category: 'Workbench', title: 'Show toolbar', description: 'Keep primary file and run actions visible below the menu.', type: 'boolean', defaultValue: true, scopes: ['user', 'workspace'] },
  { id: 'editor.fontSize', category: 'Editor', title: 'Editor font size', description: 'Code font size in pixels.', type: 'integer', minimum: 10, maximum: 24, defaultValue: 14, scopes: ['user', 'workspace'] },
  { id: 'editor.lineNumbers', category: 'Editor', title: 'Show line numbers', description: 'Display line numbers in the editor gutter.', type: 'boolean', defaultValue: true, scopes: ['user', 'workspace'] },
  { id: 'editor.wordWrap', category: 'Editor', title: 'Wrap long lines', description: 'Wrap lines that extend beyond the editor width.', type: 'boolean', defaultValue: false, scopes: ['user', 'workspace'] },
];

export interface SettingsSnapshot {
  schema: 1;
  user: Record<string, SettingPrimitive>;
  workspaces: Record<string, Record<string, SettingPrimitive>>;
}
export interface LoadedSettings { snapshot: SettingsSnapshot; invalid: boolean }
export interface EffectiveSetting { value: SettingPrimitive; origin: 'default' | SettingsScope }

const STORAGE_KEY = 'koren-code:settings:v1';
const defaultSnapshot = (): SettingsSnapshot => ({ schema: 1, user: {}, workspaces: {} });

export function loadSettings(storage: Pick<Storage, 'getItem'>): LoadedSettings {
  const raw = storage.getItem(STORAGE_KEY);
  if (raw === null) {
    const snapshot = defaultSnapshot();
    const legacyTheme = storage.getItem('koren-code:theme');
    if (legacyTheme === 'light' || legacyTheme === 'dark') snapshot.user['appearance.theme'] = legacyTheme;
    return { snapshot, invalid: false };
  }
  try {
    const parsed = JSON.parse(raw) as Partial<SettingsSnapshot>;
    if (parsed.schema !== 1 || !isRecord(parsed.user) || !isRecord(parsed.workspaces) ||
        !Object.values(parsed.workspaces).every(isRecord)) return { snapshot: defaultSnapshot(), invalid: true };
    return { snapshot: {
      schema: 1,
      user: filterValidStoredValues(parsed.user),
      workspaces: Object.fromEntries(Object.entries(parsed.workspaces).map(([path, values]) => [path, filterValidStoredValues(values)])),
    }, invalid: false };
  } catch {
    return { snapshot: defaultSnapshot(), invalid: true };
  }
}

export function effectiveSetting(snapshot: SettingsSnapshot, id: BuiltInSettingId, workspace: string | null): EffectiveSetting {
  const descriptor = descriptorFor(id);
  const workspaceValue = workspace ? snapshot.workspaces[workspace]?.[id] : undefined;
  if (workspaceValue !== undefined && descriptor.scopes.includes('workspace') && validValue(descriptor, workspaceValue)) return { value: workspaceValue, origin: 'workspace' };
  const userValue = snapshot.user[id];
  if (userValue !== undefined && validValue(descriptor, userValue)) return { value: userValue, origin: 'user' };
  return { value: descriptor.defaultValue, origin: 'default' };
}

export function searchSettings(snapshot: SettingsSnapshot, query: string, modifiedOnly: boolean, workspace: string | null): SettingDescriptor[] {
  const needle = query.trim().toLocaleLowerCase();
  return SETTING_DESCRIPTORS.filter((descriptor) => {
    const modified = snapshot.user[descriptor.id] !== undefined || (workspace !== null && snapshot.workspaces[workspace]?.[descriptor.id] !== undefined);
    const matchesText = !needle || `${descriptor.id} ${descriptor.title} ${descriptor.description} ${descriptor.category}`.toLocaleLowerCase().includes(needle);
    return matchesText && (!modifiedOnly || modified);
  });
}

export function writeSetting(storage: Pick<Storage, 'getItem' | 'setItem'>, id: BuiltInSettingId, value: SettingPrimitive, scope: SettingsScope, workspace: string | null): SettingsSnapshot {
  const loaded = loadSettings(storage);
  if (loaded.invalid) throw new Error('Settings data is invalid. Reset it explicitly before saving new values.');
  const descriptor = descriptorFor(id);
  if (!descriptor.scopes.includes(scope)) throw new Error(`${id} cannot be set at ${scope} scope.`);
  if (scope === 'workspace' && !workspace) throw new Error('Open a workspace before editing workspace settings.');
  if (!validValue(descriptor, value)) throw new Error(`Invalid value for ${id}.`);
  const snapshot: SettingsSnapshot = {
    ...loaded.snapshot,
    user: { ...loaded.snapshot.user },
    workspaces: { ...loaded.snapshot.workspaces },
  };
  if (scope === 'user') snapshot.user[id] = value;
  else snapshot.workspaces[workspace!] = { ...snapshot.workspaces[workspace!], [id]: value };
  storage.setItem(STORAGE_KEY, JSON.stringify(snapshot));
  return snapshot;
}

export function resetSetting(storage: Pick<Storage, 'getItem' | 'setItem'>, id: BuiltInSettingId, scope: SettingsScope, workspace: string | null): SettingsSnapshot {
  const loaded = loadSettings(storage);
  if (loaded.invalid) throw new Error('Settings data is invalid. Use Reset All Settings to replace it.');
  const descriptor = descriptorFor(id);
  if (!descriptor.scopes.includes(scope)) throw new Error(`${id} cannot be reset at ${scope} scope.`);
  if (scope === 'workspace' && !workspace) throw new Error('Open a workspace before editing workspace settings.');
  const snapshot = loaded.snapshot;
  if (scope === 'user') delete snapshot.user[id];
  else if (workspace) {
    const values = { ...snapshot.workspaces[workspace] };
    delete values[id];
    if (Object.keys(values).length) snapshot.workspaces[workspace] = values;
    else delete snapshot.workspaces[workspace];
  }
  storage.setItem(STORAGE_KEY, JSON.stringify(snapshot));
  return snapshot;
}

export function resetSettings(storage: Pick<Storage, 'setItem'>): SettingsSnapshot {
  const snapshot = defaultSnapshot();
  storage.setItem(STORAGE_KEY, JSON.stringify(snapshot));
  return snapshot;
}

export function isValidSettingValue(id: BuiltInSettingId, value: unknown): value is SettingPrimitive {
  return validValue(descriptorFor(id), value);
}

function descriptorFor(id: BuiltInSettingId): SettingDescriptor {
  const descriptor = SETTING_DESCRIPTORS.find((item) => item.id === id);
  if (!descriptor) throw new Error(`Unknown setting: ${id}`);
  return descriptor;
}

function validValue(descriptor: SettingDescriptor, value: unknown): value is SettingPrimitive {
  if (descriptor.type === 'boolean') return typeof value === 'boolean';
  if (descriptor.type === 'integer') return typeof value === 'number' && Number.isInteger(value) && value >= descriptor.minimum && value <= descriptor.maximum;
  return typeof value === 'string' && descriptor.choices.includes(value);
}

function filterValidStoredValues(values: Record<string, unknown>): Record<string, SettingPrimitive> {
  return Object.fromEntries(Object.entries(values).filter(([id, value]) => {
    const descriptor = SETTING_DESCRIPTORS.find((item) => item.id === id);
    return descriptor ? validValue(descriptor, value) : isPrimitive(value);
  })) as Record<string, SettingPrimitive>;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isPrimitive(value: unknown): value is SettingPrimitive {
  return typeof value === 'boolean' || typeof value === 'string' || (typeof value === 'number' && Number.isFinite(value));
}
