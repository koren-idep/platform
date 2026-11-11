import type { DocumentEditPipeline } from './sync';
export interface SaveResult { revision: number; dirty: boolean }
export async function flushAndSave(pipeline: DocumentEditPipeline, save: (revision: number) => Promise<SaveResult>): Promise<SaveResult> {
  await pipeline.flush();
  return save(pipeline.currentRevision);
}
