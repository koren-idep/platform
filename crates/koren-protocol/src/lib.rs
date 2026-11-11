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

/// Supported request operations in protocol version 2.
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
}
