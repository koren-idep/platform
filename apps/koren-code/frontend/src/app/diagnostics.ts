import type { LanguageDiagnostic, LanguageDiagnosticSet } from '../services/protocol';

export function withDiagnosticPublication(
  current: ReadonlyMap<string, LanguageDiagnosticSet> | undefined,
  publication: LanguageDiagnosticSet,
): Map<string, LanguageDiagnosticSet> {
  const next = new Map(current ?? []);
  next.set(publication.provider_id, publication);
  return next;
}

export function diagnosticsForRevision(
  providers: ReadonlyMap<string, LanguageDiagnosticSet> | undefined,
  revision: number,
): Array<{ provider: string; diagnostic: LanguageDiagnostic }> {
  if (!providers) return [];
  return [...providers.values()].flatMap((set) => set.diagnostics
    .filter((diagnostic) => diagnostic.document_revision === revision)
    .map((diagnostic) => ({ provider: set.provider_id, diagnostic })));
}
