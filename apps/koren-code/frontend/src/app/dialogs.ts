export type ApprovalChoice = 'approve' | 'deny' | null;
export function approvalChoice(returnValue: string): ApprovalChoice {
  return returnValue === 'approve' || returnValue === 'deny' ? returnValue : null;
}
export type ConflictChoice = 'overwrite' | 'reload' | null;
export function conflictChoice(returnValue: string): ConflictChoice {
  return returnValue === 'overwrite' || returnValue === 'reload' ? returnValue : null;
}
