export interface LayoutPreferences {
  explorerWidth: number;
  bottomHeight: number;
  explorerCollapsed: boolean;
  bottomCollapsed: boolean;
}

export const DEFAULT_LAYOUT: Readonly<LayoutPreferences> = {
  explorerWidth: 232,
  bottomHeight: 144,
  explorerCollapsed: false,
  bottomCollapsed: false,
};

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
    };
  } catch {
    return { ...DEFAULT_LAYOUT };
  }
}

export function persistLayout(storage: Pick<Storage, 'setItem'>, layout: LayoutPreferences): void {
  storage.setItem(KEY, JSON.stringify(layout));
}

function boundedNumber(value: unknown, minimum: number, maximum: number, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value)
    ? Math.min(maximum, Math.max(minimum, value))
    : fallback;
}
