import { describe, expect, it } from 'vitest';
import { selectedLocation } from './languageServices';

describe('language definition location selection', () => {
  it('uses the only result without prompting', () => {
    expect(selectedLocation(['one'], null)).toBe('one');
  });

  it('accepts a valid one-based choice and ignores invalid or canceled choices', () => {
    expect(selectedLocation(['one', 'two'], '2')).toBe('two');
    expect(selectedLocation(['one', 'two'], null)).toBeUndefined();
    expect(selectedLocation(['one', 'two'], '3')).toBeUndefined();
    expect(selectedLocation(['one', 'two'], '1.5')).toBeUndefined();
  });
});
