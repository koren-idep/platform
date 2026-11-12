import type { InspectionFinding, TextEdit } from '../services/protocol';

export type InspectionSeverityFilter = 'all' | 'error' | 'warning' | 'information' | 'hint';
export type InspectionGrouping = 'file' | 'provider' | 'category' | 'severity';

export function filterInspectionFindings(
  findings: readonly InspectionFinding[],
  severity: InspectionSeverityFilter,
): InspectionFinding[] {
  return severity === 'all' ? [...findings] : findings.filter((finding) => finding.severity === severity);
}

export function inspectionFindingGroupKey(
  finding: InspectionFinding,
  grouping: InspectionGrouping,
  fileLabel = finding.path,
): string {
  switch (grouping) {
    case 'file': return fileLabel;
    case 'provider': return finding.provider_id;
    case 'category': return finding.category ?? finding.rule_id;
    case 'severity': return finding.severity;
  }
}

export function revisionBoundEdits(
  action: InspectionFinding['actions'][number],
  activePath: string,
  currentRevision: number,
): TextEdit[] | null {
  if (action.disabled_reason || !action.edits.length) return null;
  const supported = action.edits.every((edit) => edit.path === activePath && edit.expected_revision === currentRevision);
  if (!supported) return null;
  return action.edits.map(({ range, new_text }) => ({ range, new_text }));
}
