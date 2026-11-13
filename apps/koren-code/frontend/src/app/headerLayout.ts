export type HeaderMenuName = 'edit' | 'view' | 'navigate' | 'run' | 'tools' | 'help';

const mediumWidthOverflow: HeaderMenuName[] = ['navigate', 'run', 'tools', 'help'];
const narrowWidthOverflow: HeaderMenuName[] = ['edit', 'view', ...mediumWidthOverflow];

export function headerMenusInOverflow(width: number): HeaderMenuName[] {
  if (width < 760) return [...narrowWidthOverflow];
  if (width < 1_100) return [...mediumWidthOverflow];
  return [];
}
