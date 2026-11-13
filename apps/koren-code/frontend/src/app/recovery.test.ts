import { describe, expect, it } from 'vitest';
import { canRestoreInPlace } from './recovery';

describe('unsaved document recovery', () => {
  it('restores in place only when the backing file still matches its saved base', () => {
    expect(canRestoreInPlace('base-a', 'base-a')).toBe(true);
    expect(canRestoreInPlace('base-a', 'base-b')).toBe(false);
    expect(canRestoreInPlace('base-a', null)).toBe(false);
  });
});
