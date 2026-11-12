import { describe, expect, it } from 'vitest';
import { filterInspectionFindings, inspectionFindingGroupKey, revisionBoundEdits } from './inspectionPresentation';
import type { InspectionFinding } from '../services/protocol';

function finding(overrides: Partial<InspectionFinding> = {}): InspectionFinding {
  return {
    id: 'finding-1', provider_id: 'org.example.text', rule_id: 'org.example.text.todo', path: 'README.md',
    document_revision: 3, range: { start: { line: 0, character: 0 }, end: { line: 0, character: 4 } },
    severity: 'warning', message: 'TODO remains', explanation: null, category: 'maintenance', related: [],
    actions: [{ id: 'fix', title: 'Replace', provider_id: 'org.example.text', kind: 'quickfix', diagnostic_ids: ['finding-1'],
      edits: [{ path: 'README.md', expected_revision: 3, range: { start: { line: 0, character: 0 }, end: { line: 0, character: 4 } }, new_text: 'DONE' }], disabled_reason: null }],
    origin: 'document', source_id: null, source_revision: null, ...overrides,
  };
}

describe('inspection problem presentation', () => {
  it('filters by severity and groups by provider, category, severity, or file', () => {
    const warning = finding();
    const error = finding({ id: 'finding-2', severity: 'error', provider_id: 'org.example.other', category: null });
    expect(filterInspectionFindings([warning, error], 'error')).toEqual([error]);
    expect(filterInspectionFindings([warning, error], 'all')).toHaveLength(2);
    expect(inspectionFindingGroupKey(warning, 'provider')).toBe('org.example.text');
    expect(inspectionFindingGroupKey(error, 'category')).toBe(error.rule_id);
    expect(inspectionFindingGroupKey(error, 'severity')).toBe('error');
    expect(inspectionFindingGroupKey(warning, 'file', 'README.md')).toBe('README.md');
  });

  it('offers only fixes bound to the current file revision', () => {
    const action = finding().actions[0]!;
    expect(revisionBoundEdits(action, 'README.md', 3)?.[0]?.new_text).toBe('DONE');
    expect(revisionBoundEdits(action, 'README.md', 4)).toBeNull();
    expect(revisionBoundEdits(action, 'src/main.rs', 3)).toBeNull();
    expect(revisionBoundEdits({ ...action, disabled_reason: 'Unsupported edit' }, 'README.md', 3)).toBeNull();
  });
});
