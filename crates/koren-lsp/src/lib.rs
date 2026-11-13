//! Language-neutral LSP process, JSON-RPC, diagnostics, and document-sync primitives.
//!
//! This crate does not discover or launch servers automatically. A host supplies an
//! authorization decision before each new command/workspace configuration is started.
//! Launch permission is not process isolation; approved servers run with the host user's OS
//! privileges and should only be trusted when configured by the user.

mod actor;
pub use actor::{
    LspActorEvent, LspRequest, LspRequestTicket, LspResponse, LspServerActors, LspServerHandle,
};

use koren_documents::{DocumentId, DocumentSnapshot};
use koren_protocol::{TextEdit, TextPosition, TextRange};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt, fs,
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

/// Maximum LSP message body accepted from a language server.
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
/// Default request timeout used for initialization and language requests.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Maximum code actions accepted from one language-server response.
pub const MAX_CODE_ACTIONS: usize = 200;
/// Maximum files represented by one text-only workspace edit.
pub const MAX_WORKSPACE_EDIT_FILES: usize = 128;
/// Maximum text edits represented by one workspace edit.
pub const MAX_WORKSPACE_EDIT_ITEMS: usize = 2_000;
/// Maximum aggregate replacement text in one workspace edit.
pub const MAX_WORKSPACE_EDIT_TEXT_BYTES: usize = 2 * 1024 * 1024;
/// Maximum opaque data retained for one deferred code action.
pub const MAX_CODE_ACTION_DATA_BYTES: usize = 64 * 1024;

/// Exact direct-process configuration reviewed by the host before server startup.
#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub struct ServerConfig {
    /// Stable configuration identifier.
    pub id: String,
    /// Executable path or program name. Never shell parsed.
    pub command: String,
    /// Direct process argument vector.
    pub arguments: Vec<String>,
    /// Explicit working directory, normally the workspace root.
    pub workspace_root: PathBuf,
    /// Explicit environment overrides. Values are redacted from Debug output.
    pub environment: BTreeMap<String, String>,
}

impl fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerConfig")
            .field("id", &self.id)
            .field("command", &self.command)
            .field("arguments", &self.arguments)
            .field("workspace_root", &self.workspace_root)
            .field(
                "environment_keys",
                &self.environment.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl ServerConfig {
    /// Validates the launch configuration and canonicalizes its workspace root.
    ///
    /// # Errors
    /// Returns an error when the command or workspace root is invalid.
    pub fn validate(&mut self) -> Result<(), LspError> {
        if self.id.trim().is_empty() || self.command.trim().is_empty() {
            return Err(LspError::InvalidConfiguration(
                "server id and command must not be empty".into(),
            ));
        }
        self.workspace_root = fs::canonicalize(&self.workspace_root).map_err(|error| {
            LspError::InvalidConfiguration(format!("invalid workspace root: {error}"))
        })?;
        if !self.workspace_root.is_dir() {
            return Err(LspError::InvalidConfiguration(
                "language-server working directory must be a directory".into(),
            ));
        }
        Ok(())
    }
}

/// Host-controlled authorization for starting an external language server.
pub trait ServerAuthorizer {
    /// Returns true only after the host has explicitly approved this exact configuration.
    fn authorize(&mut self, configuration: &ServerConfig) -> bool;
}

/// Unique server identity within one [`LspServerManager`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ServerId(u64);

impl ServerId {
    /// Returns the numeric manager-local identity.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// LSP diagnostic severity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    /// Errors that prevent a correct result.
    Error,
    /// Problems that should be reviewed.
    Warning,
    /// Informational diagnostics.
    Information,
    /// Low-priority suggestions.
    Hint,
}

/// Related diagnostic location and explanation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelatedDiagnostic {
    /// File URI of the related location.
    pub uri: String,
    /// UTF-16 range of the related location.
    pub range: TextRange,
    /// Related message.
    pub message: String,
}

/// Language-neutral diagnostic value from an LSP server or another provider.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// File URI associated with the diagnostic.
    pub uri: String,
    /// Zero-based UTF-16 source range.
    pub range: TextRange,
    /// Severity, when supplied.
    pub severity: Option<DiagnosticSeverity>,
    /// Human-readable message.
    pub message: String,
    /// Diagnostic source label.
    pub source: Option<String>,
    /// Optional string or numeric diagnostic code.
    pub code: Option<String>,
    /// Related locations.
    pub related_information: Vec<RelatedDiagnostic>,
    /// Document revision observed when the diagnostic was published, if known.
    pub document_revision: Option<u64>,
    /// Language-server document version, if supplied.
    pub document_version: Option<i32>,
}

/// Diagnostics from one independent provider; separate providers never overwrite each other.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticSet {
    /// Provider identity, normally the extension or server id.
    pub provider_id: String,
    /// Server document URI for this publication, including empty clears.
    pub uri: String,
    /// Server document version for this publication, including empty clears.
    pub document_version: Option<i32>,
    /// Document manager id when available.
    pub document_id: Option<u64>,
    /// Diagnostics produced by this provider for this publication.
    pub diagnostics: Vec<Diagnostic>,
}

/// Position and version associated with a language-server result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VersionedResult<T> {
    /// File URI the result belongs to.
    pub uri: String,
    /// Language-server document version used for the request.
    pub document_version: i32,
    /// Typed result value.
    pub value: T,
}

impl<T> VersionedResult<T> {
    /// Whether this result still refers to the current document version.
    #[must_use]
    pub fn is_current(&self, current_version: i32) -> bool {
        self.document_version == current_version
    }
}

/// LSP location returned by definition requests.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Location {
    /// Target file URI.
    pub uri: String,
    /// Target UTF-16 range.
    pub range: TextRange,
}

/// One versioned text-document edit in an LSP workspace edit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VersionedDocumentEdits {
    /// Target file URI.
    pub uri: String,
    /// LSP document version, absent for edits from the unversioned `changes` form.
    pub version: Option<i32>,
    /// UTF-16 text replacements.
    pub edits: Vec<TextEdit>,
}

/// Supported text-only portion of an LSP workspace edit.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceEdit {
    /// Text edits grouped by source document.
    pub document_edits: Vec<VersionedDocumentEdits>,
    /// Unsupported resource operations, such as create, rename, or delete.
    pub unsupported_operations: Vec<String>,
}

/// Provider command returned by a code action. It is never executed automatically.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderCommand {
    /// User-facing command title.
    pub title: String,
    /// Provider command identity.
    pub command: String,
    /// Opaque provider arguments; hosts must require explicit authorization before dispatch.
    pub arguments: Vec<Value>,
}

/// Typed LSP code action or intention.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LspCodeAction {
    /// Request-scoped identity.
    pub id: String,
    /// User-facing action title.
    pub title: String,
    /// Provider identity.
    pub provider_id: String,
    /// Optional action kind, such as `quickfix`, `refactor`, or `source`.
    pub kind: Option<String>,
    /// Messages from related diagnostics returned by the provider.
    pub related_diagnostics: Vec<String>,
    /// Optional safe-to-preview text-only edit.
    pub edit: Option<WorkspaceEdit>,
    /// Optional provider command, never implicitly executed.
    pub command: Option<ProviderCommand>,
    /// Provider supplied disabled reason.
    pub disabled_reason: Option<String>,
    /// Opaque server data retained for advertised deferred resolution.
    pub data: Option<Value>,
}

/// Prepare-rename response from an LSP server.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RenamePreparation {
    /// Symbol range to replace.
    pub range: TextRange,
    /// Placeholder name suggested by the server.
    pub placeholder: Option<String>,
}

/// Formatting options sent to a language server.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FormattingOptions {
    /// Indentation width.
    pub tab_size: u32,
    /// Whether indentation uses spaces.
    pub insert_spaces: bool,
}

/// Concise, renderable hover result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HoverResult {
    /// Plain text extracted from markup content.
    pub contents: String,
    /// Hover range, if supplied by the server.
    pub range: Option<TextRange>,
}

/// A small typed subset of an LSP completion item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompletionItem {
    /// User-visible completion label.
    pub label: String,
    /// LSP completion kind, when supplied.
    pub kind: Option<u32>,
    /// Detail string, when supplied.
    pub detail: Option<String>,
    /// Replacement text, when supplied.
    pub insert_text: Option<String>,
    /// Plain-text documentation, when supplied.
    pub documentation: Option<String>,
}

/// Asynchronous LSP notification or server output event.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerEvent {
    /// LSP diagnostics were published.
    Diagnostics(DiagnosticSet),
    /// Non-fatal server stderr output.
    Stderr(String),
    /// Malformed stdout or reader failure.
    Error(String),
    /// Unrecognized but valid server notification.
    Notification {
        /// JSON-RPC notification method.
        method: String,
        /// Notification parameters.
        params: Value,
    },
}

/// Per-document LSP open/version map. Text is never retained here.
#[derive(Default, Debug)]
pub struct LspDocumentSync {
    documents: BTreeMap<DocumentId, SyncedDocument>,
    next_version: i32,
}

#[derive(Debug)]
struct SyncedDocument {
    uri: String,
    version: i32,
    revision: u64,
}

/// Outbound synchronization notification ready for a server transport.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncNotification {
    /// LSP notification method name.
    pub method: &'static str,
    /// Typed JSON parameters.
    pub params: Value,
}

impl LspDocumentSync {
    /// Creates a document/open notification from the current authoritative snapshot.
    ///
    /// # Errors
    /// Returns an error when the document is already open.
    pub fn open(
        &mut self,
        id: DocumentId,
        uri: impl Into<String>,
        language_id: &str,
        snapshot: &DocumentSnapshot,
    ) -> Result<SyncNotification, LspError> {
        if self.documents.contains_key(&id) {
            return Err(LspError::DocumentAlreadyOpen(id.0));
        }
        let uri = uri.into();
        let version = self
            .next_version
            .checked_add(1)
            .ok_or(LspError::VersionExhausted)?;
        self.next_version = version;
        self.documents.insert(
            id,
            SyncedDocument {
                uri: uri.clone(),
                version,
                revision: snapshot.info.revision,
            },
        );
        Ok(SyncNotification {
            method: "textDocument/didOpen",
            params: json!({
                "textDocument": { "uri": uri, "languageId": language_id, "version": version, "text": snapshot.text }
            }),
        })
    }

    /// Emits a full-text change for a strictly newer document revision.
    ///
    /// The caller supplies the current platform snapshot; the LSP service keeps only the
    /// document-to-version association, not an independent buffer.
    ///
    /// # Errors
    /// Returns an error when the document is closed, the revision is stale, or the LSP
    /// version counter is exhausted.
    pub fn change(
        &mut self,
        id: DocumentId,
        snapshot: &DocumentSnapshot,
    ) -> Result<SyncNotification, LspError> {
        let document = self
            .documents
            .get(&id)
            .ok_or(LspError::DocumentNotOpen(id.0))?;
        if snapshot.info.revision <= document.revision {
            return Err(LspError::StaleDocumentRevision {
                id: id.0,
                current: document.revision,
                received: snapshot.info.revision,
            });
        }
        let version = self
            .next_version
            .checked_add(1)
            .ok_or(LspError::VersionExhausted)?;
        self.next_version = version;
        let document = self
            .documents
            .get_mut(&id)
            .ok_or(LspError::DocumentNotOpen(id.0))?;
        document.version = version;
        document.revision = snapshot.info.revision;
        Ok(SyncNotification {
            method: "textDocument/didChange",
            params: json!({
                "textDocument": { "uri": document.uri, "version": document.version },
                "contentChanges": [{ "text": snapshot.text }]
            }),
        })
    }

    /// Emits a document/close notification and removes the association.
    ///
    /// # Errors
    /// Returns an error when the document is not open.
    pub fn close(&mut self, id: DocumentId) -> Result<SyncNotification, LspError> {
        let document = self
            .documents
            .remove(&id)
            .ok_or(LspError::DocumentNotOpen(id.0))?;
        Ok(SyncNotification {
            method: "textDocument/didClose",
            params: json!({
                "textDocument": { "uri": document.uri }
            }),
        })
    }

    /// Returns current LSP version and authoritative document revision.
    #[must_use]
    pub fn version(&self, id: DocumentId) -> Option<(i32, u64)> {
        self.documents
            .get(&id)
            .map(|document| (document.version, document.revision))
    }

    /// Returns whether a response version still matches the open document.
    #[must_use]
    pub fn accepts(&self, id: DocumentId, version: i32) -> bool {
        self.documents
            .get(&id)
            .is_some_and(|document| document.version == version)
    }

    /// Returns the URI associated with an open manager-local document.
    #[must_use]
    pub fn uri(&self, id: DocumentId) -> Option<&str> {
        self.documents
            .get(&id)
            .map(|document| document.uri.as_str())
    }

    /// Finds an open manager-local document by its server URI.
    #[must_use]
    pub fn document_id(&self, uri: &str) -> Option<DocumentId> {
        self.documents
            .iter()
            .find_map(|(id, document)| (document.uri == uri).then_some(*id))
    }

    /// Lists synchronized document identifiers for orderly server shutdown.
    #[must_use]
    pub fn document_ids(&self) -> Vec<DocumentId> {
        self.documents.keys().copied().collect()
    }
}

/// Manager owns authorized language-server processes for workspaces.
#[derive(Default)]
pub struct LspServerManager {
    next_id: u64,
    servers: BTreeMap<ServerId, LspServer>,
    configurations: BTreeMap<ServerId, ServerConfig>,
    approved: BTreeSet<ServerConfig>,
}

impl fmt::Debug for LspServerManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LspServerManager")
            .field("server_count", &self.servers.len())
            .field("approved_configuration_count", &self.approved.len())
            .finish_non_exhaustive()
    }
}

impl LspServerManager {
    /// Creates an empty process manager.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a directly executed server after explicit authorization and initialization.
    ///
    /// # Errors
    /// Returns an error when configuration validation, authorization, process startup, or
    /// LSP initialization fails.
    pub fn start_server(
        &mut self,
        config: ServerConfig,
        authorizer: &mut impl ServerAuthorizer,
    ) -> Result<ServerId, LspError> {
        self.start_server_with_timeout(config, authorizer, DEFAULT_REQUEST_TIMEOUT)
    }

    /// Starts a directly executed server with a host-selected initialization deadline.
    ///
    /// # Errors
    /// Returns an error when configuration validation, authorization, process startup, or
    /// LSP initialization fails before `initialization_timeout`.
    pub fn start_server_with_timeout(
        &mut self,
        mut config: ServerConfig,
        authorizer: &mut impl ServerAuthorizer,
        initialization_timeout: Duration,
    ) -> Result<ServerId, LspError> {
        config.validate()?;
        if let Some((id, _)) = self
            .configurations
            .iter()
            .find(|(_, running)| **running == config)
        {
            return Ok(*id);
        }
        if !self.approved.contains(&config) && !authorizer.authorize(&config) {
            return Err(LspError::AuthorizationDenied(config.id));
        }
        let mut server = LspServer::spawn(&config)?;
        server.initialize(initialization_timeout)?;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(LspError::IdentifierExhausted)?;
        let id = ServerId(self.next_id);
        self.approved.insert(config.clone());
        self.configurations.insert(id, config);
        self.servers.insert(id, server);
        Ok(id)
    }

    /// Returns mutable access to one active server transport.
    ///
    /// # Errors
    /// Returns an error when the server identifier is not active.
    pub fn server_mut(&mut self, id: ServerId) -> Result<&mut LspServer, LspError> {
        self.servers
            .get_mut(&id)
            .ok_or(LspError::ServerNotFound(id))
    }

    /// Gracefully shuts down and removes one server.
    ///
    /// # Errors
    /// Returns an error when the server identifier is not active.
    pub fn stop_server(&mut self, id: ServerId) -> Result<(), LspError> {
        let mut server = self
            .servers
            .remove(&id)
            .ok_or(LspError::ServerNotFound(id))?;
        self.configurations.remove(&id);
        server.shutdown();
        Ok(())
    }

    /// Stops all servers in a workspace and clears its restart approvals.
    ///
    /// # Errors
    /// Returns an error when the workspace path is invalid or a server cannot be stopped.
    pub fn close_workspace(&mut self, workspace: &Path) -> Result<(), LspError> {
        let canonical = fs::canonicalize(workspace)
            .map_err(|error| LspError::InvalidConfiguration(error.to_string()))?;
        let ids = self
            .configurations
            .iter()
            .filter_map(|(id, config)| (config.workspace_root == canonical).then_some(*id))
            .collect::<Vec<_>>();
        for id in ids {
            self.stop_server(id)?;
        }
        self.approved
            .retain(|config| config.workspace_root != canonical);
        Ok(())
    }

    /// Stops all active servers.
    pub fn shutdown(&mut self) {
        for (_, mut server) in std::mem::take(&mut self.servers) {
            server.shutdown();
        }
        self.configurations.clear();
        self.approved.clear();
    }
}

/// One initialized JSON-RPC LSP process.
pub struct LspServer {
    config_id: String,
    workspace_root: PathBuf,
    child: Child,
    input: BufWriter<ChildStdin>,
    incoming: Receiver<Result<Value, String>>,
    next_request_id: u64,
    capabilities: Value,
    events: VecDeque<ServerEvent>,
}

impl fmt::Debug for LspServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LspServer")
            .field("config_id", &self.config_id)
            .field("workspace_root", &self.workspace_root)
            .field("pid", &self.child.id())
            .field("capabilities", &self.capabilities)
            .finish_non_exhaustive()
    }
}

impl LspServer {
    fn spawn(config: &ServerConfig) -> Result<Self, LspError> {
        let mut command = Command::new(&config.command);
        command
            .args(&config.arguments)
            .current_dir(&config.workspace_root)
            .env_clear()
            .envs(&config.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| LspError::SpawnFailed {
            command: config.command.clone(),
            message: error.to_string(),
        })?;
        let input = BufWriter::new(child.stdin.take().ok_or(LspError::MissingPipe("stdin"))?);
        let stdout = child.stdout.take().ok_or(LspError::MissingPipe("stdout"))?;
        let stderr = child.stderr.take().ok_or(LspError::MissingPipe("stderr"))?;
        let (sender, incoming) = mpsc::sync_channel(512);
        spawn_stdout_reader(stdout, sender.clone());
        spawn_stderr_reader(stderr, sender);
        Ok(Self {
            config_id: config.id.clone(),
            workspace_root: config.workspace_root.clone(),
            child,
            input,
            incoming,
            next_request_id: 1,
            capabilities: Value::Null,
            events: VecDeque::new(),
        })
    }

    /// Stable source identifier used for diagnostic aggregation.
    #[must_use]
    pub fn provider_id(&self) -> &str {
        &self.config_id
    }

    /// Negotiated server capabilities from initialize.
    #[must_use]
    pub fn capabilities(&self) -> &Value {
        &self.capabilities
    }

    fn initialize(&mut self, timeout: Duration) -> Result<(), LspError> {
        let root_uri = path_to_uri(&self.workspace_root);
        let result = self.request("initialize", json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "workspaceFolders": [{ "uri": root_uri, "name": self.workspace_root.file_name().unwrap_or_default().to_string_lossy() }],
            "clientInfo": { "name": "Koren IDE Platform", "version": env!("CARGO_PKG_VERSION") },
            "capabilities": {
                "general": { "positionEncodings": ["utf-16"] },
                "textDocument": {
                    "hover": { "dynamicRegistration": false },
                    "definition": { "dynamicRegistration": false },
                    "completion": { "completionItem": { "snippetSupport": false } },
                    "references": { "dynamicRegistration": false },
                    "rename": { "dynamicRegistration": false, "prepareSupport": true },
                    "formatting": { "dynamicRegistration": false },
                    "rangeFormatting": { "dynamicRegistration": false },
                    "codeAction": {
                        "dynamicRegistration": false,
                        "codeActionLiteralSupport": { "codeActionKind": { "valueSet": ["quickfix", "refactor", "refactor.extract", "refactor.inline", "refactor.rewrite", "source", "source.organizeImports"] } },
                        "resolveSupport": { "properties": ["edit", "command", "data"] }
                    },
                    "publishDiagnostics": { "relatedInformation": true }
                },
                "workspace": { "workspaceEdit": { "documentChanges": true, "resourceOperations": [] } }
            }
        }), timeout, None)?;
        self.capabilities = result.get("capabilities").cloned().unwrap_or(Value::Null);
        self.notify("initialized", json!({}))?;
        Ok(())
    }

    /// Opens a document using the authoritative Koren document snapshot.
    ///
    /// # Errors
    /// Returns an error when the notification cannot be written to the server.
    pub fn did_open(
        &mut self,
        uri: &str,
        language_id: &str,
        version: i32,
        text: &str,
    ) -> Result<(), LspError> {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri, "languageId": language_id, "version": version, "text": text
            }}),
        )
    }

    /// Sends a full-text update for a newer document version.
    ///
    /// # Errors
    /// Returns an error when the notification cannot be written to the server.
    pub fn did_change(&mut self, uri: &str, version: i32, text: &str) -> Result<(), LspError> {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [{ "text": text }]
            }),
        )
    }

    /// Closes a document on this server.
    ///
    /// # Errors
    /// Returns an error when the notification cannot be written to the server.
    pub fn did_close(&mut self, uri: &str) -> Result<(), LspError> {
        self.notify(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri } }),
        )
    }

    /// Sends one validated synchronization notification produced by [`LspDocumentSync`].
    ///
    /// # Errors
    /// Returns an error when the notification cannot be written to the server.
    pub fn send_sync_notification(
        &mut self,
        notification: &SyncNotification,
    ) -> Result<(), LspError> {
        self.notify(notification.method, notification.params.clone())
    }

    /// Requests hover information when the server advertised hover support.
    ///
    /// # Errors
    /// Returns an error when the server lacks hover support or the request fails.
    pub fn hover(
        &mut self,
        uri: &str,
        version: i32,
        position: TextPosition,
    ) -> Result<VersionedResult<Option<HoverResult>>, LspError> {
        self.hover_with_timeout(uri, version, position, DEFAULT_REQUEST_TIMEOUT)
    }

    /// Requests hover with a host-selected maximum wait.
    ///
    /// # Errors
    /// Returns an error when the server lacks hover support or the request fails.
    pub fn hover_with_timeout(
        &mut self,
        uri: &str,
        version: i32,
        position: TextPosition,
        timeout: Duration,
    ) -> Result<VersionedResult<Option<HoverResult>>, LspError> {
        self.hover_with_timeout_cancelled(uri, version, position, timeout, None)
    }

    pub(crate) fn hover_with_cancellation(
        &mut self,
        uri: &str,
        version: i32,
        position: TextPosition,
        timeout: Duration,
        cancelled: &AtomicBool,
    ) -> Result<VersionedResult<Option<HoverResult>>, LspError> {
        self.hover_with_timeout_cancelled(uri, version, position, timeout, Some(cancelled))
    }

    fn hover_with_timeout_cancelled(
        &mut self,
        uri: &str,
        version: i32,
        position: TextPosition,
        timeout: Duration,
        cancelled: Option<&AtomicBool>,
    ) -> Result<VersionedResult<Option<HoverResult>>, LspError> {
        self.ensure_capability("hoverProvider")?;
        let result = self.request(
            "textDocument/hover",
            text_position_params(uri, position),
            timeout,
            cancelled,
        )?;
        Ok(VersionedResult {
            uri: uri.into(),
            document_version: version,
            value: parse_hover(&result)?,
        })
    }

    /// Requests completion items when the server advertised completion support.
    ///
    /// # Errors
    /// Returns an error when the server lacks completion support or the request fails.
    pub fn completion(
        &mut self,
        uri: &str,
        version: i32,
        position: TextPosition,
    ) -> Result<VersionedResult<Vec<CompletionItem>>, LspError> {
        self.completion_with_timeout(uri, version, position, DEFAULT_REQUEST_TIMEOUT)
    }

    /// Requests completions with a host-selected maximum wait.
    ///
    /// # Errors
    /// Returns an error when the server lacks completion support or the request fails.
    pub fn completion_with_timeout(
        &mut self,
        uri: &str,
        version: i32,
        position: TextPosition,
        timeout: Duration,
    ) -> Result<VersionedResult<Vec<CompletionItem>>, LspError> {
        self.completion_with_timeout_cancelled(uri, version, position, timeout, None)
    }

    pub(crate) fn completion_with_cancellation(
        &mut self,
        uri: &str,
        version: i32,
        position: TextPosition,
        timeout: Duration,
        cancelled: &AtomicBool,
    ) -> Result<VersionedResult<Vec<CompletionItem>>, LspError> {
        self.completion_with_timeout_cancelled(uri, version, position, timeout, Some(cancelled))
    }

    fn completion_with_timeout_cancelled(
        &mut self,
        uri: &str,
        version: i32,
        position: TextPosition,
        timeout: Duration,
        cancelled: Option<&AtomicBool>,
    ) -> Result<VersionedResult<Vec<CompletionItem>>, LspError> {
        self.ensure_capability("completionProvider")?;
        let result = self.request(
            "textDocument/completion",
            text_position_params(uri, position),
            timeout,
            cancelled,
        )?;
        Ok(VersionedResult {
            uri: uri.into(),
            document_version: version,
            value: parse_completion(result)?,
        })
    }

    /// Requests definitions when the server advertised definition support.
    ///
    /// # Errors
    /// Returns an error when the server lacks definition support or the request fails.
    pub fn definition(
        &mut self,
        uri: &str,
        version: i32,
        position: TextPosition,
    ) -> Result<VersionedResult<Vec<Location>>, LspError> {
        self.definition_with_timeout(uri, version, position, DEFAULT_REQUEST_TIMEOUT)
    }

    /// Requests definitions with a host-selected maximum wait.
    ///
    /// # Errors
    /// Returns an error when the server lacks definition support or the request fails.
    pub fn definition_with_timeout(
        &mut self,
        uri: &str,
        version: i32,
        position: TextPosition,
        timeout: Duration,
    ) -> Result<VersionedResult<Vec<Location>>, LspError> {
        self.definition_with_timeout_cancelled(uri, version, position, timeout, None)
    }

    pub(crate) fn definition_with_cancellation(
        &mut self,
        uri: &str,
        version: i32,
        position: TextPosition,
        timeout: Duration,
        cancelled: &AtomicBool,
    ) -> Result<VersionedResult<Vec<Location>>, LspError> {
        self.definition_with_timeout_cancelled(uri, version, position, timeout, Some(cancelled))
    }

    fn definition_with_timeout_cancelled(
        &mut self,
        uri: &str,
        version: i32,
        position: TextPosition,
        timeout: Duration,
        cancelled: Option<&AtomicBool>,
    ) -> Result<VersionedResult<Vec<Location>>, LspError> {
        self.ensure_capability("definitionProvider")?;
        let result = self.request(
            "textDocument/definition",
            text_position_params(uri, position),
            timeout,
            cancelled,
        )?;
        Ok(VersionedResult {
            uri: uri.into(),
            document_version: version,
            value: parse_locations(result)?,
        })
    }

    /// Requests quick fixes, refactorings, source actions, and provider intentions for a range.
    ///
    /// An empty diagnostics list is valid and allows intention actions. Returned commands remain
    /// inert data; the host must present them and apply any edits only after explicit user choice.
    ///
    /// # Errors
    /// Returns an error when the server does not advertise code actions or the request fails.
    pub fn code_actions(
        &mut self,
        uri: &str,
        version: i32,
        range: TextRange,
        diagnostics: &[Value],
        only: &[String],
        timeout: Duration,
    ) -> Result<VersionedResult<Vec<LspCodeAction>>, LspError> {
        self.ensure_capability("codeActionProvider")?;
        let mut context = json!({ "diagnostics": diagnostics });
        if !only.is_empty() {
            context["only"] = json!(only);
        }
        let result = self.request(
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": uri }, "range": range, "context": context
            }),
            timeout,
            None,
        )?;
        Ok(VersionedResult {
            uri: uri.into(),
            document_version: version,
            value: parse_code_actions(&self.config_id, &result)?,
        })
    }

    /// Resolves an advertised deferred code action.
    ///
    /// # Errors
    /// Returns an error when `codeActionProvider.resolveProvider` was not advertised.
    pub fn resolve_code_action(
        &mut self,
        action: Value,
        timeout: Duration,
    ) -> Result<LspCodeAction, LspError> {
        if self
            .capabilities
            .pointer("/codeActionProvider/resolveProvider")
            != Some(&Value::Bool(true))
        {
            return Err(LspError::UnsupportedFeature(
                "codeActionProvider.resolveProvider".into(),
            ));
        }
        let result = self.request("codeAction/resolve", action, timeout, None)?;
        parse_code_actions(&self.config_id, &json!([result]))?
            .into_iter()
            .next()
            .ok_or_else(|| LspError::Protocol("resolved code action was empty".into()))
    }

    /// Requests references at a position when the server supports them.
    ///
    /// # Errors
    /// Returns an error when references are unsupported or the request fails.
    pub fn references(
        &mut self,
        uri: &str,
        version: i32,
        position: TextPosition,
        include_declaration: bool,
        timeout: Duration,
    ) -> Result<VersionedResult<Vec<Location>>, LspError> {
        self.ensure_capability("referencesProvider")?;
        let mut params = text_position_params(uri, position);
        params["context"] = json!({ "includeDeclaration": include_declaration });
        let result = self.request("textDocument/references", params, timeout, None)?;
        Ok(VersionedResult {
            uri: uri.into(),
            document_version: version,
            value: parse_locations(result)?,
        })
    }

    /// Requests whether a symbol can be renamed and which range/name should be used.
    ///
    /// # Errors
    /// Returns an error when rename is unsupported or the request fails.
    pub fn prepare_rename(
        &mut self,
        uri: &str,
        version: i32,
        position: TextPosition,
        timeout: Duration,
    ) -> Result<VersionedResult<Option<RenamePreparation>>, LspError> {
        self.ensure_capability("renameProvider")?;
        let result = self.request(
            "textDocument/prepareRename",
            text_position_params(uri, position),
            timeout,
            None,
        )?;
        let value = if result.is_null() {
            None
        } else {
            let range = result
                .get("range")
                .cloned()
                .ok_or_else(|| LspError::Protocol("prepareRename omitted range".into()))?;
            Some(RenamePreparation {
                range: serde_json::from_value(range)
                    .map_err(|error| LspError::Protocol(error.to_string()))?,
                placeholder: result
                    .get("placeholder")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        };
        Ok(VersionedResult {
            uri: uri.into(),
            document_version: version,
            value,
        })
    }

    /// Requests a rename workspace edit. The returned edit is not applied by this client.
    ///
    /// # Errors
    /// Returns an error when rename is unsupported or the request fails.
    pub fn rename(
        &mut self,
        uri: &str,
        version: i32,
        position: TextPosition,
        new_name: &str,
        timeout: Duration,
    ) -> Result<VersionedResult<Option<WorkspaceEdit>>, LspError> {
        self.ensure_capability("renameProvider")?;
        let mut params = text_position_params(uri, position);
        params["newName"] = json!(new_name);
        let result = self.request("textDocument/rename", params, timeout, None)?;
        let edit = if result.is_null() {
            None
        } else {
            Some(parse_workspace_edit(&result)?)
        };
        Ok(VersionedResult {
            uri: uri.into(),
            document_version: version,
            value: edit,
        })
    }

    /// Requests full-document formatting when advertised by the server.
    ///
    /// # Errors
    /// Returns an error when formatting is unsupported or the request fails.
    pub fn formatting(
        &mut self,
        uri: &str,
        version: i32,
        options: FormattingOptions,
        timeout: Duration,
    ) -> Result<VersionedResult<Vec<TextEdit>>, LspError> {
        self.ensure_capability("documentFormattingProvider")?;
        let result = self.request(
            "textDocument/formatting",
            json!({
                "textDocument": { "uri": uri }, "options": format_options(options)
            }),
            timeout,
            None,
        )?;
        Ok(VersionedResult {
            uri: uri.into(),
            document_version: version,
            value: parse_text_edits(&result)?,
        })
    }

    /// Requests formatting for one range when advertised by the server.
    ///
    /// # Errors
    /// Returns an error when range formatting is unsupported or the request fails.
    pub fn range_formatting(
        &mut self,
        uri: &str,
        version: i32,
        range: TextRange,
        options: FormattingOptions,
        timeout: Duration,
    ) -> Result<VersionedResult<Vec<TextEdit>>, LspError> {
        self.ensure_capability("documentRangeFormattingProvider")?;
        let result = self.request(
            "textDocument/rangeFormatting",
            json!({
                "textDocument": { "uri": uri }, "range": range, "options": format_options(options)
            }),
            timeout,
            None,
        )?;
        Ok(VersionedResult {
            uri: uri.into(),
            document_version: version,
            value: parse_text_edits(&result)?,
        })
    }

    /// Drains server notifications and stderr/errors currently available.
    pub fn drain_events(&mut self) -> Vec<ServerEvent> {
        while let Ok(message) = self.incoming.try_recv() {
            self.consume(message);
        }
        self.events.drain(..).collect()
    }

    /// Returns whether the child is currently alive, reporting a process status error.
    ///
    /// # Errors
    /// Returns an error when the process status cannot be queried.
    pub fn is_running(&mut self) -> Result<bool, LspError> {
        self.child
            .try_wait()
            .map(|status| status.is_none())
            .map_err(|error| LspError::Transport(error.to_string()))
    }

    /// Gracefully requests shutdown and terminates the child if it does not exit promptly.
    pub fn shutdown(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return;
        }
        let _ = self.request("shutdown", Value::Null, Duration::from_millis(300), None);
        let _ = self.notify("exit", Value::Null);
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn ensure_capability(&self, key: &str) -> Result<(), LspError> {
        let capability = self.capabilities.get(key);
        if capability.is_some_and(|value| !value.is_null() && value != &Value::Bool(false)) {
            Ok(())
        } else {
            Err(LspError::UnsupportedFeature(key.into()))
        }
    }

    pub(crate) fn request_feature_with_cancellation(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancelled: &AtomicBool,
    ) -> Result<Value, LspError> {
        let capability = match method {
            "textDocument/hover" => "hoverProvider",
            "textDocument/completion" => "completionProvider",
            "textDocument/definition" => "definitionProvider",
            "textDocument/codeAction" => "codeActionProvider",
            "textDocument/references" => "referencesProvider",
            "textDocument/prepareRename" | "textDocument/rename" => "renameProvider",
            "textDocument/formatting" => "documentFormattingProvider",
            "textDocument/rangeFormatting" => "documentRangeFormattingProvider",
            _ => return Err(LspError::UnsupportedFeature(method.to_owned())),
        };
        self.ensure_capability(capability)?;
        self.request(method, params, timeout, Some(cancelled))
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), LspError> {
        write_message(&mut self.input, &rpc_envelope(method, None, params))
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancelled: Option<&AtomicBool>,
    ) -> Result<Value, LspError> {
        let id = self.next_request_id;
        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .ok_or(LspError::IdentifierExhausted)?;
        write_message(&mut self.input, &rpc_envelope(method, Some(id), params))?;
        let deadline = Instant::now() + timeout;
        loop {
            if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
                let _ = self.notify("$/cancelRequest", json!({ "id": id }));
                return Err(LspError::RequestCancelled);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let _ = self.notify("$/cancelRequest", json!({ "id": id }));
                return Err(LspError::RequestTimeout(method.into()));
            }
            let wait = cancelled.map_or(remaining, |_| remaining.min(Duration::from_millis(10)));
            match self.incoming.recv_timeout(wait) {
                Ok(Ok(message)) if message.get("id").and_then(Value::as_u64) == Some(id) => {
                    if let Some(error) = message.get("error") {
                        return Err(LspError::ServerResponse(error.to_string()));
                    }
                    return Ok(message.get("result").cloned().unwrap_or(Value::Null));
                }
                Ok(message) => self.consume(message),
                Err(RecvTimeoutError::Timeout) => {
                    if Instant::now() >= deadline {
                        let _ = self.notify("$/cancelRequest", json!({ "id": id }));
                        return Err(LspError::RequestTimeout(method.into()));
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return Err(LspError::ServerTerminated),
            }
        }
    }

    fn consume(&mut self, message: Result<Value, String>) {
        match message {
            Err(error) => self.events.push_back(ServerEvent::Error(error)),
            Ok(value) => {
                let Some(method) = value.get("method").and_then(Value::as_str) else {
                    return;
                };
                let params = value.get("params").cloned().unwrap_or(Value::Null);
                if method == "textDocument/publishDiagnostics" {
                    match parse_diagnostics(&self.config_id, &params) {
                        Ok(diagnostics) => {
                            self.events.push_back(ServerEvent::Diagnostics(diagnostics));
                        }
                        Err(error) => self.events.push_back(ServerEvent::Error(error.to_string())),
                    }
                } else if method == "$/koren/stderr" {
                    if let Some(message) = params.get("message").and_then(Value::as_str) {
                        self.events
                            .push_back(ServerEvent::Stderr(message.to_owned()));
                    }
                } else {
                    self.events.push_back(ServerEvent::Notification {
                        method: method.into(),
                        params,
                    });
                }
            }
        }
    }
}

impl Drop for LspServer {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn spawn_stdout_reader(stdout: ChildStdout, sender: SyncSender<Result<Value, String>>) {
    thread::Builder::new()
        .name("koren-lsp-stdout".into())
        .spawn(move || {
            let mut input = BufReader::new(stdout);
            loop {
                match read_message(&mut input) {
                    Ok(Some(message)) => match sender.try_send(Ok(message)) {
                        Ok(()) | Err(TrySendError::Full(_)) => {}
                        Err(TrySendError::Disconnected(_)) => break,
                    },
                    Ok(None) => break,
                    Err(error) => {
                        let _ = sender.send(Err(error.to_string()));
                        break;
                    }
                }
            }
        })
        .expect("failed to start LSP stdout reader");
}

fn spawn_stderr_reader(stderr: ChildStderr, sender: SyncSender<Result<Value, String>>) {
    thread::Builder::new()
        .name("koren-lsp-stderr".into())
        .spawn(move || {
            for line in BufReader::new(stderr).lines() {
                match line {
                    Ok(text) if !text.is_empty() => {
                        let _ = sender.try_send(Ok(
                            json!({"method":"$/koren/stderr", "params":{"message":text}}),
                        ));
                    }
                    Ok(_) => {}
                    Err(error) => {
                        let _ = sender.send(Err(error.to_string()));
                        break;
                    }
                }
            }
        })
        .expect("failed to start LSP stderr reader");
}

fn rpc_envelope(method: &str, id: Option<u64>, params: Value) -> Value {
    let mut message = serde_json::Map::new();
    message.insert("jsonrpc".into(), Value::String("2.0".into()));
    message.insert("method".into(), Value::String(method.into()));
    if let Some(id) = id {
        message.insert("id".into(), Value::from(id));
    }
    message.insert("params".into(), params);
    Value::Object(message)
}

fn write_message(output: &mut impl Write, message: &Value) -> Result<(), LspError> {
    let bytes =
        serde_json::to_vec(&message).map_err(|error| LspError::Protocol(error.to_string()))?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(LspError::MessageTooLarge(bytes.len()));
    }
    write!(output, "Content-Length: {}\r\n\r\n", bytes.len()).map_err(|error| io_error(&error))?;
    output.write_all(&bytes).map_err(|error| io_error(&error))?;
    output.flush().map_err(|error| io_error(&error))
}

/// Reads one Content-Length framed JSON-RPC message.
///
/// # Errors
/// Returns an error for malformed headers or JSON, oversized messages, and I/O failures.
pub fn read_message(input: &mut impl BufRead) -> Result<Option<Value>, LspError> {
    let mut length = None;
    let mut saw_header = false;
    loop {
        let mut line = String::new();
        let count = input
            .read_line(&mut line)
            .map_err(|error| io_error(&error))?;
        if count == 0 {
            return if saw_header {
                Err(LspError::Protocol("EOF in LSP headers".into()))
            } else {
                Ok(None)
            };
        }
        saw_header = true;
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                if length.is_some() {
                    return Err(LspError::Protocol("duplicate Content-Length header".into()));
                }
                let parsed = value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| LspError::Protocol("invalid Content-Length header".into()))?;
                if parsed > MAX_MESSAGE_BYTES {
                    return Err(LspError::MessageTooLarge(parsed));
                }
                length = Some(parsed);
            }
        }
    }
    let length =
        length.ok_or_else(|| LspError::Protocol("missing Content-Length header".into()))?;
    let mut bytes = vec![0; length];
    input
        .read_exact(&mut bytes)
        .map_err(|error| io_error(&error))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| LspError::Protocol(error.to_string()))
}

fn io_error(error: &io::Error) -> LspError {
    LspError::Transport(error.to_string())
}

fn text_position_params(uri: &str, position: TextPosition) -> Value {
    json!({ "textDocument": { "uri": uri }, "position": position })
}

fn parse_hover(value: &Value) -> Result<Option<HoverResult>, LspError> {
    if value.is_null() {
        return Ok(None);
    }
    let contents = value
        .get("contents")
        .ok_or_else(|| LspError::Protocol("hover result omitted contents".into()))?;
    let text = match contents {
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(markup_text).collect::<Vec<_>>().join("\n"),
        Value::Object(_) => markup_text(contents),
        _ => return Err(LspError::Protocol("invalid hover contents".into())),
    };
    let range = value
        .get("range")
        .map(|range| serde_json::from_value(range.clone()))
        .transpose()
        .map_err(|error| LspError::Protocol(error.to_string()))?;
    Ok(Some(HoverResult {
        contents: text,
        range,
    }))
}

fn markup_text(value: &Value) -> String {
    value
        .get("value")
        .and_then(Value::as_str)
        .or_else(|| value.as_str())
        .unwrap_or("")
        .to_owned()
}

fn parse_completion(value: Value) -> Result<Vec<CompletionItem>, LspError> {
    let items = if value.is_array() {
        value
    } else {
        value
            .get("items")
            .cloned()
            .unwrap_or(Value::Array(Vec::new()))
    };
    items
        .as_array()
        .ok_or_else(|| LspError::Protocol("invalid completion result".into()))?
        .iter()
        .map(|item| {
            let label = item
                .get("label")
                .and_then(Value::as_str)
                .ok_or_else(|| LspError::Protocol("completion item omitted label".into()))?
                .to_owned();
            let documentation = item.get("documentation").map(markup_text);
            Ok(CompletionItem {
                label,
                kind: item
                    .get("kind")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok()),
                detail: item
                    .get("detail")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                insert_text: item
                    .get("insertText")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                documentation,
            })
        })
        .collect()
}

fn parse_locations(value: Value) -> Result<Vec<Location>, LspError> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let values = value.as_array().cloned().unwrap_or_else(|| vec![value]);
    values
        .into_iter()
        .map(|location| {
            let uri = location
                .get("uri")
                .and_then(Value::as_str)
                .ok_or_else(|| LspError::Protocol("definition omitted URI".into()))?
                .to_owned();
            let range = location
                .get("range")
                .cloned()
                .ok_or_else(|| LspError::Protocol("definition omitted range".into()))?;
            let range = serde_json::from_value(range)
                .map_err(|error| LspError::Protocol(error.to_string()))?;
            Ok(Location { uri, range })
        })
        .collect()
}

fn parse_text_edits(value: &Value) -> Result<Vec<TextEdit>, LspError> {
    let Some(edits) = value.as_array() else {
        if value.is_null() {
            return Ok(Vec::new());
        }
        return Err(LspError::Protocol(
            "text edit response was not an array".into(),
        ));
    };
    if edits.len() > MAX_WORKSPACE_EDIT_ITEMS {
        return Err(LspError::MessageTooLarge(edits.len()));
    }
    edits
        .iter()
        .map(|edit| {
            let range = edit
                .get("range")
                .cloned()
                .ok_or_else(|| LspError::Protocol("text edit omitted range".into()))?;
            let range = serde_json::from_value(range)
                .map_err(|error| LspError::Protocol(error.to_string()))?;
            let new_text = edit
                .get("newText")
                .and_then(Value::as_str)
                .ok_or_else(|| LspError::Protocol("text edit omitted newText".into()))?;
            Ok(TextEdit {
                range,
                new_text: new_text.to_owned(),
            })
        })
        .collect()
}

fn parse_workspace_edit(value: &Value) -> Result<WorkspaceEdit, LspError> {
    let mut result = WorkspaceEdit::default();
    if let Some(changes) = value.get("changes").and_then(Value::as_object) {
        for (uri, edits) in changes {
            result.document_edits.push(VersionedDocumentEdits {
                uri: uri.clone(),
                version: None,
                edits: parse_text_edits(edits)?,
            });
        }
    }
    if let Some(changes) = value.get("documentChanges").and_then(Value::as_array) {
        for change in changes {
            if let Some(document) = change.get("textDocument") {
                let uri = document
                    .get("uri")
                    .and_then(Value::as_str)
                    .ok_or_else(|| LspError::Protocol("document change omitted URI".into()))?;
                let version = document
                    .get("version")
                    .and_then(Value::as_i64)
                    .and_then(|version| i32::try_from(version).ok());
                let empty_edits = Value::Null;
                let edits = parse_text_edits(change.get("edits").unwrap_or(&empty_edits))?;
                result.document_edits.push(VersionedDocumentEdits {
                    uri: uri.to_owned(),
                    version,
                    edits,
                });
            } else {
                let kind = change
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                result.unsupported_operations.push(kind.to_owned());
            }
        }
    }
    if value.get("changes").is_none() && value.get("documentChanges").is_none() {
        return Err(LspError::Protocol(
            "workspace edit contained no supported change set".into(),
        ));
    }
    result
        .document_edits
        .sort_by(|left, right| left.uri.cmp(&right.uri));
    let edit_count = result
        .document_edits
        .iter()
        .map(|file| file.edits.len())
        .sum::<usize>();
    let text_bytes = result
        .document_edits
        .iter()
        .flat_map(|file| &file.edits)
        .map(|edit| edit.new_text.len())
        .sum::<usize>();
    if result.document_edits.len() > MAX_WORKSPACE_EDIT_FILES
        || edit_count > MAX_WORKSPACE_EDIT_ITEMS
        || text_bytes > MAX_WORKSPACE_EDIT_TEXT_BYTES
    {
        return Err(LspError::MessageTooLarge(text_bytes.max(edit_count)));
    }
    if result
        .document_edits
        .windows(2)
        .any(|pair| pair[0].uri == pair[1].uri)
    {
        result
            .unsupported_operations
            .push("multiple edit groups for one document".into());
    }
    Ok(result)
}

fn parse_code_actions(provider_id: &str, value: &Value) -> Result<Vec<LspCodeAction>, LspError> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let Some(actions) = value.as_array() else {
        return Err(LspError::Protocol(
            "code action response was not an array".into(),
        ));
    };
    if actions.len() > MAX_CODE_ACTIONS {
        return Err(LspError::MessageTooLarge(actions.len()));
    }
    actions
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let title = action
                .get("title")
                .and_then(Value::as_str)
                .ok_or_else(|| LspError::Protocol("code action omitted title".into()))?
                .to_owned();
            let edit = action.get("edit").map(parse_workspace_edit).transpose()?;
            let command_value = action.get("command").cloned();
            let command = command_value
                .map(|command| {
                    if command.is_string() {
                        Ok(ProviderCommand {
                            title: title.clone(),
                            command: command.as_str().unwrap_or_default().to_owned(),
                            arguments: Vec::new(),
                        })
                    } else {
                        let command_id = command
                            .get("command")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                LspError::Protocol("provider command omitted command".into())
                            })?;
                        Ok(ProviderCommand {
                            title: command
                                .get("title")
                                .and_then(Value::as_str)
                                .unwrap_or(&title)
                                .to_owned(),
                            command: command_id.to_owned(),
                            arguments: command
                                .get("arguments")
                                .and_then(Value::as_array)
                                .cloned()
                                .unwrap_or_default(),
                        })
                    }
                })
                .transpose()?;
            let related_diagnostics = action
                .get("diagnostics")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|diagnostic| {
                    diagnostic
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .collect();
            let data = action.get("data").cloned();
            if data.as_ref().is_some_and(|data| {
                serde_json::to_vec(data)
                    .map_or(true, |bytes| bytes.len() > MAX_CODE_ACTION_DATA_BYTES)
            }) {
                return Err(LspError::MessageTooLarge(
                    MAX_CODE_ACTION_DATA_BYTES.saturating_add(1),
                ));
            }
            Ok(LspCodeAction {
                id: format!("{provider_id}:{index}:{title}"),
                title,
                provider_id: provider_id.to_owned(),
                kind: action
                    .get("kind")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                related_diagnostics,
                edit,
                command,
                disabled_reason: action
                    .pointer("/disabled/reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                data,
            })
        })
        .collect()
}

fn format_options(options: FormattingOptions) -> Value {
    json!({ "tabSize": options.tab_size, "insertSpaces": options.insert_spaces })
}

fn parse_diagnostics(provider_id: &str, params: &Value) -> Result<DiagnosticSet, LspError> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| LspError::Protocol("diagnostics omitted URI".into()))?
        .to_owned();
    let version = params
        .get("version")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok());
    let values = params
        .get("diagnostics")
        .and_then(Value::as_array)
        .ok_or_else(|| LspError::Protocol("diagnostics omitted list".into()))?;
    let diagnostics = values
        .iter()
        .map(|value| {
            let range: TextRange = serde_json::from_value(
                value
                    .get("range")
                    .cloned()
                    .ok_or_else(|| LspError::Protocol("diagnostic omitted range".into()))?,
            )
            .map_err(|error| LspError::Protocol(error.to_string()))?;
            let severity = match value.get("severity").and_then(Value::as_u64) {
                Some(1) => Some(DiagnosticSeverity::Error),
                Some(2) => Some(DiagnosticSeverity::Warning),
                Some(3) => Some(DiagnosticSeverity::Information),
                Some(4) => Some(DiagnosticSeverity::Hint),
                _ => None,
            };
            let code = value.get("code").map(|code| {
                code.as_str()
                    .map_or_else(|| code.to_string(), str::to_owned)
            });
            let related_information = value
                .get("relatedInformation")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|related| {
                    Ok(RelatedDiagnostic {
                        uri: related
                            .pointer("/location/uri")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                LspError::Protocol("related diagnostic omitted URI".into())
                            })?
                            .to_owned(),
                        range: serde_json::from_value(
                            related.pointer("/location/range").cloned().ok_or_else(|| {
                                LspError::Protocol("related diagnostic omitted range".into())
                            })?,
                        )
                        .map_err(|error| LspError::Protocol(error.to_string()))?,
                        message: related
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                    })
                })
                .collect::<Result<Vec<_>, LspError>>()?;
            Ok(Diagnostic {
                uri: uri.clone(),
                range,
                severity,
                message: value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                source: value
                    .get("source")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                code,
                related_information,
                document_revision: None,
                document_version: version,
            })
        })
        .collect::<Result<Vec<_>, LspError>>()?;
    Ok(DiagnosticSet {
        provider_id: provider_id.into(),
        uri,
        document_version: version,
        document_id: None,
        diagnostics,
    })
}

/// Encodes a local path as a file URI for LSP requests and document synchronization.
#[must_use]
pub fn path_to_uri(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let mut result = String::from("file://");
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'-' | b'_' | b'.' | b'~') {
            result.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(result, "%{byte:02X}");
        }
    }
    result
}

/// Structured protocol, authorization, process, and document synchronization failures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LspError {
    /// Invalid process or workspace settings.
    InvalidConfiguration(String),
    /// Host rejected external process startup.
    AuthorizationDenied(String),
    /// Process failed to spawn.
    SpawnFailed {
        /// Configured executable.
        command: String,
        /// Operating-system spawn error.
        message: String,
    },
    /// Required child pipe was unavailable.
    MissingPipe(&'static str),
    /// JSON-RPC framing or payload is invalid.
    Protocol(String),
    /// Transport read or write failed.
    Transport(String),
    /// Server exited while a response was pending.
    ServerTerminated,
    /// A request exceeded its deadline.
    RequestTimeout(String),
    /// The host cancelled the request before its response arrived.
    RequestCancelled,
    /// Server returned an error response.
    ServerResponse(String),
    /// A requested feature was not advertised by the server.
    UnsupportedFeature(String),
    /// Incoming or outgoing message exceeded the configured bound.
    MessageTooLarge(usize),
    /// Document already has an LSP synchronization association.
    DocumentAlreadyOpen(u64),
    /// Document is not open in the synchronization map.
    DocumentNotOpen(u64),
    /// An obsolete or out-of-order document snapshot was supplied.
    StaleDocumentRevision {
        /// Document manager id.
        id: u64,
        /// Highest accepted document revision.
        current: u64,
        /// Rejected stale revision.
        received: u64,
    },
    /// Language-server document version or identifier space was exhausted.
    VersionExhausted,
    /// Manager-local server id space was exhausted.
    IdentifierExhausted,
    /// The bounded actor queue is full.
    RequestQueueFull,
    /// The server actor has not finished initialization.
    ServerNotReady(ServerId),
    /// Server manager does not contain the requested id.
    ServerNotFound(ServerId),
}

impl fmt::Display for LspError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration(message) => {
                write!(f, "invalid language-server configuration: {message}")
            }
            Self::AuthorizationDenied(id) => {
                write!(f, "host denied language-server startup for '{id}'")
            }
            Self::SpawnFailed { command, message } => {
                write!(f, "failed to start language server '{command}': {message}")
            }
            Self::MissingPipe(pipe) => write!(f, "language-server {pipe} pipe is unavailable"),
            Self::Protocol(message) => write!(f, "language-server protocol error: {message}"),
            Self::Transport(message) => write!(f, "language-server transport failed: {message}"),
            Self::ServerTerminated => {
                f.write_str("language server terminated while handling a request")
            }
            Self::RequestTimeout(method) => {
                write!(f, "language-server request '{method}' timed out")
            }
            Self::RequestCancelled => f.write_str("language-server request was cancelled"),
            Self::ServerResponse(message) => {
                write!(f, "language server returned an error: {message}")
            }
            Self::UnsupportedFeature(feature) => {
                write!(f, "language server does not support {feature}")
            }
            Self::MessageTooLarge(size) => {
                write!(f, "language-server message is too large ({size} bytes)")
            }
            Self::DocumentAlreadyOpen(id) => write!(f, "document {id} is already synchronized"),
            Self::DocumentNotOpen(id) => write!(f, "document {id} is not synchronized"),
            Self::StaleDocumentRevision {
                id,
                current,
                received,
            } => write!(
                f,
                "document {id} revision {received} is stale; current revision is {current}"
            ),
            Self::VersionExhausted => {
                f.write_str("language-server document version space is exhausted")
            }
            Self::IdentifierExhausted => {
                f.write_str("language-server identifier space is exhausted")
            }
            Self::RequestQueueFull => f.write_str("language-server request queue is full"),
            Self::ServerNotReady(id) => {
                write!(f, "language server {} is not initialized", id.get())
            }
            Self::ServerNotFound(id) => write!(f, "language server {} is not active", id.get()),
        }
    }
}

impl std::error::Error for LspError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Cursor,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    fn snapshot(revision: u64, text: &str) -> DocumentSnapshot {
        DocumentSnapshot {
            info: koren_documents::DocumentInfo {
                id: DocumentId(7),
                path: Some("src/main.demo".into()),
                revision,
                dirty: revision > 0,
            },
            text: text.into(),
        }
    }

    #[test]
    fn framing_round_trips_multiple_messages_and_rejects_invalid_headers() {
        let mut encoded = Vec::new();
        write_message(&mut encoded, &json!({"jsonrpc":"2.0","method":"one"})).unwrap();
        write_message(&mut encoded, &json!({"jsonrpc":"2.0","method":"two"})).unwrap();
        let mut input = Cursor::new(encoded);
        assert_eq!(read_message(&mut input).unwrap().unwrap()["method"], "one");
        assert_eq!(read_message(&mut input).unwrap().unwrap()["method"], "two");
        let mut invalid = Cursor::new(b"Content-Length: nope\r\n\r\n".as_slice());
        assert!(matches!(
            read_message(&mut invalid),
            Err(LspError::Protocol(_))
        ));
        let oversized = format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE_BYTES + 1);
        assert!(matches!(
            read_message(&mut Cursor::new(oversized)),
            Err(LspError::MessageTooLarge(_))
        ));
    }

    #[test]
    fn document_sync_uses_utf16_document_revisions_and_discards_old_changes() {
        let id = DocumentId(7);
        let mut sync = LspDocumentSync::default();
        let opened = sync
            .open(id, "file:///demo", "demo", &snapshot(0, "a😀b"))
            .unwrap();
        assert_eq!(opened.method, "textDocument/didOpen");
        assert_eq!(sync.version(id), Some((1, 0)));
        let changed = sync.change(id, &snapshot(4, "a😀bc")).unwrap();
        assert_eq!(changed.params["textDocument"]["version"], 2);
        assert_eq!(sync.version(id), Some((2, 4)));
        assert!(matches!(
            sync.change(id, &snapshot(2, "stale")),
            Err(LspError::StaleDocumentRevision { .. })
        ));
        assert!(
            !VersionedResult {
                uri: "file:///demo".into(),
                document_version: 1,
                value: ()
            }
            .is_current(2)
        );
        assert_eq!(sync.close(id).unwrap().method, "textDocument/didClose");
        assert!(sync.version(id).is_none());
        let reopened = sync
            .open(
                DocumentId(8),
                "file:///demo",
                "demo",
                &snapshot(0, "new contents"),
            )
            .unwrap();
        assert_eq!(reopened.params["textDocument"]["version"], 3);
        assert_eq!(sync.version(DocumentId(8)), Some((3, 0)));
        assert!(
            !VersionedResult {
                uri: "file:///demo".into(),
                document_version: 1,
                value: ()
            }
            .is_current(3),
            "a delayed diagnostic from the previous open must not match the reopened document"
        );
    }

    #[test]
    fn diagnostics_from_multiple_sources_are_typed_and_preserved() {
        let params = json!({"uri":"file:///demo", "version":3, "diagnostics":[{
            "range":{"start":{"line":0,"character":1},"end":{"line":0,"character":4}},
            "severity":2,"code":12,"source":"fake","message":"review this",
            "relatedInformation":[{"location":{"uri":"file:///other","range":{"start":{"line":1,"character":0},"end":{"line":1,"character":2}}},"message":"related"}]
        }]});
        let first = parse_diagnostics("server-a", &params).unwrap();
        let second = parse_diagnostics("server-b", &params).unwrap();
        assert_eq!(first.provider_id, "server-a");
        assert_eq!(
            first.diagnostics[0].severity,
            Some(DiagnosticSeverity::Warning)
        );
        assert_eq!(first.diagnostics[0].code.as_deref(), Some("12"));
        assert_eq!(first.diagnostics[0].document_version, Some(3));
        assert_eq!(first.document_version, Some(3));
        assert_eq!(
            first.diagnostics[0].related_information[0].uri,
            "file:///other"
        );
        assert_ne!(first.provider_id, second.provider_id);
    }

    #[test]
    fn advertised_capabilities_gate_typed_language_requests() {
        assert!(
            parse_hover(&json!({"contents":{"kind":"markdown","value":"hello"}}))
                .unwrap()
                .unwrap()
                .contents
                .contains("hello")
        );
        assert_eq!(
            parse_completion(
                json!({"isIncomplete":false,"items":[{"label":"alpha","kind":3,"detail":"fn"}]})
            )
            .unwrap()[0]
                .label,
            "alpha"
        );
        assert_eq!(parse_locations(json!({"uri":"file:///target","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}}})).unwrap().len(), 1);
        let edit = parse_workspace_edit(&json!({"documentChanges":[
            {"textDocument":{"uri":"file:///demo","version":2},"edits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"newText":"x"}]},
            {"kind":"rename","oldUri":"file:///a","newUri":"file:///b"}
        ]})).unwrap();
        assert_eq!(edit.document_edits[0].version, Some(2));
        assert_eq!(edit.unsupported_operations, ["rename"]);
        let actions = parse_code_actions("server", &json!([{"title":"Run provider command","command":{"title":"Run","command":"server.run","arguments":[1]}}])).unwrap();
        assert_eq!(actions[0].command.as_ref().unwrap().command, "server.run");
        let oversized_data = Value::String("x".repeat(MAX_CODE_ACTION_DATA_BYTES + 1));
        assert!(matches!(
            parse_code_actions(
                "server",
                &json!([{"title":"Resolve","data":oversized_data}])
            ),
            Err(LspError::MessageTooLarge(_))
        ));
    }

    struct Allow(Arc<AtomicUsize>);
    impl ServerAuthorizer for Allow {
        fn authorize(&mut self, _configuration: &ServerConfig) -> bool {
            self.0.fetch_add(1, Ordering::SeqCst);
            true
        }
    }
    struct Deny;
    impl ServerAuthorizer for Deny {
        fn authorize(&mut self, _configuration: &ServerConfig) -> bool {
            false
        }
    }

    fn config(workspace_root: PathBuf) -> ServerConfig {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fake_lsp_server.py");
        let command = if cfg!(windows) { "python" } else { "python3" };
        ServerConfig {
            id: "fake-lsp".into(),
            command: command.into(),
            arguments: vec![script.to_string_lossy().into_owned()],
            workspace_root,
            environment: BTreeMap::new(),
        }
    }

    #[test]
    fn authorized_fake_server_initializes_syncs_and_supports_requests() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("koren-lsp-{nonce}"));
        fs::create_dir_all(&root).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut authorizer = Allow(calls.clone());
        let mut manager = LspServerManager::new();
        let id = manager
            .start_server(config(root.clone()), &mut authorizer)
            .unwrap();
        let server = manager.server_mut(id).unwrap();
        assert!(server.capabilities()["hoverProvider"].as_bool().unwrap());
        let hover = server
            .hover(
                "file:///demo",
                1,
                TextPosition {
                    line: 0,
                    character: 1,
                },
            )
            .unwrap();
        assert_eq!(hover.value.unwrap().contents, "hover info");
        assert_eq!(
            server
                .completion(
                    "file:///demo",
                    1,
                    TextPosition {
                        line: 0,
                        character: 1
                    }
                )
                .unwrap()
                .value[0]
                .label,
            "complete"
        );
        assert_eq!(
            server
                .definition(
                    "file:///demo",
                    1,
                    TextPosition {
                        line: 0,
                        character: 1
                    }
                )
                .unwrap()
                .value,
            []
        );
        exercise_advanced_requests(server);
        server.did_open("file:///demo", "demo", 1, "hello").unwrap();
        thread::sleep(Duration::from_millis(25));
        assert!(
            server
                .drain_events()
                .iter()
                .any(|event| matches!(event, ServerEvent::Diagnostics(_)))
        );
        let id2 = manager
            .start_server(config(root.clone()), &mut authorizer)
            .unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "identical approved config reuses the session authorization"
        );
        manager.stop_server(id2).unwrap();
        manager.close_workspace(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    fn exercise_advanced_requests(server: &mut LspServer) {
        let range = TextRange {
            start: TextPosition {
                line: 0,
                character: 0,
            },
            end: TextPosition {
                line: 0,
                character: 4,
            },
        };
        let actions = server
            .code_actions(
                "file:///demo",
                1,
                range,
                &[],
                &["quickfix".into()],
                Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(actions.value[0].provider_id, "fake-lsp");
        assert_eq!(actions.value[0].kind.as_deref(), Some("quickfix"));
        assert_eq!(
            actions.value[0].edit.as_ref().unwrap().document_edits[0].edits[0].new_text,
            "demo"
        );
        assert!(actions.value[0].data.is_some());
        assert_eq!(
            server
                .references("file:///demo", 1, range.start, true, Duration::from_secs(1))
                .unwrap()
                .value
                .len(),
            1
        );
        assert_eq!(
            server
                .prepare_rename("file:///demo", 1, range.start, Duration::from_secs(1))
                .unwrap()
                .value
                .unwrap()
                .placeholder
                .as_deref(),
            Some("name")
        );
        let rename = server
            .rename(
                "file:///demo",
                1,
                range.start,
                "next",
                Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(
            rename.value.unwrap().document_edits[0].edits[0].new_text,
            "next"
        );
        let formatting = FormattingOptions {
            tab_size: 4,
            insert_spaces: true,
        };
        assert_eq!(
            server
                .formatting("file:///demo", 1, formatting, Duration::from_secs(1))
                .unwrap()
                .value[0]
                .new_text,
            "x"
        );
        assert_eq!(
            server
                .range_formatting("file:///demo", 1, range, formatting, Duration::from_secs(1))
                .unwrap()
                .value[0]
                .new_text,
            "x"
        );
    }

    #[test]
    fn server_start_requires_authorization_and_changed_config_is_rechecked() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("koren-lsp-deny-{nonce}"));
        fs::create_dir_all(&root).unwrap();
        let mut manager = LspServerManager::new();
        assert!(matches!(
            manager.start_server(config(root.clone()), &mut Deny),
            Err(LspError::AuthorizationDenied(_))
        ));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut allow = Allow(calls.clone());
        let id = manager
            .start_server(config(root.clone()), &mut allow)
            .unwrap();
        let mut changed = config(root.clone());
        changed.arguments.push("changed".into());
        assert!(matches!(
            manager.start_server(changed, &mut Deny),
            Err(LspError::AuthorizationDenied(_))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        manager.stop_server(id).unwrap();
        manager.shutdown();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn server_process_exit_during_initialize_is_reported_and_cleaned_up() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("koren-lsp-crash-{nonce}"));
        fs::create_dir_all(&root).unwrap();
        let mut config = config(root.clone());
        config.arguments = vec!["-c".into(), "pass".into()];
        let mut manager = LspServerManager::new();
        assert!(matches!(
            manager.start_server(config, &mut Allow(Arc::new(AtomicUsize::new(0)))),
            Err(LspError::ServerTerminated | LspError::RequestTimeout(_))
        ));
        manager.shutdown();
        fs::remove_dir_all(root).unwrap();
    }
}
