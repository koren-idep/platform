export interface LayoutPreferences {
  explorerWidth: number;
  bottomHeight: number;
  explorerCollapsed: boolean;
  bottomCollapsed: boolean;
  primaryView: 'files' | 'search' | 'git' | 'extensions';
  secondaryWidth: number;
  secondaryCollapsed: boolean;
}

export const DEFAULT_LAYOUT: Readonly<LayoutPreferences> = {
  explorerWidth: 380,
  bottomHeight: 220,
  explorerCollapsed: false,
  bottomCollapsed: true,
  primaryView: 'files',
  secondaryWidth: 300,
  secondaryCollapsed: true,
};

export function sidebarContentId(view: LayoutPreferences['primaryView']): string {
  return view === 'files' ? 'tree' : view === 'git' ? 'source-control' : view === 'extensions' ? 'sidebar-extensions' : 'search';
}

const KEY = 'koren-code:layout';

export function readLayout(storage: Pick<Storage, 'getItem'>): LayoutPreferences {
  const saved = storage.getItem(KEY);
  if (!saved) return { ...DEFAULT_LAYOUT };
  try {
    const value = JSON.parse(saved) as Partial<LayoutPreferences>;
    return {
      explorerWidth: boundedNumber(value.explorerWidth, 140, 480, DEFAULT_LAYOUT.explorerWidth),
      bottomHeight: boundedNumber(value.bottomHeight, 90, 480, DEFAULT_LAYOUT.bottomHeight),
      explorerCollapsed: value.explorerCollapsed === true,
      bottomCollapsed: value.bottomCollapsed === true,
      primaryView: value.primaryView === 'search' || value.primaryView === 'git' || value.primaryView === 'extensions' ? value.primaryView : 'files',
      secondaryWidth: boundedNumber(value.secondaryWidth, 180, 480, DEFAULT_LAYOUT.secondaryWidth),
      secondaryCollapsed: value.secondaryCollapsed !== false,
    };
  } catch {
    return { ...DEFAULT_LAYOUT };
  }
}

export function persistLayout(storage: Pick<Storage, 'setItem'>, layout: LayoutPreferences): void {
  storage.setItem(KEY, JSON.stringify(layout));
}

export function fitLayoutToViewport(layout: LayoutPreferences, width: number, height: number): LayoutPreferences {
  const usableWidth = Math.max(320, width);
  const usableHeight = Math.max(240, height);
  return {
    ...layout,
    explorerWidth: Math.round(Math.min(layout.explorerWidth, Math.max(148, usableWidth * 0.42))),
    bottomHeight: Math.round(Math.min(layout.bottomHeight, Math.max(96, Math.min(480, usableHeight * 0.5)))),
    secondaryWidth: Math.round(Math.min(layout.secondaryWidth, Math.max(180, usableWidth * 0.32))),
  };
}

function boundedNumber(value: unknown, minimum: number, maximum: number, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value)
    ? Math.min(maximum, Math.max(minimum, value))
    : fallback;
}
