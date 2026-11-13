export function canRestoreInPlace(expectedBaseHash: string, currentBaseHash: string | null): boolean {
  return currentBaseHash !== null && currentBaseHash === expectedBaseHash;
}
