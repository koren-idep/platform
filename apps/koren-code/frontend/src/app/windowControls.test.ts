import { describe, expect, it } from 'vitest';
import { windowActionMessage } from './windowControls';

describe('custom window controls', () => {
  it('encodes actions for the native window host', () => {
    expect(windowActionMessage('minimize')).toBe('{"type":"window_action","action":"minimize"}');
    expect(windowActionMessage('toggle_maximize')).toBe('{"type":"window_action","action":"toggle_maximize"}');
    expect(windowActionMessage('close')).toBe('{"type":"window_action","action":"close"}');
    expect(windowActionMessage('drag')).toBe('{"type":"window_action","action":"drag"}');
  });
});
