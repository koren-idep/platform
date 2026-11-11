export type Theme = 'dark' | 'light';
export function nextTheme(current: Theme): Theme { return current === 'dark' ? 'light' : 'dark'; }
export function readTheme(storage: Pick<Storage, 'getItem'>): Theme {
  return storage.getItem('koren-code:theme') === 'light' ? 'light' : 'dark';
}
export function persistTheme(storage: Pick<Storage, 'setItem'>, theme: Theme): void { storage.setItem('koren-code:theme', theme); }
