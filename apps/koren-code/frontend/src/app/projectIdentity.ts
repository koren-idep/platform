export const PROJECT_COLORS = [
  { name: 'Sky Blue', value: '#3794ff' },
  { name: 'Indigo', value: '#7967e8' },
  { name: 'Violet', value: '#a16ae8' },
  { name: 'Mint', value: '#41b883' },
  { name: 'Amber', value: '#d99227' },
  { name: 'Rose', value: '#df6382' },
  { name: 'Slate', value: '#7f8c9f' },
  { name: 'Orange', value: '#e07a35' },
] as const;

export function automaticProjectColor(workspace: string): string {
  let hash = 2166136261;
  for (const character of workspace.replaceAll('\\', '/').toLocaleLowerCase()) {
    hash = Math.imul(hash ^ character.codePointAt(0)!, 16777619);
  }
  return PROJECT_COLORS[(hash >>> 0) % PROJECT_COLORS.length]!.value;
}

export function isProjectColor(value: string): boolean {
  return /^#[\da-f]{6}$/i.test(value);
}

export function colorContrastRatio(hex: string): { light: number; dark: number } {
  const channels = hex.slice(1).match(/.{2}/g)?.map((channel) => Number.parseInt(channel, 16) / 255);
  if (!channels || channels.length !== 3) return { light: 0, dark: 0 };
  const linear = channels.map((channel) => channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4);
  const luminance = 0.2126 * linear[0]! + 0.7152 * linear[1]! + 0.0722 * linear[2]!;
  return { light: 1.05 / (luminance + 0.05), dark: (luminance + 0.05) / 0.05 };
}

export function hasReadableProjectAccent(hex: string): boolean {
  if (!isProjectColor(hex)) return false;
  const ratio = colorContrastRatio(hex);
  return Math.max(ratio.light, ratio.dark) >= 3;
}
