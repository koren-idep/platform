import { describe, expect, it } from 'vitest';
import { headerMenusInOverflow } from './headerLayout';

describe('responsive application header', () => {
  it('keeps every menu visible when the header has room', () => {
    expect(headerMenusInOverflow(1280)).toEqual([]);
  });

  it('moves only lower priority menus into More at medium widths', () => {
    expect(headerMenusInOverflow(1000)).toEqual(['navigate', 'run', 'tools', 'help']);
  });

  it('preserves File and moves the remaining menu labels into More on narrow windows', () => {
    expect(headerMenusInOverflow(720)).toEqual(['edit', 'view', 'navigate', 'run', 'tools', 'help']);
  });
});
