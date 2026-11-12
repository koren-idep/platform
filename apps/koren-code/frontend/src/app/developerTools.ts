import type { GitChangedFile, SearchMatch, SearchSummary } from '../services/protocol';

export function groupSearchMatches(matches: readonly SearchMatch[]): Map<string, SearchMatch[]> {
  const groups = new Map<string, SearchMatch[]>();
  for (const match of matches) {
    const values = groups.get(match.path) ?? [];
    values.push(match);
    groups.set(match.path, values);
  }
  return groups;
}

export function partitionGitFiles(files: readonly GitChangedFile[]): { staged: GitChangedFile[]; unstaged: GitChangedFile[] } {
  return {
    staged: files.filter((file) => file.staged),
    unstaged: files.filter((file) => file.unstaged || file.untracked),
  };
}

export function searchSummaryText(summary: SearchSummary): string {
  return `${summary.matches_found} matches in ${summary.files_scanned} files${summary.cancelled ? ' · cancelled' : ''}${summary.truncated ? ' · result limit reached' : ''}${summary.skipped_binary_files ? ` · ${summary.skipped_binary_files} binary files skipped` : ''}`;
}

export function normalizedCommitMessage(message: string): string | null {
  const normalized = message.trim();
  return normalized.length ? normalized : null;
}
