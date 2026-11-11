import { describe, expect, it } from 'vitest';
import { diagnosticsForRevision, withDiagnosticPublication } from './diagnostics';
import type { LanguageDiagnosticSet } from '../services/protocol';

function publication(provider_id: string, revision: number, message: string): LanguageDiagnosticSet {
  return {
    provider_id, uri: 'file:///demo', document_id: 7, document_version: revision, diagnostics: [{
      uri: 'file:///demo', range: { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } },
      severity: 'warning', message, source: provider_id, code: null, related_information: [],
      document_revision: revision, document_version: revision,
    }],
  };
}

describe('language diagnostics', () => {
  it('preserves independent providers and filters obsolete document revisions', () => {
    let sets = withDiagnosticPublication(undefined, publication('server-a', 2, 'A'));
    sets = withDiagnosticPublication(sets, publication('inspections', 2, 'B'));
    expect(diagnosticsForRevision(sets, 2).map((entry) => entry.diagnostic.message)).toEqual(['A', 'B']);
    expect(diagnosticsForRevision(sets, 3)).toEqual([]);
  });

  it('replaces only the provider that published a fresh empty result', () => {
    let sets = withDiagnosticPublication(undefined, publication('server-a', 1, 'old'));
    sets = withDiagnosticPublication(sets, publication('server-b', 2, 'keep'));
    sets = withDiagnosticPublication(sets, { ...publication('server-a', 2, ''), diagnostics: [] });
    expect(diagnosticsForRevision(sets, 2).map((entry) => entry.provider)).toEqual(['server-b']);
  });
});
