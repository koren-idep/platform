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
    /// Active typed menu actions.
    pub menu_items: Vec<ExtensionMenuSummary>,
    /// Declarative language-server configurations.
    pub language_servers: Vec<LanguageServerSummary>,
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
    fn extension_and_language_service_contracts_are_additive_protocol_v2_variants() {
        let requests = [
            RequestMethod::ListExtensions,
            RequestMethod::SetExtensionEnabled {
                extension_id: "org.example.hello".into(),
                enabled: true,
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
    }
}
