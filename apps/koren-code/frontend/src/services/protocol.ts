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
export interface ExtensionSummary {
  id: string; name: string; publisher: string; version: string; description: string;
  enabled: boolean; state: 'disabled' | 'installed' | 'activating' | 'active' | 'failed'; error: string | null;
}
export interface ExtensionCommandSummary { id: string; title: string; description: string; shortcut: string | null; enabled: boolean }
export interface CommandCatalogSummary { id: string; title: string; description: string; shortcut: string | null; enabled: boolean }
export interface ExtensionMenuSummary { command_id: string; label: string; location: string; enabled: boolean }
export type ExtensionUiElement =
  | { kind: 'text' | 'heading'; id: string; text: string }
  | { kind: 'button'; id: string; label: string; command_id: string }
  | { kind: 'list'; id: string; items: string[] }
  | { kind: 'table'; id: string; rows: [string, string][] }
  | { kind: 'input'; id: string; label: string; value: string; required: boolean; max_length: number }
  | { kind: 'select'; id: string; label: string; options: string[]; value: string }
  | { kind: 'checkbox'; id: string; label: string; checked: boolean }
  | { kind: 'status'; id: string; label: string; tone: string };
export interface ExtensionUiDocument { id: string; elements: ExtensionUiElement[] }
export type ExtensionSettingValue = boolean | number | string;
export type ExtensionSettingKind =
  | { type: 'boolean'; default: boolean }
  | { type: 'integer'; default: number; min: number; max: number }
  | { type: 'text'; default: string; max_length: number }
  | { type: 'choice'; default: string; options: string[] };
export interface ExtensionSettingSummary { id: string; title: string; description: string; kind: ExtensionSettingKind; value: ExtensionSettingValue }
export interface ExtensionToolWindowSummary { id: string; title: string; initial_view: ExtensionUiDocument }
export interface ExtensionProjectWizardSummary { id: string; title: string; form: ExtensionUiDocument }
export interface ExtensionThemeSummary { id: string; title: string; base: string; tokens: Record<string, string> }
export interface ExtensionDiagnosticDetailSummary { id: string; source: string | null; code: string | null; title: string; explanation: string; suggested_command: string | null; related: string[] }
export interface ExtensionCatalog { extensions: ExtensionSummary[]; commands: ExtensionCommandSummary[]; built_in_commands: CommandCatalogSummary[]; menu_items: ExtensionMenuSummary[]; language_servers: LanguageServerSummary[]; tool_windows: ExtensionToolWindowSummary[]; settings: ExtensionSettingSummary[]; themes: ExtensionThemeSummary[]; diagnostic_details: ExtensionDiagnosticDetailSummary[]; project_wizards: ExtensionProjectWizardSummary[] }
export interface LanguageServerSummary {
  extension_id: string; configuration_id: string; command: string; arguments: string[];
  languages: string[]; file_extensions: string[]; environment_keys: string[];
}
export interface LanguageServerApprovalSummary {
  approval_id: number; extension_id: string; configuration_id: string; command: string;
  arguments: string[]; working_directory: string; environment_keys: string[];
}
export type DiagnosticSeverity = 'error' | 'warning' | 'information' | 'hint' | null;
export interface LanguageDiagnostic {
  uri: string; range: TextRange; severity: DiagnosticSeverity; message: string; source: string | null;
  code: string | null; related_information: Array<{ uri: string; range: TextRange; message: string }>;
  document_revision: number | null; document_version: number | null;
}
export interface LanguageDiagnosticSet {
  provider_id: string; uri: string; document_version: number | null; document_id: number | null; diagnostics: LanguageDiagnostic[];
}
export interface InspectionFinding {
  id: string; provider_id: string; rule_id: string; path: string; document_revision: number | null;
  range: TextRange; severity: 'error' | 'warning' | 'information' | 'hint'; message: string;
  explanation: string | null; category: string | null;
  related: Array<{ path: string; range: TextRange; message: string }>;
  actions: Array<{ id: string; title: string; provider_id: string; kind: string; diagnostic_ids: string[];
    edits: Array<{ path: string; expected_revision: number | null; range: TextRange; new_text: string }>;
    disabled_reason: string | null }>;
  origin: string;
  source_id: string | null;
  source_revision: string | null;
}
export interface InspectionRuleSummary {
  id: string; provider_id: string; title: string; description: string; category: string;
  languages: string[]; default_severity: 'error' | 'warning' | 'information' | 'hint';
  enabled: boolean; severity: 'error' | 'warning' | 'information' | 'hint'; workspace_override: boolean;
}
export interface InspectionConfigurationSummary {
  profiles: string[]; selected_profile: string; workspace_specific: boolean; rules: InspectionRuleSummary[];
}
export interface ProjectInspectionSummary {
  job_id: number; state: string; completed_documents: number; total_documents: number;
  current_path: string | null; result_count: number; error: string | null;
}

export type RequestMethod =
  | { method: 'ping' }
  | { method: 'list_commands' }
  | { method: 'execute_command'; params: { command_id: string } }
  | { method: 'list_extensions' }
  | { method: 'set_extension_setting'; params: { setting_id: string; value: ExtensionSettingValue } }
  | { method: 'list_inspection_rules' }
  | { method: 'select_inspection_profile'; params: { profile_name: string; workspace_specific: boolean } }
  | { method: 'create_inspection_profile'; params: { profile_name: string; workspace_specific: boolean } }
  | { method: 'set_inspection_rule'; params: { rule_id: string; enabled: boolean; severity: string | null; workspace_specific: boolean; reset: boolean } }
  | { method: 'import_sarif'; params: { path: string } }
  | { method: 'export_sarif' }
  | { method: 'start_project_inspection' }
  | { method: 'project_inspection_status'; params: { job_id: number } }
  | { method: 'cancel_project_inspection'; params: { job_id: number } }
  | { method: 'project_inspection_findings'; params: { job_id: number; offset: number; limit: number } }
  | { method: 'set_extension_enabled'; params: { extension_id: string; enabled: boolean } }
  | { method: 'activate_extension'; params: { extension_id: string } }
  | { method: 'start_language_server'; params: { extension_id: string; configuration_id: string } }
  | { method: 'approve_language_server'; params: { approval_id: number } }
  | { method: 'deny_language_server'; params: { approval_id: number } }
  | { method: 'stop_language_server'; params: { server_id: number } }
  | { method: 'language_hover'; params: { document_id: number; expected_revision: number; position: TextPosition } }
  | { method: 'language_completion'; params: { document_id: number; expected_revision: number; position: TextPosition } }
  | { method: 'language_definition'; params: { document_id: number; expected_revision: number; position: TextPosition } }
  | { method: 'language_references'; params: { document_id: number; expected_revision: number; position: TextPosition; include_declaration: boolean } }
  | { method: 'language_format'; params: { document_id: number; expected_revision: number } }
  | { method: 'language_code_actions'; params: { document_id: number; expected_revision: number; range: TextRange } }
  | { method: 'language_rename'; params: { document_id: number; expected_revision: number; position: TextPosition; new_name: string } }
  | { method: 'inspect_document'; params: { document_id: number; expected_revision: number; language: string } }
  | { method: 'open_workspace'; params: { path: string } }
  | { method: 'close_workspace' }
  | { method: 'list_directory'; params: { path: string } }
  | { method: 'create_workspace_file'; params: { path: string } }
  | { method: 'create_workspace_directory'; params: { path: string } }
  | { method: 'rename_workspace_entry'; params: { source: string; destination: string } }
  | { method: 'delete_workspace_entry'; params: { path: string; recursive: boolean; confirmed: boolean } }
  | { method: 'workspace_entry_metadata'; params: { path: string } }
  | { method: 'discover_git_repositories' }
  | { method: 'git_status'; params: { path: string } }
  | { method: 'git_diff'; params: { repository: string; path: string; staged: boolean } }
  | { method: 'git_stage'; params: { repository: string; path: string } }
  | { method: 'git_unstage'; params: { repository: string; path: string } }
  | { method: 'git_commit'; params: { repository: string; message: string } }
  | { method: 'start_workspace_search'; params: { query: string; case_sensitive: boolean; regular_expression: boolean; file_filter: string | null } }
  | { method: 'cancel_workspace_search'; params: { job_id: number } }
  | { method: 'poll_workspace_search'; params: { job_id: number } }
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
  | { method: 'cleanup_task'; params: { run_id: number } }
  | { method: 'request_terminal_start' }
  | { method: 'list_terminal_sessions' }
  | { method: 'approve_terminal_start'; params: { request_id: number } }
  | { method: 'deny_terminal_start'; params: { request_id: number } }
  | { method: 'terminal_write'; params: { session_id: number; input_base64: string } }
  | { method: 'terminal_resize'; params: { session_id: number; rows: number; columns: number } }
  | { method: 'close_terminal'; params: { session_id: number; terminate_active: boolean } };

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
  | { kind: 'extensions'; value: ExtensionCatalog }
  | { kind: 'extension_setting_updated'; value: { setting_id: string } }
  | { kind: 'inspection_configuration'; value: InspectionConfigurationSummary }
  | { kind: 'imported_sarif'; value: { report_id: string; findings: InspectionFinding[]; warnings: string[] } }
  | { kind: 'sarif_report'; value: { document: string; warnings: string[] } }
  | { kind: 'project_inspection_started'; value: { job_id: number } }
  | { kind: 'project_inspection_status'; value: ProjectInspectionSummary }
  | { kind: 'project_inspection_findings'; value: { job_id: number; offset: number; next_offset: number | null; findings: InspectionFinding[] } }
  | { kind: 'project_inspection_cancelled'; value: { job_id: number } }
  | { kind: 'extension_changed'; value: ExtensionSummary }
  | { kind: 'language_servers'; value: LanguageServerSummary[] }
  | { kind: 'language_server_approval_required'; value: LanguageServerApprovalSummary }
  | { kind: 'language_server_started'; value: { server_id: number; extension_id: string; configuration_id: string } }
  | { kind: 'language_server_denied'; value: { approval_id: number } }
  | { kind: 'language_server_stopped'; value: { server_id: number } }
  | { kind: 'language_hover'; value: { document_revision: number; contents: string | null; range: TextRange | null } }
  | { kind: 'language_completions'; value: { document_revision: number; items: Array<{ label: string; detail: string | null; insert_text: string | null; documentation: string | null }> } }
  | { kind: 'language_definitions'; value: { document_revision: number; locations: Array<{ uri: string; range: TextRange }> } }
  | { kind: 'language_references'; value: { document_revision: number; locations: Array<{ uri: string; range: TextRange }> } }
  | { kind: 'language_formatting'; value: { document_revision: number; edits: TextEdit[] } }
  | { kind: 'language_code_actions'; value: { document_revision: number; actions: Array<{ id: string; title: string; provider_id: string; kind: string | null; related_diagnostics: string[]; edits: TextEdit[] | null; unsupported_reason: string | null; disabled_reason: string | null }> } }
  | { kind: 'language_rename'; value: { document_revision: number; edits: TextEdit[] } }
  | { kind: 'inspection_findings'; value: { document_revision: number; findings: InspectionFinding[] } }
  | { kind: 'workspace_opened'; value: { root: string } }
  | { kind: 'workspace_closed'; value: { root: string } }
  | { kind: 'directory_entries'; value: DirectoryEntry[] }
  | { kind: 'workspace_entry'; value: DirectoryEntry }
  | { kind: 'workspace_entry_changed'; value: { operation: string; path: string; previous_path: string | null } }
  | { kind: 'git_repositories'; value: GitRepositorySummary[] }
  | { kind: 'git_status'; value: GitStatusSummary }
  | { kind: 'git_diff'; value: { path: string; staged: boolean; text: string } }
  | { kind: 'git_operation'; value: { operation: string; commit: string | null } }
  | { kind: 'search_started'; value: { job_id: number } }
  | { kind: 'search_batch'; value: { job_id: number; matches: SearchMatch[]; progress: SearchProgress | null; complete: boolean; summary: SearchSummary | null; error: string | null } }
  | { kind: 'search_cancelled'; value: { job_id: number } }
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
  | { kind: 'task_cleaned'; value: { run_id: number } }
  | { kind: 'terminal_approval_required'; value: TerminalApprovalSummary }
  | { kind: 'terminal_sessions'; value: TerminalSessionSummary[] }
  | { kind: 'terminal_approved'; value: { request_id: number; session: TerminalSessionSummary } }
  | { kind: 'terminal_denied'; value: { request_id: number } }
  | { kind: 'terminal_action'; value: { session_id: number } };

export interface GitRepositorySummary { root: string; branch: string | null; has_head: boolean }
export interface GitChangedFile { path: string; staged: boolean; unstaged: boolean; untracked: boolean; deleted: boolean; conflicted: boolean }
export interface GitStatusSummary { root: string; branch: string | null; has_head: boolean; files: GitChangedFile[] }
export interface SearchMatch { path: string; line: number; column_utf16: number; match_length_utf16: number; preview: string }
export interface SearchProgress { files_scanned: number; matches_found: number; current_path: string | null }
export interface SearchSummary { files_scanned: number; matches_found: number; skipped_binary_files: number; truncated: boolean; cancelled: boolean }
export interface TerminalApprovalSummary {
  request_id: number; profile_id: string; profile_name: string; executable: string; arguments: string[];
  working_directory: string; environment_keys: string[]; fallback_executable: string | null;
  actor: string; workspace: string | null;
}
export interface TerminalSessionSummary {
  session_id: number; profile_id: string; working_directory: string;
  state: 'created' | 'starting' | 'running' | 'exited' | 'failed' | 'terminated';
  exit_code: number | null; process_group_active: boolean; error: string | null; rows: number; columns: number;
}
export interface PlatformEvent {
  kind: string; document_id?: number; project_id?: number; run_id?: number; server_id?: number; revision?: number;
  root?: string; path?: string; previous_path?: string; operation?: string; commit?: string;
  diagnostics?: LanguageDiagnosticSet; message?: string; terminal_id?: number; sequence?: number;
  data_base64?: string; terminal_state?: TerminalSessionSummary['state']; exit_code?: number; terminal_process_group_active?: boolean;
}
