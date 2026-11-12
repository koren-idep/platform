import { describe, expect, it } from 'vitest';
import { gitRepositoryChoices, groupSearchMatches, normalizedCommitMessage, partitionGitFiles, searchSummaryText } from './developerTools';
import type { GitChangedFile, SearchMatch } from '../services/protocol';

const match = (path: string, line: number): SearchMatch => ({ path, line, column_utf16: 0, match_length_utf16: 1, preview: 'x' });
const file = (path: string, values: Partial<GitChangedFile>): GitChangedFile => ({ path, staged: false, unstaged: false, untracked: false, deleted: false, conflicted: false, ...values });

describe('developer tool presentations', () => {
  it('groups incremental search matches by source file while preserving order', () => {
    const grouped = groupSearchMatches([match('src/a.rs', 2), match('src/b.rs', 1), match('src/a.rs', 7)]);
    expect([...grouped.keys()]).toEqual(['src/a.rs', 'src/b.rs']);
    expect(grouped.get('src/a.rs')?.map((item) => item.line)).toEqual([2, 7]);
  });

  it('shows staged and unstaged sides independently for partially staged files', () => {
    const split = partitionGitFiles([
      file('both.txt', { staged: true, unstaged: true }),
      file('new.txt', { untracked: true }),
      file('staged.txt', { staged: true }),
    ]);
    expect(split.staged.map((item) => item.path)).toEqual(['both.txt', 'staged.txt']);
    expect(split.unstaged.map((item) => item.path)).toEqual(['both.txt', 'new.txt']);
  });

  it('keeps the workspace-root Git repository selectable alongside nested repositories', () => {
    const choices = gitRepositoryChoices('/work/koren/', [
      { root: '/work/koren', branch: 'main' },
      { root: '/work/koren/examples/project', branch: null },
    ]);
    expect(choices).toEqual([
      { path: '.', label: './main' },
      { path: 'examples/project', label: 'examples/project/(detached)' },
    ]);
    expect(new Set(choices.map((choice) => choice.path)).size).toBe(choices.length);
  });

  it('normalizes explicit commit messages and reports bounded search outcomes', () => {
    expect(normalizedCommitMessage('  release notes  ')).toBe('release notes');
    expect(normalizedCommitMessage('  ')).toBeNull();
    expect(searchSummaryText({ matches_found: 2, files_scanned: 4, skipped_binary_files: 1, truncated: true, cancelled: false }))
      .toBe('2 matches in 4 files · result limit reached · 1 binary files skipped');
  });
});
