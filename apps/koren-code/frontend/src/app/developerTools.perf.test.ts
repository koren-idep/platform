import { it } from 'vitest';
import { groupSearchMatches } from './developerTools';
import type { SearchMatch } from '../services/protocol';

it('benchmarks grouping 5,000 search results across 100 files', () => {
  const matches: SearchMatch[] = Array.from({ length: 5000 }, (_, index) => ({
    path: `src/module-${index % 100}.rs`, line: index + 1, column_utf16: index % 80,
    match_length_utf16: 5, preview: `fn example_${index}() {}`,
  }));
  const samples: number[] = [];
  for (let index = 0; index < 50; index++) {
    const start = performance.now(); groupSearchMatches(matches); samples.push(performance.now() - start);
  }
  samples.sort((left, right) => left - right);
  console.log(`5,000 search result grouping: 50 iterations, mean ${(samples.reduce((sum, value) => sum + value, 0) / samples.length).toFixed(3)} ms, median ${samples[Math.floor(samples.length / 2)]!.toFixed(3)} ms`);
});
