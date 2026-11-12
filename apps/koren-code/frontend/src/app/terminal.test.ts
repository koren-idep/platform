import { describe, expect, it } from 'vitest';
import { decodeTerminalBytes, encodeTerminalBytes, terminalSequenceOrder } from './terminalTransport';

describe('terminal transport helpers', () => {
  it('preserves arbitrary binary output through base64 encoding', () => {
    const bytes = Uint8Array.from([0, 10, 13, 27, 127, 128, 255]);
    expect(decodeTerminalBytes(encodeTerminalBytes(bytes))).toEqual(bytes);
  });

  it('retains event ordering and identifies duplicates or missing chunks', () => {
    expect(terminalSequenceOrder(0, 1)).toBe('accepted');
    expect(terminalSequenceOrder(1, 1)).toBe('duplicate');
    expect(terminalSequenceOrder(1, 3)).toBe('gap');
  });
});
