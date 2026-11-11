export const PROTOCOL_VERSION = 2 as const;

export type DirectoryEntryKind = 'file' | 'directory' | 'symlink' | 'other';
export interface DirectoryEntry {
  name: string;
  path: string;
  kind: DirectoryEntryKind;
  size_bytes: number;
}
export interface ProjectSummary {
  project_id: number;
  display_name: string;
  root: string;
  provider_id: string;
  configuration_path: string | null;
  state: 'registered' | 'closing';
}
export interface CargoTargetSummary {
  name: string;
  kind: string;
  source_path: string | null;
}
export interface CargoPackageSummary {
  name: string;
  version: string | null;
  manifest_path: string;
  targets: CargoTargetSummary[];
}
export interface CargoProjectSummary {
  project: ProjectSummary;
  is_workspace: boolean;
  package: CargoPackageSummary | null;
  members: CargoPackageSummary[];
}
export interface TaskSummary {
  task_id: number;
  key: string;
  display_name: string;
  category: string;
  executable: string;
  arguments: string[];
  working_directory: string;
  timeout_ms: number | null;
  project_id: number | null;
}
export interface ExecutionApprovalSummary {
  request_id: number;
  task_id: number;
  task_key: string;
  display_name: string;
  executable: string;
  arguments: string[];
  working_directory: string;
  environment_keys: string[];
  project_id: number | null;
  workspace_root: string;
  requesting_actor: string;
  expires_in_ms: number;
}
export type TaskLifecycle = 'pending' | 'running' | 'succeeded' | 'failed' | 'cancelled' | 'timed_out' | 'spawn_failed';
export interface TaskSnapshot {
  run_id: number;
  task_id: number;
  state: TaskLifecycle;
  exit_code: number | null;
  error: string | null;
  stdout: string;
  stderr: string;
  output_truncated: boolean;
}

export type RequestMethod =
  | { method: 'ping' }
  | { method: 'list_commands' }
  | { method: 'execute_command'; params: { command_id: string } }
  | { method: 'open_workspace'; params: { path: string } }
  | { method: 'close_workspace' }
  | { method: 'list_directory'; params: { path: string } }
  | { method: 'open_document'; params: { path: string } }
  | { method: 'read_document'; params: { document_id: number } }
  | { method: 'apply_document_edits'; params: { document_id: number; expected_revision: number; edits: TextEdit[] } }
  | { method: 'save_document'; params: { document_id: number; expected_revision: number } }
  | { method: 'read_document_conflict'; params: { document_id: number } }
  | { method: 'resolve_document_conflict'; params: { document_id: number; expected_revision: number; decision: 'overwrite' | 'reload' } }
  | { method: 'close_document'; params: { document_id: number; discard_unsaved_changes: boolean } }
  | { method: 'list_projects' }
  | { method: 'discover_cargo_projects'; params: { max_directories: number; max_projects: number; max_depth: number } }
  | { method: 'cargo_project_info'; params: { project_id: number } }
  | { method: 'list_tasks' }
  | { method: 'request_task_execution'; params: { task_id: number } }
  | { method: 'list_pending_approvals' }
  | { method: 'approve_task_execution'; params: { request_id: number } }
  | { method: 'deny_task_execution'; params: { request_id: number } }
  | { method: 'task_status'; params: { run_id: number } }
  | { method: 'task_output'; params: { run_id: number } }
  | { method: 'cancel_task'; params: { run_id: number } }
  | { method: 'cleanup_task'; params: { run_id: number } };

export interface TextPosition { line: number; character: number }
export interface TextRange { start: TextPosition; end: TextPosition }
export interface TextEdit { range: TextRange; new_text: string }
export interface RequestEnvelope { version: number; request_id: number; method: RequestMethod }
export interface ProtocolError { code: string; message: string }
export interface ResponseEnvelope {
  version: number;
  request_id: number;
  result: { Ok: ResponseValue } | { Err: ProtocolError };
}
export type ResponseValue =
  | { kind: 'pong' }
  | { kind: 'commands'; value: string[] }
  | { kind: 'command_output'; value: string }
  | { kind: 'workspace_opened'; value: { root: string } }
  | { kind: 'workspace_closed'; value: { root: string } }
  | { kind: 'directory_entries'; value: DirectoryEntry[] }
  | { kind: 'document_opened'; value: { document_id: number; path: string | null; revision: number; dirty: boolean } }
  | { kind: 'document_contents'; value: { document_id: number; contents: string; revision: number; dirty: boolean } }
  | { kind: 'document_changed'; value: { document_id: number; revision: number; dirty: boolean } }
  | { kind: 'document_saved'; value: { document_id: number; revision: number; dirty: boolean } }
  | { kind: 'document_conflict_contents'; value: { document_id: number; path: string | null; contents: string | null } }
  | { kind: 'document_closed'; value: { document_id: number } }
  | { kind: 'projects'; value: ProjectSummary[] }
  | { kind: 'cargo_projects'; value: CargoProjectSummary[] }
  | { kind: 'cargo_project'; value: CargoProjectSummary }
  | { kind: 'tasks'; value: TaskSummary[] }
  | { kind: 'task_started'; value: { run_id: number } }
  | { kind: 'approval_required'; value: ExecutionApprovalSummary }
  | { kind: 'pending_approvals'; value: ExecutionApprovalSummary[] }
  | { kind: 'task_approved'; value: { request_id: number; run_id: number } }
  | { kind: 'task_denied'; value: { request_id: number } }
  | { kind: 'task_snapshot'; value: TaskSnapshot }
  | { kind: 'task_cancelled'; value: { run_id: number } }
  | { kind: 'task_cleaned'; value: { run_id: number } };

export interface PlatformEvent { kind: string; document_id?: number; project_id?: number; run_id?: number; revision?: number; root?: string }
