import { describe, expect, it } from 'vitest';
import { absoluteDebugSourcePath } from './debugSource';

describe('debug source paths', () => {
  it('resolves workspace-relative editor paths to absolute source paths', () => {
    expect(absoluteDebugSourcePath('/tmp/project/', 'src/main.rs')).toBe('/tmp/project/src/main.rs');
  });

  it('preserves already absolute Unix and Windows paths', () => {
    expect(absoluteDebugSourcePath('/tmp/project', '/tmp/project/src/main.rs')).toBe('/tmp/project/src/main.rs');
    expect(absoluteDebugSourcePath('C:\\project', 'C:\\project\\src\\main.rs')).toBe('C:/project/src/main.rs');
  });

  it('rejects a relative workspace root', () => {
    expect(() => absoluteDebugSourcePath('project', 'src/main.rs')).toThrow('absolute workspace root');
  });
});
