export function encodeTerminalBytes(bytes: Uint8Array): string {
  let binary = '';
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  return btoa(binary);
}

export function decodeTerminalBytes(value: string): Uint8Array {
  const binary = atob(value);
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}

export function terminalSequenceOrder(previous: number, next: number): 'accepted' | 'duplicate' | 'gap' {
  if (next <= previous) return 'duplicate';
  return next === previous + 1 ? 'accepted' : 'gap';
}
