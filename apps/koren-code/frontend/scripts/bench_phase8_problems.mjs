// Microbench the production Problems filtering helper and the state publication operation.
import { filterInspectionFindings } from '../src/app/inspectionPresentation.ts';

const findings = Array.from({ length: 1000 }, (_, index) => ({
  id: `bench:${index}`, provider_id: `provider.${index % 8}`, rule_id: `rule.${index % 24}`,
  path: `src/file-${index % 40}.txt`, document_revision: 1,
  range: { start: { line: index % 200, character: 0 }, end: { line: index % 200, character: 4 } },
  severity: ['error', 'warning', 'information', 'hint'][index % 4],
  message: `Finding ${index}`, explanation: null, category: `category.${index % 6}`,
  related: [], actions: [], origin: 'document', source_id: null, source_revision: null,
}));
const cache = new Map();

function measure(label, operation, iterations = 10_000) {
  for (let index = 0; index < 500; index += 1) operation();
  const samples = [];
  for (let index = 0; index < iterations; index += 1) {
    const start = performance.now(); operation(); samples.push((performance.now() - start) * 1000);
  }
  const mean = samples.reduce((sum, value) => sum + value, 0) / samples.length;
  console.log(`${label},iterations=${iterations},mean_us=${mean.toFixed(3)},min_us=${Math.min(...samples).toFixed(3)}`);
}

measure('frontend_diagnostic_publication_1000_findings', () => { cache.set(1, findings); });
measure('problems_filter_warning_1000_findings', () => {
  const visible = filterInspectionFindings(findings, 'warning');
  globalThis.__benchSink = visible.length;
});
