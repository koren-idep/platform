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
    }
}
