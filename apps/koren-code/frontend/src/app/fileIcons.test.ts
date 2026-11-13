import { describe, expect, it } from 'vitest';
import { fileIconMarkup, fileIconName } from './fileIcons';

describe('shared workspace file icons', () => {
  it('chooses file icons from names without a language service', () => {
    expect(fileIconName('src/main.rs')).toBe('rust');
    expect(fileIconName('settings.json')).toBe('json');
    expect(fileIconName('Cargo.toml')).toBe('toml');
    expect(fileIconName('notes.txt')).toBe('text');
    expect(fileIconName('unknown.bin')).toBe('file');
    expect(fileIconMarkup('src/main.rs', 'file')).toContain('data-file-icon="rust"');
    expect(fileIconMarkup('src/main.rs', 'file')).toContain('m5.8 4.7-3 3.3');
    expect(fileIconMarkup('src', 'directory', true)).toContain('c8a66b');
    expect(fileIconMarkup('linked.rs', 'symlink')).toContain('M6.1 5.1');
  });
});
