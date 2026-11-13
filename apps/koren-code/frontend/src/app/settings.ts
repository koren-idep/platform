export type SettingsScope = 'user' | 'workspace';
export type SettingPrimitive = boolean | number | string;
export type BuiltInSettingId =
  | 'appearance.theme'
  | 'appearance.classicMenu'
  | 'appearance.projectColors.enabled'
  | 'appearance.projectColors.autoAssign'
  | 'appearance.projectColors.style'
  | 'appearance.projectColors.intensity'
  | 'appearance.projectColors.showWorkspaceName'
  | 'appearance.projectIdentity.color'
  | 'appearance.projectIdentity.icon'
  | 'workbench.density'
  | 'workbench.uiScale'
  | 'workbench.toolbarVisible'
  | 'editor.fontSize'
  | 'editor.tabSize'
  | 'editor.indentSize'
  | 'editor.insertSpaces'
  | 'editor.lineNumbers'
  | 'editor.wordWrap';

interface SettingBase {
  id: BuiltInSettingId;
  category: string;
  title: string;
  description: string;
  scopes: readonly SettingsScope[];
  defaultValue: SettingPrimitive;
  keywords: readonly string[];
  owner: 'koren';
  requiresRestart: boolean;
  isAvailable: boolean;
}
export type SettingDescriptor =
  | (SettingBase & { type: 'boolean' })
  | (SettingBase & { type: 'choice'; choices: readonly string[] })
  | (SettingBase & { type: 'integer'; minimum: number; maximum: number })
  | (SettingBase & { type: 'color' });

const userScope = ['user'] as const;
const userWorkspaceScope = ['user', 'workspace'] as const;

export const SETTING_DESCRIPTORS: readonly SettingDescriptor[] = [
  { id: 'appearance.theme', category: 'Appearance/Theme', title: 'Color theme', description: 'Choose Koren Dark or the light theme.', type: 'choice', choices: ['dark', 'light'], defaultValue: 'dark', scopes: userScope, keywords: ['color', 'contrast'], owner: 'koren', requiresRestart: false, isAvailable: true },
  { id: 'appearance.classicMenu', category: 'Appearance/Interface', title: 'Show menu bar', description: 'Show File, Edit, View, Navigate, Run, Tools, and Help in the title bar.', type: 'boolean', defaultValue: true, scopes: userScope, keywords: ['menu', 'compact'], owner: 'koren', requiresRestart: false, isAvailable: true },
  { id: 'appearance.projectColors.enabled', category: 'Appearance/Project Identity', title: 'Enable Project Colors', description: 'Display the workspace color as a restrained accent in the application header.', type: 'boolean', defaultValue: true, scopes: userScope, keywords: ['project', 'identity', 'accent'], owner: 'koren', requiresRestart: false, isAvailable: true },
  { id: 'appearance.projectColors.autoAssign', category: 'Appearance/Project Identity', title: 'Automatically assign colors', description: 'Choose a stable color for workspaces without a saved color.', type: 'boolean', defaultValue: true, scopes: userScope, keywords: ['project', 'identity', 'automatic'], owner: 'koren', requiresRestart: false, isAvailable: true },
  { id: 'appearance.projectColors.style', category: 'Appearance/Project Identity', title: 'Color style', description: 'Choose how the workspace accent appears in the header.', type: 'choice', choices: ['subtle-gradient', 'solid-tint', 'icon-only', 'none'], defaultValue: 'subtle-gradient', scopes: userScope, keywords: ['project', 'gradient', 'solid', 'icon'], owner: 'koren', requiresRestart: false, isAvailable: true },
  { id: 'appearance.projectColors.intensity', category: 'Appearance/Project Identity', title: 'Accent intensity', description: 'Set the strength of the workspace color wash.', type: 'integer', minimum: 8, maximum: 40, defaultValue: 22, scopes: userScope, keywords: ['project', 'gradient', 'strength'], owner: 'koren', requiresRestart: false, isAvailable: true },
  { id: 'appearance.projectColors.showWorkspaceName', category: 'Appearance/Project Identity', title: 'Show workspace name', description: 'Display the current workspace name in the compact toolbar.', type: 'boolean', defaultValue: true, scopes: userScope, keywords: ['project', 'name'], owner: 'koren', requiresRestart: false, isAvailable: true },
  { id: 'appearance.projectIdentity.color', category: 'Appearance/Project Identity', title: 'Project color', description: 'Set a local color for this workspace. This preference is not written to Git.', type: 'color', defaultValue: '#5294d8', scopes: ['workspace'], keywords: ['project', 'custom', 'hex', 'palette'], owner: 'koren', requiresRestart: false, isAvailable: true },
  { id: 'appearance.projectIdentity.icon', category: 'Appearance/Project Identity', title: 'Project icon', description: 'Choose a small icon for this workspace.', type: 'choice', choices: ['folder', 'code', 'package', 'tools'], defaultValue: 'folder', scopes: ['workspace'], keywords: ['project', 'icon'], owner: 'koren', requiresRestart: false, isAvailable: true },
  { id: 'workbench.density', category: 'Appearance/Interface', title: 'Interface density', description: 'Use compact rows or larger comfortable targets.', type: 'choice', choices: ['compact', 'comfortable'], defaultValue: 'compact', scopes: userWorkspaceScope, keywords: ['spacing', 'rows'], owner: 'koren', requiresRestart: false, isAvailable: true },
  { id: 'workbench.uiScale', category: 'Appearance/Interface', title: 'Interface scale', description: 'Scale the application interface without changing code text size.', type: 'integer', minimum: 80, maximum: 125, defaultValue: 100, scopes: userWorkspaceScope, keywords: ['zoom', 'scale', 'display'], owner: 'koren', requiresRestart: false, isAvailable: true },
  { id: 'workbench.toolbarVisible', category: 'Appearance/Interface', title: 'Show toolbar actions', description: 'Show the contextual Run, Search, and Settings actions in the main toolbar.', type: 'boolean', defaultValue: true, scopes: userScope, keywords: ['toolbar', 'buttons'], owner: 'koren', requiresRestart: false, isAvailable: true },
  { id: 'editor.fontSize', category: 'Editor/Font and Typography', title: 'Editor font size', description: 'Code size in pixels. Use Ctrl+= / Ctrl+- to adjust, or Ctrl+0 to reset.', type: 'integer', minimum: 10, maximum: 24, defaultValue: 14, scopes: userWorkspaceScope, keywords: ['font', 'text', 'size', 'zoom', 'shortcut'], owner: 'koren', requiresRestart: false, isAvailable: true },
  { id: 'editor.tabSize', category: 'Editor/Indentation', title: 'Tab size', description: 'Number of columns represented by a tab character.', type: 'integer', minimum: 2, maximum: 8, defaultValue: 4, scopes: userWorkspaceScope, keywords: ['indent', 'spaces'], owner: 'koren', requiresRestart: false, isAvailable: true },
  { id: 'editor.indentSize', category: 'Editor/Indentation', title: 'Indent size', description: 'Columns inserted by one indentation command.', type: 'integer', minimum: 1, maximum: 8, defaultValue: 4, scopes: userWorkspaceScope, keywords: ['indent', 'spaces', 'tab'], owner: 'koren', requiresRestart: false, isAvailable: true },
  { id: 'editor.insertSpaces', category: 'Editor/Indentation', title: 'Insert spaces', description: 'Use spaces instead of tab characters for indentation.', type: 'boolean', defaultValue: true, scopes: userWorkspaceScope, keywords: ['indent', 'tabs'], owner: 'koren', requiresRestart: false, isAvailable: true },
  { id: 'editor.lineNumbers', category: 'Editor/Display', title: 'Show line numbers', description: 'Display line numbers in the editor gutter.', type: 'boolean', defaultValue: true, scopes: userWorkspaceScope, keywords: ['gutter', 'line'], owner: 'koren', requiresRestart: false, isAvailable: true },
  { id: 'editor.wordWrap', category: 'Editor/Display', title: 'Wrap long lines', description: 'Wrap lines that extend beyond the editor width.', type: 'boolean', defaultValue: false, scopes: userWorkspaceScope, keywords: ['wrap', 'line'], owner: 'koren', requiresRestart: false, isAvailable: true },
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
        !Object.values(parsed.workspaces).every(isRecord) ||
        !Object.entries(parsed.user).every(isStoredValueValid) ||
        !Object.values(parsed.workspaces).every((values) => Object.entries(values).every(isStoredValueValid))) return { snapshot: defaultSnapshot(), invalid: true };
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
    const matchesText = !needle || `${descriptor.id} ${descriptor.title} ${descriptor.description} ${descriptor.category} ${descriptor.keywords.join(' ')}`.toLocaleLowerCase().includes(needle);
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
  if (descriptor.type === 'color') return typeof value === 'string' && /^#[\da-f]{6}$/i.test(value);
  return typeof value === 'string' && descriptor.choices.includes(value);
}

function filterValidStoredValues(values: Record<string, unknown>): Record<string, SettingPrimitive> {
  return Object.fromEntries(Object.entries(values).filter(([id, value]) => {
    const descriptor = SETTING_DESCRIPTORS.find((item) => item.id === id);
    return descriptor ? validValue(descriptor, value) : isPrimitive(value);
  })) as Record<string, SettingPrimitive>;
}

function isStoredValueValid([id, value]: [string, unknown]): boolean {
  const descriptor = SETTING_DESCRIPTORS.find((item) => item.id === id);
  return descriptor ? validValue(descriptor, value) : isPrimitive(value);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isPrimitive(value: unknown): value is SettingPrimitive {
  return typeof value === 'boolean' || typeof value === 'string' || (typeof value === 'number' && Number.isFinite(value));
}
