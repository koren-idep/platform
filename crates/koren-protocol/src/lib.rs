//! Experimental, strongly typed internal request and response contracts.

use serde::{Deserialize, Serialize};

/// Experimental protocol version after adding workspace and document operations.
pub const PROTOCOL_VERSION: u16 = 2;

/// A zero-based line and UTF-16 code-unit column.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TextPosition {
    /// Zero-based logical line.
    pub line: usize,
    /// Zero-based UTF-16 code-unit column; surrogate pairs occupy two units.
    pub character: usize,
}

/// Half-open text range in UTF-16 editor coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TextRange {
    /// Inclusive start position.
    pub start: TextPosition,
    /// Exclusive end position.
    pub end: TextPosition,
}

/// One replacement edit against a document snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TextEdit {
    /// Range to replace.
    pub range: TextRange,
    /// Replacement UTF-8 text.
    pub new_text: String,
}

/// Explicit choice for resolving a detected external file conflict.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictDecision {
    /// Replace the exact external version previously observed during conflict detection.
    Overwrite,
    /// Discard the in-memory edits and load the current file contents.
    Reload,
}

/// Supported request operations in protocol version 2. Project/task operations are additive
/// extensions of the tagged request contract and do not alter existing version-2 shapes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum RequestMethod {
    /// Check that the platform is responsive.
    Ping,
    /// Enumerate commands registered by the host.
    ListCommands,
    /// Invoke a registered command without arguments.
    ExecuteCommand {
        /// Identifier of the command to invoke.
        command_id: String,
    },
    /// List installed extensions and their active command contributions.
    ListExtensions,
    /// Update a validated namespaced extension setting.
    SetExtensionSetting {
        /// Fully qualified setting identifier.
        setting_id: String,
        /// JSON-compatible primitive value.
        value: ExtensionSettingValue,
    },
    /// List inspection profiles and rules currently registered by enabled providers.
    ListInspectionRules,
    /// Select an existing inspection profile globally or for the active workspace.
    SelectInspectionProfile {
        /// Existing profile name.
        profile_name: String,
        /// Apply this choice only to the current workspace.
        workspace_specific: bool,
    },
    /// Create and select a named inspection profile.
    CreateInspectionProfile {
        /// New profile display name.
        profile_name: String,
        /// Select this profile only for the active workspace.
        workspace_specific: bool,
    },
    /// Update one registered rule in the selected profile, or reset it to provider defaults.
    SetInspectionRule {
        /// Stable registered rule ID.
        rule_id: String,
        /// Enable the rule.
        enabled: bool,
        /// Optional severity override.
        severity: Option<String>,
        /// Apply this override only to the current workspace.
        workspace_specific: bool,
        /// Remove the override and restore provider defaults.
        reset: bool,
    },
    /// Import a user-selected local SARIF file.
    ImportSarif {
        /// Absolute path returned by the native file chooser.
        path: String,
    },
    /// Export the currently fresh inspection results as SARIF 2.1.0.
    ExportSarif,
    /// Start an explicit inspection job over the currently open workspace documents.
    StartProjectInspection,
    /// Read the progress summary for a started inspection job.
    ProjectInspectionStatus {
        /// Job identity.
        job_id: u64,
    },
    /// Request cancellation of an active project inspection job.
    CancelProjectInspection {
        /// Job identity.
        job_id: u64,
    },
    /// Read one bounded page of project inspection findings.
    ProjectInspectionFindings {
        /// Job identity.
        job_id: u64,
        /// First result offset.
        offset: usize,
        /// Maximum result count, capped by the host.
        limit: usize,
    },
    /// Explicitly enable an installed extension without executing its code.
    SetExtensionEnabled {
        /// Reverse-DNS extension identifier.
        extension_id: String,
        /// Whether the extension should be enabled.
        enabled: bool,
    },
    /// Explicitly execute an enabled extension's Lua activation entry point.
    ActivateExtension {
        /// Reverse-DNS extension identifier.
        extension_id: String,
    },
    /// Request startup of one declared language server.
    StartLanguageServer {
        /// Declaring extension.
        extension_id: String,
        /// Configuration identifier from its manifest.
        configuration_id: String,
    },
    /// Approve and consume one exact language-server launch request.
    ApproveLanguageServer {
        /// One-time approval identifier.
        approval_id: u64,
    },
    /// Deny and consume one language-server launch request.
    DenyLanguageServer {
        /// One-time approval identifier.
        approval_id: u64,
    },
    /// Gracefully stop one active language server.
    StopLanguageServer {
        /// Server identifier returned after startup.
        server_id: u64,
    },
    /// Request hover from a server synchronized with the stated revision.
    LanguageHover {
        /// Open document identifier.
        document_id: u64,
        /// Revision shown by the editor.
        expected_revision: u64,
        /// UTF-16 document position.
        position: TextPosition,
    },
    /// Request completions from a server synchronized with the stated revision.
    LanguageCompletion {
        /// Open document identifier.
        document_id: u64,
        /// Revision shown by the editor.
        expected_revision: u64,
        /// UTF-16 document position.
        position: TextPosition,
    },
    /// Request definition locations from a synchronized language server.
    LanguageDefinition {
        /// Open document identifier.
        document_id: u64,
        /// Revision shown by the editor.
        expected_revision: u64,
        /// UTF-16 document position.
        position: TextPosition,
    },
    /// Find symbol references through a capable language server.
    LanguageReferences {
        /// Open document identifier.
        document_id: u64,
        /// Revision shown by the editor.
        expected_revision: u64,
        /// UTF-16 symbol position.
        position: TextPosition,
        /// Include the declaration in results.
        include_declaration: bool,
    },
    /// Format the current document through a capable language server.
    LanguageFormat {
        /// Open document identifier.
        document_id: u64,
        /// Revision shown by the editor.
        expected_revision: u64,
    },
    /// Request code actions and intentions for a selected range.
    LanguageCodeActions {
        /// Open document identifier.
        document_id: u64,
        /// Revision shown by the editor.
        expected_revision: u64,
        /// Selected UTF-16 range; an empty range requests intentions.
        range: TextRange,
    },
    /// Request a symbol rename proposal from the active language server.
    LanguageRename {
        /// Open document identifier.
        document_id: u64,
        /// Revision shown by the editor.
        expected_revision: u64,
        /// UTF-16 symbol position.
        position: TextPosition,
        /// New symbol name.
        new_name: String,
    },
    /// Explicitly inspect one revisioned document with registered local providers.
    InspectDocument {
        /// Open document identifier.
        document_id: u64,
        /// Revision shown by the editor.
        expected_revision: u64,
        /// Language identifier selected by the editor integration.
        language: String,
    },
    /// Read information about a local directory.
    WorkspaceInfo {
        /// Local filesystem path to inspect.
        path: String,
    },
    /// Open a workspace root.
    OpenWorkspace {
        /// Local filesystem path to the root.
        path: String,
    },
    /// Close the open workspace after its documents have been closed.
    CloseWorkspace,
    /// List one workspace-relative directory without recursion.
    ListDirectory {
        /// Relative directory path; empty or `.` means the workspace root.
        path: String,
    },
    /// Create an empty file without replacing an existing workspace entry.
    CreateWorkspaceFile {
        /// Relative destination path.
        path: String,
    },
    /// Create one directory without replacing an existing workspace entry.
    CreateWorkspaceDirectory {
        /// Relative destination path.
        path: String,
    },
    /// Move or rename a workspace entry without overwriting an existing destination.
    RenameWorkspaceEntry {
        /// Existing relative source path.
        source: String,
        /// New relative destination path.
        destination: String,
    },
    /// Delete an entry after explicit user confirmation.
    DeleteWorkspaceEntry {
        /// Existing relative source path.
        path: String,
        /// Recursively delete a directory's contents.
        recursive: bool,
        /// Confirmation from the UI; false always rejects the operation.
        confirmed: bool,
    },
    /// Query no-follow metadata for one existing entry.
    WorkspaceEntryMetadata {
        /// Existing relative path.
        path: String,
    },
    /// Find local Git repositories in the active workspace without starting Git processes.
    DiscoverGitRepositories,
    /// Read status for the repository at or above a workspace-relative path.
    GitStatus {
        /// Workspace-relative path inside the desired repository.
        path: String,
    },
    /// Read a bounded text diff for one repository-relative path.
    GitDiff {
        /// Workspace-relative repository root or descendant path.
        repository: String,
        /// Repository-relative file path.
        path: String,
        /// Compare the index to HEAD instead of the working tree.
        staged: bool,
    },
    /// Stage exactly one path in a local repository.
    GitStage {
        /// Workspace-relative repository root or descendant path.
        repository: String,
        /// Repository-relative file path.
        path: String,
    },
    /// Unstage exactly one path in a local repository.
    GitUnstage {
        /// Workspace-relative repository root or descendant path.
        repository: String,
        /// Repository-relative file path.
        path: String,
    },
    /// Create a local commit from explicitly staged changes.
    GitCommit {
        /// Workspace-relative repository root or descendant path.
        repository: String,
        /// Commit message entered by the user.
        message: String,
    },
    /// Start an explicit bounded workspace text search.
    StartWorkspaceSearch {
        /// Literal text or regular expression.
        query: String,
        /// Match case distinctly.
        case_sensitive: bool,
        /// Treat the query as a regular expression.
        regular_expression: bool,
        /// Optional glob for workspace-relative paths or file names.
        file_filter: Option<String>,
    },
    /// Request cancellation of a running search.
    CancelWorkspaceSearch {
        /// Search job identity.
        job_id: u64,
    },
    /// Drain available incremental results from a search job.
    PollWorkspaceSearch {
        /// Search job identity.
        job_id: u64,
    },
    /// Open a workspace-relative UTF-8 text file as a document.
    OpenDocument {
        /// Workspace-relative file path.
        path: String,
    },
    /// Read a document snapshot including its contents and revision.
    ReadDocument {
        /// Identifier returned by document open or create.
        document_id: u64,
    },
    /// Apply an atomic set of edits against the stated revision.
    ApplyDocumentEdits {
        /// Document identifier.
        document_id: u64,
        /// Revision the edits were calculated against.
        expected_revision: u64,
        /// Non-overlapping edits expressed in UTF-16 positions.
        edits: Vec<TextEdit>,
    },
    /// Save a dirty document, rejecting an external file conflict.
    SaveDocument {
        /// Document identifier.
        document_id: u64,
        /// Revision the save was requested for.
        expected_revision: u64,
    },
    /// Read bounded current disk contents after a save conflict was detected.
    ReadDocumentConflict {
        /// Document identifier.
        document_id: u64,
    },
    /// Resolve a conflict returned by a prior save attempt.
    ResolveDocumentConflict {
        /// Document identifier.
        document_id: u64,
        /// Revision whose content is being resolved.
        expected_revision: u64,
        /// Explicit reload or overwrite decision.
        decision: ConflictDecision,
    },
    /// Close a document, requiring explicit discard when it has unsaved changes.
    CloseDocument {
        /// Document identifier.
        document_id: u64,
        /// Whether unsaved changes may be discarded.
        discard_unsaved_changes: bool,
    },
    /// Register a language-neutral logical project.
    RegisterProject {
        /// User-facing project label.
        display_name: String,
        /// Workspace-relative directory.
        root: String,
        /// Provider identity.
        provider_id: String,
        /// Optional workspace-relative configuration file.
        configuration_path: Option<String>,
    },
    /// List projects in the current workspace.
    ListProjects,
    /// Explicitly discover projects using a bounded marker-file provider.
    DiscoverProjects {
        /// Registered provider identifier.
        provider_id: String,
        /// Filename recognized as a project marker; the file is never read or executed.
        marker_filename: String,
        /// Maximum directories to inspect.
        max_directories: usize,
        /// Maximum candidates to return.
        max_projects: usize,
        /// Maximum directory nesting, with workspace root at depth zero.
        max_depth: usize,
    },
    /// Retrieve one project.
    ProjectInfo {
        /// Manager-local project identifier.
        project_id: u64,
    },
    /// Unregister one project.
    UnregisterProject {
        /// Manager-local project identifier.
        project_id: u64,
    },
    /// Explicitly discover Cargo projects without running Cargo or a project command.
    DiscoverCargoProjects {
        /// Maximum directories to inspect.
        max_directories: usize,
        /// Maximum Cargo roots to return.
        max_projects: usize,
        /// Maximum directory nesting, with workspace root at depth zero.
        max_depth: usize,
    },
    /// Retrieve manifest-derived Cargo information for one project.
    CargoProjectInfo {
        /// Manager-local project identifier.
        project_id: u64,
    },
    /// Register a direct-process task definition. Registration does not authorize execution.
    RegisterTask {
        /// Stable key within the project scope.
        key: String,
        /// User-facing label.
        display_name: String,
        /// One of build, check, test, clean, run, or custom.
        category: String,
        /// Executable path or name.
        executable: String,
        /// Direct process arguments; never shell parsed.
        arguments: Vec<String>,
        /// Workspace-relative working directory; empty means workspace root.
        working_directory: String,
        /// Process environment overrides, omitted from responses.
        environment: std::collections::BTreeMap<String, String>,
        /// Optional timeout.
        timeout_ms: Option<u64>,
        /// Optional associated project identifier.
        project_id: Option<u64>,
    },
    /// List task definitions without exposing environment values.
    ListTasks,
    /// Start a task with the host's configured authorization policy.
    StartTask {
        /// Task definition identifier.
        task_id: u64,
    },
    /// Request host-controlled execution of one task definition.
    RequestTaskExecution {
        /// Task definition identifier.
        task_id: u64,
    },
    /// List unexpired task execution requests awaiting host approval.
    ListPendingApprovals,
    /// Approve and consume one exact pending execution request.
    ApproveTaskExecution {
        /// One-time approval request identifier.
        request_id: u64,
    },
    /// Deny and consume one pending execution request.
    DenyTaskExecution {
        /// One-time approval request identifier.
        request_id: u64,
    },
    /// Query lifecycle state.
    TaskStatus {
        /// Task run identifier.
        run_id: u64,
    },
    /// Query retained output and lifecycle state.
    TaskOutput {
        /// Task run identifier.
        run_id: u64,
    },
    /// Cancel a running task.
    CancelTask {
        /// Task run identifier.
        run_id: u64,
    },
    /// Remove a completed run and its retained output.
    CleanupTask {
        /// Task run identifier.
        run_id: u64,
    },
    /// Request an interactive shell session; the host always requires explicit approval.
    RequestTerminalStart,
    /// List current PTY-backed terminal sessions.
    ListTerminalSessions,
    /// Approve one exact pending interactive shell request.
    ApproveTerminalStart {
        /// One-time approval request identity.
        request_id: u64,
    },
    /// Deny one pending interactive shell request.
    DenyTerminalStart {
        /// One-time approval request identity.
        request_id: u64,
    },
    /// Send one base64-encoded binary input chunk to a running session.
    TerminalWrite {
        /// Terminal session identity.
        session_id: u64,
        /// Base64-encoded raw terminal input bytes.
        input_base64: String,
    },
    /// Resize a running session's PTY.
    TerminalResize {
        /// Terminal session identity.
        session_id: u64,
        /// New row count.
        rows: u16,
        /// New column count.
        columns: u16,
    },
    /// Close a terminal session, optionally confirming active process termination.
    CloseTerminal {
        /// Terminal session identity.
        session_id: u64,
        /// Whether the user confirmed terminating a live process.
        terminate_active: bool,
    },
    /// Register a validated, language-neutral DAP adapter and launch configuration.
    /// Registration does not authorize or start the adapter.
    RegisterDebugConfiguration {
        /// Direct local stdio adapter definition.
        adapter: koren_debug::DebugAdapterDefinition,
        /// Launch or attach configuration bound to the adapter.
        configuration: koren_debug::DebugConfiguration,
    },
    /// List debugger profiles without exposing environment values.
    ListDebugConfigurations,
    /// Create an exact, expiring authorization request for a debug profile.
    RequestDebugStart {
        /// Registered profile identifier.
        configuration_id: String,
    },
    /// Approve one exact adapter and target launch/attach request.
    ApproveDebugStart {
        /// One-time approval identifier.
        approval_id: u64,
    },
    /// Deny one pending debug start request.
    DenyDebugStart {
        /// One-time approval identifier.
        approval_id: u64,
    },
    /// List current debug sessions.
    ListDebugSessions,
    /// Request one execution control operation.
    DebugAction {
        /// Session identity.
        session_id: u64,
        /// `continue`, `pause`, `step_over`, `step_into`, or `step_out`.
        action: String,
        /// Thread identity for thread-scoped actions.
        thread_id: Option<i64>,
    },
    /// Request bounded threads, stack, scope, or variable data.
    DebugInspect {
        /// Session identity.
        session_id: u64,
        /// `threads`, `stack_trace`, `scopes`, or `variables`.
        operation: String,
        /// Thread identity for threads or stack traces.
        thread_id: Option<i64>,
        /// Frame identity for scope lookup.
        frame_id: Option<i64>,
        /// Stop-scoped DAP variable reference.
        reference: Option<koren_debug::VariableReference>,
        /// First requested frame or variable offset.
        start: usize,
        /// Bounded page size.
        count: usize,
    },
    /// Replace source breakpoints and synchronize active sessions.
    SetDebugBreakpoints {
        /// Absolute source path.
        source: String,
        /// Complete replacement set for the source.
        breakpoints: Vec<koren_debug::BreakpointSpec>,
    },
    /// Disconnect or terminate a debug session.
    StopDebugSession {
        /// Session identity.
        session_id: u64,
    },
}

/// Versioned request envelope for internal consumers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Protocol schema version.
    pub version: u16,
    /// Caller-assigned request identifier.
    pub request_id: u64,
    /// Typed operation.
    pub method: RequestMethod,
}

impl Request {
    /// Creates a request using the current experimental schema version.
    #[must_use]
    pub fn new(request_id: u64, method: RequestMethod) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id,
            method,
        }
    }
}

/// Structured protocol failure categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The peer uses an unsupported schema version.
    UnsupportedVersion,
    /// The requested operation or command is not available.
    NotFound,
    /// A request is malformed or violates a contract rule.
    InvalidRequest,
    /// The requested document save conflicts with external changes.
    Conflict,
    /// A path is invalid under workspace boundary rules.
    InvalidPath,
    /// A document or workspace is not open.
    NotOpen,
    /// An operation failed because of the local environment.
    Internal,
    /// The host authorization policy denied task execution.
    Unauthorized,
    /// The request cannot be completed while the target resource is busy.
    Busy,
    /// The requested operation is invalid for the resource's current state.
    InvalidState,
}

/// Structured error payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProtocolError {
    /// Machine-readable category.
    pub code: ErrorCode,
    /// Human-readable explanation.
    pub message: String,
}

/// Kind of workspace directory entry in a protocol response.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectoryEntryKind {
    /// Regular file.
    File,
    /// Directory.
    Directory,
    /// Symbolic link, not traversed by workspace operations.
    Symlink,
    /// Other filesystem object.
    Other,
}

/// Serializable metadata for a workspace directory entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DirectoryEntry {
    /// UTF-8 entry name.
    pub name: String,
    /// UTF-8 path relative to the workspace root.
    pub path: String,
    /// Filesystem entry kind.
    pub kind: DirectoryEntryKind,
    /// File size or filesystem-reported directory size.
    pub size_bytes: u64,
}

/// A local Git repository summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitRepositorySummary {
    /// Canonical worktree root.
    pub root: String,
    /// Current branch, or `None` when HEAD is detached/unborn.
    pub branch: Option<String>,
    /// Whether the repository has a commit at HEAD.
    pub has_head: bool,
}

/// One changed path returned by the local Git service.
// Git's index and worktree states overlap; independent flags preserve staged plus unstaged edits.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitChangedFile {
    /// Repository-relative path.
    pub path: String,
    /// Whether the index contains a staged change.
    pub staged: bool,
    /// Whether the working tree contains an unstaged change.
    pub unstaged: bool,
    /// Whether the path is untracked.
    pub untracked: bool,
    /// Whether the path is deleted.
    pub deleted: bool,
    /// Whether the path has an unresolved conflict.
    pub conflicted: bool,
}

/// Branch and path status for one repository.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitStatusSummary {
    /// Canonical worktree root.
    pub root: String,
    /// Current branch name when attached.
    pub branch: Option<String>,
    /// Whether HEAD has a commit.
    pub has_head: bool,
    /// Changed files.
    pub files: Vec<GitChangedFile>,
}

/// One workspace text-search match.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SearchMatch {
    /// Workspace-relative source path.
    pub path: String,
    /// One-based line number.
    pub line: usize,
    /// Zero-based UTF-16 column.
    pub column_utf16: usize,
    /// Match width in UTF-16 code units.
    pub match_length_utf16: usize,
    /// Bounded source line preview.
    pub preview: String,
}

/// Terminal statistics for one workspace search.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SearchSummary {
    /// Files scanned.
    pub files_scanned: usize,
    /// Matches returned.
    pub matches_found: usize,
    /// Binary or invalid UTF-8 files skipped.
    pub skipped_binary_files: usize,
    /// Whether the result cap stopped traversal.
    pub truncated: bool,
    /// Whether the search was cancelled.
    pub cancelled: bool,
}

/// Coalesced progress from a running workspace search.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SearchProgress {
    /// Files scanned so far.
    pub files_scanned: usize,
    /// Matches produced so far.
    pub matches_found: usize,
    /// Most recently scanned path.
    pub current_path: Option<String>,
}

/// Typed successful response values.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ResponseValue {
    /// Ping response.
    Pong,
    /// Available command identifiers.
    Commands(Vec<String>),
    /// Text returned by a command.
    CommandOutput(String),
    /// Installed extension metadata and active command contributions.
    Extensions(ExtensionCatalog),
    /// A setting value was stored successfully.
    ExtensionSettingUpdated {
        /// Fully qualified setting identifier.
        setting_id: String,
    },
    /// Current inspection profile and rule configuration.
    InspectionConfiguration(InspectionConfigurationSummary),
    /// Imported historical findings from a user-selected SARIF report.
    ImportedSarif {
        /// Import report identity.
        report_id: String,
        /// Imported findings, clearly marked with imported origin.
        findings: Vec<InspectionFinding>,
        /// Adapter warnings for unsupported fields or actions.
        warnings: Vec<String>,
    },
    /// Exported SARIF document and adapter warnings.
    SarifReport {
        /// Serialized SARIF 2.1.0 document.
        document: String,
        /// Adapter warnings for unsupported fields or actions.
        warnings: Vec<String>,
    },
    /// A project inspection job was accepted.
    ProjectInspectionStarted {
        /// Job identity.
        job_id: u64,
    },
    /// Progress summary for a project inspection job.
    ProjectInspectionStatus(ProjectInspectionSummary),
    /// One page of project inspection findings.
    ProjectInspectionFindings(ProjectInspectionFindingPage),
    /// A cancellation request was recorded.
    ProjectInspectionCancelled {
        /// Job identity.
        job_id: u64,
    },
    /// Extension lifecycle transition result.
    ExtensionChanged(ExtensionSummary),
    /// Installed extension language-server configurations.
    LanguageServers(Vec<LanguageServerSummary>),
    /// Host approval is required before the external executable can start.
    LanguageServerApprovalRequired(LanguageServerApprovalSummary),
    /// Language server started after approval.
    LanguageServerStarted {
        /// Manager-local server identity.
        server_id: u64,
        /// Declaring extension.
        extension_id: String,
        /// Configuration identifier.
        configuration_id: String,
    },
    /// Language-server approval was denied.
    LanguageServerDenied {
        /// Consumed approval identity.
        approval_id: u64,
    },
    /// Language server stopped.
    LanguageServerStopped {
        /// Stopped server identity.
        server_id: u64,
    },
    /// Versioned hover content or an empty result.
    LanguageHover {
        /// Document revision used for the request.
        document_revision: u64,
        /// Plain text or markdown content.
        contents: Option<String>,
        /// Optional hover range.
        range: Option<TextRange>,
    },
    /// Versioned completion items.
    LanguageCompletions {
        /// Document revision used for the request.
        document_revision: u64,
        /// Plain-text completion values.
        items: Vec<LanguageCompletionItem>,
    },
    /// Versioned definition navigation results.
    LanguageDefinitions {
        /// Document revision used for the request.
        document_revision: u64,
        /// Definition locations.
        locations: Vec<LanguageLocation>,
    },
    /// Versioned symbol reference locations.
    LanguageReferences {
        /// Document revision used for the request.
        document_revision: u64,
        /// Reference locations.
        locations: Vec<LanguageLocation>,
    },
    /// Versioned formatting proposal; the host must preview before applying.
    LanguageFormatting {
        /// Document revision used for the request.
        document_revision: u64,
        /// Proposed text edits.
        edits: Vec<TextEdit>,
    },
    /// Versioned code actions, including unsupported proposals with an explanation.
    LanguageCodeActions {
        /// Document revision used for the request.
        document_revision: u64,
        /// Available actions.
        actions: Vec<LanguageCodeAction>,
    },
    /// Versioned, single-document rename proposal; preview before applying.
    LanguageRename {
        /// Document revision used for the request.
        document_revision: u64,
        /// Proposed text edits for the active document.
        edits: Vec<TextEdit>,
    },
    /// Inspection results tied to the analyzed document revision.
    InspectionFindings {
        /// Document revision used for analysis.
        document_revision: u64,
        /// Provider-attributed findings.
        findings: Vec<InspectionFinding>,
    },
    /// Root path and top-level entry count.
    Workspace {
        /// Canonical workspace root path.
        root: String,
        /// Number of top-level entries.
        entries: usize,
    },
    /// Successfully opened workspace.
    WorkspaceOpened {
        /// Canonical workspace root.
        root: String,
    },
    /// Successfully closed workspace.
    WorkspaceClosed {
        /// Canonical root that was closed.
        root: String,
    },
    /// Entries from one workspace directory.
    DirectoryEntries(Vec<DirectoryEntry>),
    /// Metadata for one filesystem entry.
    WorkspaceEntry(DirectoryEntry),
    /// Result of a workspace filesystem mutation.
    WorkspaceEntryChanged {
        /// Operation: created, renamed, or deleted.
        operation: String,
        /// Resulting or removed relative path.
        path: String,
        /// Previous relative path for a rename.
        previous_path: Option<String>,
    },
    /// Discovered repositories under the active workspace.
    GitRepositories(Vec<GitRepositorySummary>),
    /// Current branch and path-specific status for one repository.
    GitStatus(GitStatusSummary),
    /// Bounded textual diff for one repository-relative path.
    GitDiff {
        /// Repository-relative file path.
        path: String,
        /// Whether this compares the index to HEAD.
        staged: bool,
        /// Text diff content.
        text: String,
    },
    /// A local Git mutation completed.
    GitOperation {
        /// Completed operation.
        operation: String,
        /// New commit object ID when a commit was created.
        commit: Option<String>,
    },
    /// A bounded search was started.
    SearchStarted {
        /// Search job identity.
        job_id: u64,
    },
    /// Incremental search results and terminal status.
    SearchBatch {
        /// Search job identity.
        job_id: u64,
        /// Results drained in this batch.
        matches: Vec<SearchMatch>,
        /// Latest progress snapshot, if available.
        progress: Option<SearchProgress>,
        /// Whether a terminal event was received.
        complete: bool,
        /// Terminal summary when complete.
        summary: Option<SearchSummary>,
        /// Terminal error, if traversal failed.
        error: Option<String>,
    },
    /// A search cancellation request was recorded.
    SearchCancelled {
        /// Search job identity.
        job_id: u64,
    },
    /// Metadata for a newly opened or already open document.
    DocumentOpened {
        /// Manager-local document identifier.
        document_id: u64,
        /// Workspace-relative path, absent for an untitled document.
        path: Option<String>,
        /// Current document revision.
        revision: u64,
        /// Whether contents differ from the last saved state.
        dirty: bool,
    },
    /// Contents and metadata for a document read request.
    DocumentContents {
        /// Manager-local document identifier.
        document_id: u64,
        /// UTF-8 document contents.
        contents: String,
        /// Current document revision.
        revision: u64,
        /// Whether contents differ from the last saved state.
        dirty: bool,
    },
    /// Result of applying one or more edits.
    DocumentChanged {
        /// Manager-local document identifier.
        document_id: u64,
        /// New document revision.
        revision: u64,
        /// Whether the document is dirty.
        dirty: bool,
    },
    /// Result of saving or resolving a document conflict.
    DocumentSaved {
        /// Manager-local document identifier.
        document_id: u64,
        /// Saved or reloaded revision.
        revision: u64,
        /// Whether the document remains dirty.
        dirty: bool,
    },
    /// External contents for a pending conflict; absent means the file was deleted.
    DocumentConflictContents {
        /// Document identifier.
        document_id: u64,
        /// Workspace-relative backing path.
        path: Option<String>,
        /// External UTF-8 file contents, or `None` if deleted.
        contents: Option<String>,
    },
    /// Result of closing a document.
    DocumentClosed {
        /// Manager-local document identifier.
        document_id: u64,
    },
    /// One or more projects.
    Projects(Vec<ProjectSummary>),
    /// Project registration or lookup result.
    Project(ProjectSummary),
    /// Project removed.
    ProjectClosed {
        /// Removed project identifier.
        project_id: u64,
    },
    /// Cargo projects discovered within explicit traversal bounds.
    CargoProjects(Vec<CargoProjectSummary>),
    /// Typed Cargo information for one logical project.
    CargoProject(CargoProjectSummary),
    /// Task definition registered.
    TaskRegistered {
        /// Registered task identifier.
        task_id: u64,
    },
    /// Task definitions without environment values.
    Tasks(Vec<TaskSummary>),
    /// Task process started.
    TaskStarted {
        /// Started run identifier.
        run_id: u64,
    },
    /// Execution request is awaiting one-time approval.
    ApprovalRequired(ExecutionApprovalSummary),
    /// Pending requests in manager-local identifier order.
    PendingApprovals(Vec<ExecutionApprovalSummary>),
    /// Approved request and started task run.
    TaskApproved {
        /// Consumed request identifier.
        request_id: u64,
        /// New task run identifier.
        run_id: u64,
    },
    /// Denied request was consumed without starting a process.
    TaskDenied {
        /// Consumed request identifier.
        request_id: u64,
    },
    /// Task state and bounded output snapshot.
    TaskSnapshot {
        /// Run identifier.
        run_id: u64,
        /// Definition identifier.
        task_id: u64,
        /// Typed task process lifecycle state.
        state: TaskLifecycle,
        /// Process exit code, if the process was spawned and provided one.
        exit_code: Option<i32>,
        /// Spawn or process failure information.
        error: Option<String>,
        /// Retained standard output.
        stdout: String,
        /// Retained standard error.
        stderr: String,
        /// Whether output exceeded the retention limit.
        output_truncated: bool,
    },
    /// Task cancellation requested.
    TaskCancelled {
        /// Run identifier.
        run_id: u64,
    },
    /// Completed task output cleaned up.
    TaskCleaned {
        /// Run identifier.
        run_id: u64,
    },
    /// Interactive shell request awaiting explicit approval.
    TerminalApprovalRequired(TerminalApprovalSummary),
    /// Current terminal sessions.
    TerminalSessions(Vec<TerminalSessionSummary>),
    /// Approved shell request and started PTY session.
    TerminalApproved {
        /// Consumed approval request identity.
        request_id: u64,
        /// Newly running terminal session.
        session: TerminalSessionSummary,
    },
    /// Denied shell request was consumed without spawning a process.
    TerminalDenied {
        /// Consumed approval request identity.
        request_id: u64,
    },
    /// Terminal input, resize, or close operation completed.
    TerminalAction {
        /// Terminal session identity.
        session_id: u64,
    },
    /// A registered debug profile, with environment values omitted.
    DebugConfigurations(Vec<DebugConfigurationSummary>),
    /// Debug start approval is required before any adapter process starts.
    DebugApprovalRequired(DebugApprovalSummary),
    /// The exact approved profile started a new debug session.
    DebugSessionStarted(koren_debug::DebugSessionInfo),
    /// Current debug session snapshots.
    DebugSessions(Vec<koren_debug::DebugSessionInfo>),
    /// A bounded DAP request was queued on its session actor.
    DebugRequestAccepted {
        /// Session identity.
        session_id: u64,
        /// Request identity for asynchronous response correlation.
        request_id: u64,
    },
    /// The complete source breakpoint replacement set.
    DebugBreakpoints(Vec<koren_debug::SourceBreakpoint>),
    /// A pending debug start was denied without launching an adapter.
    DebugDenied {
        /// Consumed approval identifier.
        approval_id: u64,
    },
}

/// Public debug profile summary without secret environment values.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DebugConfigurationSummary {
    /// Configuration identity.
    pub id: String,
    /// User-visible configuration name.
    pub name: String,
    /// Adapter identity.
    pub adapter_id: String,
    /// Adapter display name.
    pub adapter_name: String,
    /// Launch or attach request type.
    pub request: koren_debug::DebugRequestType,
    /// Program path or executable metadata.
    pub program: Option<String>,
    /// Attach process identifier, if applicable.
    pub process_id: Option<u32>,
}

/// Exact launch details presented before starting an adapter and its target.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DebugApprovalSummary {
    /// One-time approval identity.
    pub approval_id: u64,
    /// Adapter display name.
    pub adapter_name: String,
    /// Adapter executable and direct argv.
    pub adapter_command: String,
    /// Adapter arguments.
    pub adapter_arguments: Vec<String>,
    /// Adapter working directory.
    pub adapter_working_directory: String,
    /// Adapter environment variable names.
    pub adapter_environment_keys: Vec<String>,
    /// Launch/attach target request.
    pub request: koren_debug::DebugRequestType,
    /// Target program path.
    pub program: Option<String>,
    /// Target process identifier.
    pub process_id: Option<u32>,
    /// Target arguments.
    pub arguments: Vec<String>,
    /// Target working directory.
    pub working_directory: String,
    /// Target environment variable names.
    pub environment_keys: Vec<String>,
    /// Expiration time remaining in milliseconds.
    pub expires_in_ms: u64,
}

/// Exact interactive shell details shown to the user before approval.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TerminalApprovalSummary {
    /// One-time approval request identifier.
    pub request_id: u64,
    /// Configured terminal profile identifier.
    pub profile_id: String,
    /// User-facing profile name.
    pub profile_name: String,
    /// Shell executable path.
    pub executable: String,
    /// Direct argv values.
    pub arguments: Vec<String>,
    /// Initial working directory.
    pub working_directory: String,
    /// Environment variable names; values are never exposed.
    pub environment_keys: Vec<String>,
    /// Detected-shell fallback executable, if any.
    pub fallback_executable: Option<String>,
    /// Requesting actor.
    pub actor: String,
    /// Associated workspace root, if one is open.
    pub workspace: Option<String>,
}

/// Current public state of a PTY-backed terminal session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TerminalSessionSummary {
    /// Unique terminal session identifier.
    pub session_id: u64,
    /// Configured profile identifier.
    pub profile_id: String,
    /// Working directory resolved before approval.
    pub working_directory: String,
    /// Session lifecycle state.
    pub state: TerminalLifecycle,
    /// Process exit status, if known.
    pub exit_code: Option<u32>,
    /// Whether the owned process group still has live processes.
    pub process_group_active: bool,
    /// Safe-to-display startup or PTY error.
    pub error: Option<String>,
    /// Current PTY rows.
    pub rows: u16,
    /// Current PTY columns.
    pub columns: u16,
}

/// PTY session lifecycle states exposed to desktop consumers.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalLifecycle {
    /// Reserved without allocating a PTY.
    Created,
    /// PTY and shell startup are in progress.
    Starting,
    /// Shell process is running.
    Running,
    /// Shell exited.
    Exited,
    /// PTY or shell startup failed.
    Failed,
    /// Host terminated the session.
    Terminated,
}

/// Extension metadata exposed to the desktop without implementation details.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtensionSummary {
    /// Reverse-DNS identifier.
    pub id: String,
    /// User-facing name.
    pub name: String,
    /// Publisher.
    pub publisher: String,
    /// Release version.
    pub version: String,
    /// Description.
    pub description: String,
    /// Whether explicitly enabled.
    pub enabled: bool,
    /// Lifecycle state.
    pub state: String,
    /// Last activation error.
    pub error: Option<String>,
}

/// Installed extensions and their safe command/menu contributions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtensionCatalog {
    /// Installed extension metadata.
    pub extensions: Vec<ExtensionSummary>,
    /// Active namespaced commands.
    pub commands: Vec<ExtensionCommandSummary>,
    /// Host built-in commands available to the command palette.
    #[serde(default)]
    pub built_in_commands: Vec<CommandCatalogSummary>,
    /// Active typed menu actions.
    pub menu_items: Vec<ExtensionMenuSummary>,
    /// Declarative language-server configurations.
    pub language_servers: Vec<LanguageServerSummary>,
    /// User-openable extension panels.
    #[serde(default)]
    pub tool_windows: Vec<ExtensionToolWindowSummary>,
    /// Namespaced extension setting definitions and effective values.
    #[serde(default)]
    pub settings: Vec<ExtensionSettingSummary>,
    /// Safe design-token themes.
    #[serde(default)]
    pub themes: Vec<ExtensionThemeSummary>,
    /// Non-destructive diagnostic enrichments.
    #[serde(default)]
    pub diagnostic_details: Vec<ExtensionDiagnosticDetailSummary>,
    /// Generic project wizard declarations for future host-authorized submissions.
    #[serde(default)]
    pub project_wizards: Vec<ExtensionProjectWizardSummary>,
}

/// Host-owned command palette metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandCatalogSummary {
    /// Stable host command identifier.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Command description.
    pub description: String,
    /// Optional keyboard shortcut.
    pub shortcut: Option<String>,
    /// Whether it is currently available.
    pub enabled: bool,
}

/// Primitive setting value sent through the typed protocol.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExtensionSettingValue {
    /// Boolean value.
    Boolean(bool),
    /// Integer value.
    Integer(i64),
    /// Plain-text value.
    Text(String),
}

/// Typed setting control schema.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExtensionSettingKind {
    /// Boolean toggle.
    Boolean {
        /// Default value.
        default: bool,
    },
    /// Bounded integer value.
    Integer {
        /// Default value.
        default: i64,
        /// Inclusive minimum.
        min: i64,
        /// Inclusive maximum.
        max: i64,
    },
    /// Bounded plain-text value.
    Text {
        /// Default value.
        default: String,
        /// Maximum byte length.
        max_length: usize,
    },
    /// One of a fixed option set.
    Choice {
        /// Default option.
        default: String,
        /// Allowed option values.
        options: Vec<String>,
    },
}

/// Safe contributed extension setting.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtensionSettingSummary {
    /// Fully qualified identifier.
    pub id: String,
    /// User-facing label.
    pub title: String,
    /// Optional help text.
    pub description: String,
    /// Accepted value schema.
    pub kind: ExtensionSettingKind,
    /// Current effective value.
    pub value: ExtensionSettingValue,
}

/// Declarative safe panel content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtensionUiDocument {
    /// Stable view identifier.
    pub id: String,
    /// Safe elements in display order.
    pub elements: Vec<ExtensionUiElement>,
}

/// Supported host-rendered UI primitives.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExtensionUiElement {
    /// Plain text.
    Text {
        /// Stable element identifier.
        id: String,
        /// Plain display text.
        text: String,
    },
    /// Section heading.
    Heading {
        /// Stable element identifier.
        id: String,
        /// Heading text.
        text: String,
    },
    /// Action button invoking an owned command.
    Button {
        /// Stable element identifier.
        id: String,
        /// Button label.
        label: String,
        /// Registered extension command.
        command_id: String,
    },
    /// List of plain text items.
    List {
        /// Stable element identifier.
        id: String,
        /// Display items.
        items: Vec<String>,
    },
    /// Two-column table.
    Table {
        /// Stable element identifier.
        id: String,
        /// Display rows.
        rows: Vec<[String; 2]>,
    },
    /// Bounded text field.
    Input {
        /// Stable component identifier.
        id: String,
        /// Field label.
        label: String,
        /// Current value.
        value: String,
        /// Whether the user must provide a value.
        required: bool,
        /// Maximum byte length.
        max_length: usize,
    },
    /// Fixed-option choice field.
    Select {
        /// Stable component identifier.
        id: String,
        /// Field label.
        label: String,
        /// Allowed values.
        options: Vec<String>,
        /// Selected default.
        value: String,
    },
    /// Boolean field.
    Checkbox {
        /// Stable component identifier.
        id: String,
        /// Field label.
        label: String,
        /// Default state.
        checked: bool,
    },
    /// Semantic state indicator.
    Status {
        /// Stable element identifier.
        id: String,
        /// Status label.
        label: String,
        /// Semantic tone name.
        tone: String,
    },
}

/// User-openable extension panel catalog item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtensionToolWindowSummary {
    /// Fully qualified panel identifier.
    pub id: String,
    /// Panel title.
    pub title: String,
    /// Initial declarative panel content.
    pub initial_view: ExtensionUiDocument,
}

/// Generic declarative project-creation form.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtensionProjectWizardSummary {
    /// Fully qualified wizard identifier.
    pub id: String,
    /// User-facing title.
    pub title: String,
    /// Safe declarative field model.
    pub form: ExtensionUiDocument,
}

/// Safe theme description.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtensionThemeSummary {
    /// Fully qualified theme identifier.
    pub id: String,
    /// Theme title.
    pub title: String,
    /// Base palette name.
    pub base: String,
    /// Approved design-token overrides.
    pub tokens: std::collections::BTreeMap<String, String>,
}

/// Non-destructive diagnostic presentation enrichment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtensionDiagnosticDetailSummary {
    /// Fully qualified detail identifier.
    pub id: String,
    /// Optional diagnostic source selector.
    pub source: Option<String>,
    /// Optional diagnostic code selector.
    pub code: Option<String>,
    /// Detail heading.
    pub title: String,
    /// Additional explanation.
    pub explanation: String,
    /// Optional extension command.
    pub suggested_command: Option<String>,
    /// Related context strings.
    pub related: Vec<String>,
}

/// A validated extension command contribution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtensionCommandSummary {
    /// Namespaced command identifier.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Short description.
    pub description: String,
    /// Optional keyboard shortcut.
    pub shortcut: Option<String>,
    /// Whether the host-evaluated availability condition currently allows invocation.
    pub enabled: bool,
}

/// Validated extension menu action exposed through the desktop bridge.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtensionMenuSummary {
    /// Namespaced command identifier.
    pub command_id: String,
    /// Menu label.
    pub label: String,
    /// Placement identifier.
    pub location: String,
    /// Current host-evaluated command availability.
    pub enabled: bool,
}

/// Declared external language-server command for a workspace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LanguageServerSummary {
    /// Declaring extension identifier.
    pub extension_id: String,
    /// Manifest-local configuration identifier.
    pub configuration_id: String,
    /// Direct executable.
    pub command: String,
    /// Direct argument vector.
    pub arguments: Vec<String>,
    /// Language identifiers served.
    pub languages: Vec<String>,
    /// File suffixes served.
    pub file_extensions: Vec<String>,
    /// Environment override names; values are never exposed.
    pub environment_keys: Vec<String>,
}

/// Exact launch parameters requiring one-time user approval.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LanguageServerApprovalSummary {
    /// One-time request identifier.
    pub approval_id: u64,
    /// Declaring extension identifier.
    pub extension_id: String,
    /// Configuration identifier.
    pub configuration_id: String,
    /// Executable to be started.
    pub command: String,
    /// Exact argument vector.
    pub arguments: Vec<String>,
    /// Working directory of the server.
    pub working_directory: String,
    /// Environment override names; values are hidden.
    pub environment_keys: Vec<String>,
}

/// Plain-text completion information returned through protocol v2.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LanguageCompletionItem {
    /// Completion label.
    pub label: String,
    /// Optional display detail.
    pub detail: Option<String>,
    /// Optional insertion text.
    pub insert_text: Option<String>,
    /// Optional documentation.
    pub documentation: Option<String>,
}

/// A definition target and UTF-16 range.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LanguageLocation {
    /// File URI.
    pub uri: String,
    /// Target range.
    pub range: TextRange,
}

/// A language-server code action safe to present to the user.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LanguageCodeAction {
    /// Request-scoped identity.
    pub id: String,
    /// User-facing title.
    pub title: String,
    /// Provider identity.
    pub provider_id: String,
    /// Optional LSP action kind.
    pub kind: Option<String>,
    /// Related diagnostic messages.
    pub related_diagnostics: Vec<String>,
    /// Text edits for the active document when the proposal is single-file and supported.
    pub edits: Option<Vec<TextEdit>>,
    /// Explanation when the proposal cannot be applied by this host.
    pub unsupported_reason: Option<String>,
    /// Provider supplied disabled reason.
    pub disabled_reason: Option<String>,
}

/// A provider-attributed inspection result returned to a host UI.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InspectionFinding {
    /// Stable finding instance identity.
    pub id: String,
    /// Stable provider identity.
    pub provider_id: String,
    /// Stable rule identity.
    pub rule_id: String,
    /// Workspace-relative source path.
    pub path: String,
    /// Document revision used by analysis.
    pub document_revision: Option<u64>,
    /// UTF-16 source range.
    pub range: TextRange,
    /// Severity name: error, warning, information, or hint.
    pub severity: String,
    /// User-facing diagnostic message.
    pub message: String,
    /// Optional contextual explanation.
    pub explanation: Option<String>,
    /// Optional grouping category.
    pub category: Option<String>,
    /// Related source locations.
    pub related: Vec<InspectionRelatedLocation>,
    /// Proposed user-confirmed text actions.
    pub actions: Vec<InspectionCodeAction>,
    /// Distinguishes imported/project/document results.
    pub origin: String,
    /// Imported report or project snapshot identifier, when available.
    pub source_id: Option<String>,
    /// Source revision attached to a historical imported result, when known.
    pub source_revision: Option<String>,
}

/// Inspection profile settings exposed to a desktop host.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InspectionConfigurationSummary {
    /// Available profile names.
    pub profiles: Vec<String>,
    /// Selected profile for the current workspace.
    pub selected_profile: String,
    /// Whether the active workspace selects its own profile.
    pub workspace_specific: bool,
    /// Registered rules and effective values.
    pub rules: Vec<InspectionRuleSummary>,
}

/// Progress and lifecycle state for one explicit project snapshot job.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectInspectionSummary {
    /// Job identity.
    pub job_id: u64,
    /// Current state: queued, running, completed, cancelled, or failed.
    pub state: String,
    /// Documents completed by providers.
    pub completed_documents: usize,
    /// Documents captured in the snapshot.
    pub total_documents: usize,
    /// Most recently analyzed workspace-relative path.
    pub current_path: Option<String>,
    /// Findings retained from the completed job.
    pub result_count: usize,
    /// Provider or worker failure, if any.
    pub error: Option<String>,
}

/// Bounded page of a completed project analysis.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectInspectionFindingPage {
    /// Job identity.
    pub job_id: u64,
    /// Starting result offset.
    pub offset: usize,
    /// Next result offset, or none at the end.
    pub next_offset: Option<usize>,
    /// Current page of findings.
    pub findings: Vec<InspectionFinding>,
}

/// Registered inspection rule plus the currently effective settings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InspectionRuleSummary {
    /// Stable namespaced rule ID.
    pub id: String,
    /// Owning provider ID.
    pub provider_id: String,
    /// User-facing title.
    pub title: String,
    /// Rule description.
    pub description: String,
    /// Grouping category.
    pub category: String,
    /// Supported language IDs.
    pub languages: Vec<String>,
    /// Provider default severity.
    pub default_severity: String,
    /// Effective enablement.
    pub enabled: bool,
    /// Effective severity.
    pub severity: String,
    /// Whether the current workspace overrides the profile setting.
    pub workspace_override: bool,
}

/// Related source location for an inspection finding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InspectionRelatedLocation {
    /// Workspace-relative source path.
    pub path: String,
    /// UTF-16 source range.
    pub range: TextRange,
    /// Related context message.
    pub message: String,
}

/// Supported text-only inspection action.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InspectionCodeAction {
    /// Stable request-scoped identity.
    pub id: String,
    /// User-facing action title.
    pub title: String,
    /// Provider identity.
    pub provider_id: String,
    /// Action category.
    pub kind: String,
    /// Diagnostic identifiers this action addresses.
    pub diagnostic_ids: Vec<String>,
    /// Proposed edits; the host must preview and confirm before application.
    pub edits: Vec<InspectionProposedTextEdit>,
    /// Reason the host currently cannot apply this action.
    pub disabled_reason: Option<String>,
}

/// Revision-bound proposed text replacement.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InspectionProposedTextEdit {
    /// Workspace-relative source path.
    pub path: String,
    /// Document revision used to calculate this edit.
    pub expected_revision: Option<u64>,
    /// UTF-16 replacement range.
    pub range: TextRange,
    /// Replacement contents.
    pub new_text: String,
}

/// Serializable project summary with manager-local identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectSummary {
    /// Project identifier.
    pub project_id: u64,
    /// Display name.
    pub display_name: String,
    /// Workspace-relative project root.
    pub root: String,
    /// Provider identifier.
    pub provider_id: String,
    /// Optional workspace-relative configuration file.
    pub configuration_path: Option<String>,
    /// Lifecycle of the registered project.
    pub state: ProjectLifecycle,
}

/// Project lifecycle exposed by the protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectLifecycle {
    /// Registered and usable.
    Registered,
    /// Closing.
    Closing,
}

/// Task lifecycle exposed by the protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskLifecycle {
    /// Reserved but not started.
    Pending,
    /// Process is active.
    Running,
    /// Process exited successfully.
    Succeeded,
    /// Process exited unsuccessfully.
    Failed,
    /// Explicit cancellation.
    Cancelled,
    /// Task timeout elapsed.
    TimedOut,
    /// Operating-system process creation failed.
    SpawnFailed,
}

/// Serializable task summary. Environment overrides are intentionally omitted.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskSummary {
    /// Task identifier.
    pub task_id: u64,
    /// Stable definition key in its project scope.
    pub key: String,
    /// Display name.
    pub display_name: String,
    /// Category label.
    pub category: String,
    /// Executable name/path.
    pub executable: String,
    /// Argument vector.
    pub arguments: Vec<String>,
    /// Working directory.
    pub working_directory: String,
    /// Optional timeout in milliseconds.
    pub timeout_ms: Option<u64>,
    /// Associated project identifier.
    pub project_id: Option<u64>,
}

/// Typed Cargo target information returned by the headless session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CargoTargetSummary {
    /// Target name.
    pub name: String,
    /// Normalized target kind.
    pub kind: String,
    /// Source path when known.
    pub source_path: Option<String>,
}

/// Package information from a local Cargo manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CargoPackageSummary {
    /// Package name.
    pub name: String,
    /// Package version, if declared directly in this manifest.
    pub version: Option<String>,
    /// Workspace-relative manifest path.
    pub manifest_path: String,
    /// Declared or conventionally detected targets.
    pub targets: Vec<CargoTargetSummary>,
}

/// Cargo workspace or package attached to a language-neutral Koren project.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CargoProjectSummary {
    /// General logical project fields.
    pub project: ProjectSummary,
    /// Whether the manifest declares a Cargo workspace.
    pub is_workspace: bool,
    /// Package represented by the root manifest, when present.
    pub package: Option<CargoPackageSummary>,
    /// Packages declared as workspace members.
    pub members: Vec<CargoPackageSummary>,
}

/// Safe presentation view of one pending execution approval.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecutionApprovalSummary {
    /// One-time approval request identifier.
    pub request_id: u64,
    /// Task definition identifier.
    pub task_id: u64,
    /// Task's stable key.
    pub task_key: String,
    /// Human-readable task name.
    pub display_name: String,
    /// Executable to be started.
    pub executable: String,
    /// Exact argv to be started.
    pub arguments: Vec<String>,
    /// Working directory.
    pub working_directory: String,
    /// Names of environment variables overridden; values are intentionally omitted.
    pub environment_keys: Vec<String>,
    /// Associated logical project, if any.
    pub project_id: Option<u64>,
    /// Workspace scope invalidating this request on close.
    pub workspace_root: String,
    /// Requesting actor known to this host.
    pub requesting_actor: String,
    /// Remaining request lifetime in milliseconds when serialized.
    pub expires_in_ms: u64,
}

/// Versioned response envelope for internal consumers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// Protocol schema version.
    pub version: u16,
    /// Identifier copied from the request.
    pub request_id: u64,
    /// Either a typed value or a structured error.
    pub result: Result<ResponseValue, ProtocolError>,
}

impl Response {
    /// Creates a response using the current experimental schema version.
    #[must_use]
    pub fn new(request_id: u64, result: Result<ResponseValue, ProtocolError>) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id,
            result,
        }
    }
}

/// Checks that an incoming request uses this build's schema version.
///
/// # Errors
///
/// Returns [`ProtocolError`] with [`ErrorCode::UnsupportedVersion`] if the
/// supplied version differs from [`PROTOCOL_VERSION`].
pub fn validate_version(version: u16) -> Result<(), ProtocolError> {
    if version == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(ProtocolError {
            code: ErrorCode::UnsupportedVersion,
            message: format!(
                "protocol version {version} is unsupported; expected {PROTOCOL_VERSION}"
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_and_has_stable_tagged_shape() {
        let request = Request::new(
            7,
            RequestMethod::ExecuteCommand {
                command_id: "core.ping".into(),
            },
        );
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            json,
            r#"{"version":2,"request_id":7,"method":{"method":"execute_command","params":{"command_id":"core.ping"}}}"#
        );
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), request);
    }

    #[test]
    fn response_serializes_structured_errors_and_values() {
        let error = ProtocolError {
            code: ErrorCode::NotFound,
            message: "unknown command".into(),
        };
        let response = Response::new(9, Err(error));
        let decoded: Response =
            serde_json::from_str(&serde_json::to_string(&response).unwrap()).unwrap();
        assert_eq!(decoded, response);
        let value = Response::new(10, Ok(ResponseValue::Pong));
        assert_eq!(
            serde_json::from_str::<Response>(&serde_json::to_string(&value).unwrap()).unwrap(),
            value
        );
    }

    #[test]
    fn protocol_version_is_checked_and_previous_version_rejected() {
        assert!(validate_version(PROTOCOL_VERSION).is_ok());
        let error = validate_version(1).unwrap_err();
        assert_eq!(error.code, ErrorCode::UnsupportedVersion);
    }

    #[test]
    fn additive_project_and_task_contracts_remain_version_two_and_typed() {
        let request = Request::new(
            11,
            RequestMethod::RegisterTask {
                key: "check".into(),
                display_name: "Check".into(),
                category: "check".into(),
                executable: "tool".into(),
                arguments: vec!["--safe".into()],
                working_directory: ".".into(),
                environment: std::collections::BTreeMap::new(),
                timeout_ms: None,
                project_id: None,
            },
        );
        let encoded = serde_json::to_string(&request).unwrap();
        assert!(encoded.contains("\"version\":2"));
        assert_eq!(serde_json::from_str::<Request>(&encoded).unwrap(), request);
        let response = Response::new(
            11,
            Ok(ResponseValue::TaskSnapshot {
                run_id: 3,
                task_id: 1,
                state: TaskLifecycle::Succeeded,
                exit_code: Some(0),
                error: None,
                stdout: "ok".into(),
                stderr: String::new(),
                output_truncated: false,
            }),
        );
        assert_eq!(
            serde_json::from_str::<Response>(&serde_json::to_string(&response).unwrap()).unwrap(),
            response
        );

        let approval_request = Request::new(12, RequestMethod::RequestTaskExecution { task_id: 8 });
        let encoded = serde_json::to_string(&approval_request).unwrap();
        assert!(encoded.contains("\"version\":2"));
        assert!(encoded.contains("request_task_execution"));
        assert_eq!(
            serde_json::from_str::<Request>(&encoded).unwrap(),
            approval_request
        );
        let conflict_preview =
            Request::new(13, RequestMethod::ReadDocumentConflict { document_id: 6 });
        let encoded = serde_json::to_string(&conflict_preview).unwrap();
        assert!(encoded.contains("read_document_conflict"));
        assert_eq!(
            serde_json::from_str::<Request>(&encoded).unwrap(),
            conflict_preview
        );
        let preview_response = Response::new(
            13,
            Ok(ResponseValue::DocumentConflictContents {
                document_id: 6,
                path: Some("src/main.rs".into()),
                contents: Some("fn main() {}\\n".into()),
            }),
        );
        assert_eq!(
            serde_json::from_str::<Response>(&serde_json::to_string(&preview_response).unwrap())
                .unwrap(),
            preview_response
        );
        let approval = Response::new(
            12,
            Ok(ResponseValue::ApprovalRequired(ExecutionApprovalSummary {
                request_id: 4,
                task_id: 8,
                task_key: "cargo.metadata".into(),
                display_name: "Cargo offline metadata".into(),
                executable: "cargo".into(),
                arguments: vec!["metadata".into(), "--offline".into()],
                working_directory: "/workspace".into(),
                environment_keys: vec!["TOKEN".into()],
                project_id: Some(3),
                workspace_root: "/workspace".into(),
                requesting_actor: "interactive_user".into(),
                expires_in_ms: 120_000,
            })),
        );
        let encoded = serde_json::to_string(&approval).unwrap();
        assert!(encoded.contains("approval_required"));
        assert!(!encoded.contains("secret-value"));
        assert_eq!(
            serde_json::from_str::<Response>(&encoded).unwrap(),
            approval
        );
    }

    #[test]
    fn terminal_contracts_round_trip_with_typed_lifecycle() {
        let request = Request::new(
            12,
            RequestMethod::TerminalWrite {
                session_id: 5,
                input_base64: "AP8=".into(),
            },
        );
        let encoded = serde_json::to_string(&request).unwrap();
        assert_eq!(serde_json::from_str::<Request>(&encoded).unwrap(), request);
        let response = Response::new(
            12,
            Ok(ResponseValue::TerminalSessions(vec![
                TerminalSessionSummary {
                    session_id: 5,
                    profile_id: "default".into(),
                    working_directory: "/tmp".into(),
                    state: TerminalLifecycle::Created,
                    exit_code: None,
                    process_group_active: false,
                    error: None,
                    rows: 24,
                    columns: 80,
                },
            ])),
        );
        let encoded = serde_json::to_string(&response).unwrap();
        assert!(encoded.contains("\"state\":\"created\""));
        assert_eq!(
            serde_json::from_str::<Response>(&encoded).unwrap(),
            response
        );
    }

    #[test]
    fn extension_and_language_service_contracts_are_additive_protocol_v2_variants() {
        let requests = [
            RequestMethod::ListExtensions,
            RequestMethod::SetExtensionEnabled {
                extension_id: "org.example.hello".into(),
                enabled: true,
            },
            RequestMethod::SetExtensionSetting {
                setting_id: "org.example.hello.show_path".into(),
                value: ExtensionSettingValue::Boolean(false),
            },
            RequestMethod::StartLanguageServer {
                extension_id: "org.example.lang".into(),
                configuration_id: "demo".into(),
            },
            RequestMethod::LanguageHover {
                document_id: 4,
                expected_revision: 9,
                position: TextPosition {
                    line: 1,
                    character: 2,
                },
            },
        ];
        for method in requests {
            let request = Request::new(42, method);
            let encoded = serde_json::to_string(&request).unwrap();
            assert!(encoded.contains("\"version\":2"));
            assert_eq!(serde_json::from_str::<Request>(&encoded).unwrap(), request);
        }
        let response = Response::new(
            42,
            Ok(ResponseValue::LanguageServerApprovalRequired(
                LanguageServerApprovalSummary {
                    approval_id: 2,
                    extension_id: "org.example.lang".into(),
                    configuration_id: "demo".into(),
                    command: "demo-server".into(),
                    arguments: vec!["--stdio".into()],
                    working_directory: "/workspace".into(),
                    environment_keys: vec!["API_TOKEN".into()],
                },
            )),
        );
        let encoded = serde_json::to_string(&response).unwrap();
        assert!(encoded.contains("language_server_approval_required"));
        assert!(encoded.contains("API_TOKEN"));
        assert_eq!(
            serde_json::from_str::<Response>(&encoded).unwrap(),
            response
        );
        let response = Response::new(
            43,
            Ok(ResponseValue::ExtensionSettingUpdated {
                setting_id: "org.example.hello.show_path".into(),
            }),
        );
        let encoded = serde_json::to_string(&response).unwrap();
        assert_eq!(
            serde_json::from_str::<Response>(&encoded).unwrap(),
            response
        );
    }

    #[test]
    fn inspection_profile_contract_round_trips() {
        let request = Request::new(
            22,
            RequestMethod::SetInspectionRule {
                rule_id: "org.example.todo".into(),
                enabled: false,
                severity: Some("error".into()),
                workspace_specific: true,
                reset: false,
            },
        );
        let encoded = serde_json::to_string(&request).unwrap();
        assert!(encoded.contains("set_inspection_rule"));
        assert_eq!(serde_json::from_str::<Request>(&encoded).unwrap(), request);
        let response = Response::new(
            22,
            Ok(ResponseValue::InspectionConfiguration(
                InspectionConfigurationSummary {
                    profiles: vec!["Default".into()],
                    selected_profile: "Default".into(),
                    workspace_specific: true,
                    rules: vec![InspectionRuleSummary {
                        id: "org.example.todo".into(),
                        provider_id: "org.example".into(),
                        title: "TODO".into(),
                        description: "Find TODO".into(),
                        category: "style".into(),
                        languages: vec!["plaintext".into()],
                        default_severity: "warning".into(),
                        enabled: false,
                        severity: "error".into(),
                        workspace_override: true,
                    }],
                },
            )),
        );
        assert_eq!(
            serde_json::from_str::<Response>(&serde_json::to_string(&response).unwrap()).unwrap(),
            response
        );
    }
}
