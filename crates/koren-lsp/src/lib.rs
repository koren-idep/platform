//! Language-neutral LSP process, JSON-RPC, diagnostics, and document-sync primitives.
//!
//! This crate does not discover or launch servers automatically. A host supplies an
//! authorization decision before each new command/workspace configuration is started.
//! Launch permission is not process isolation; approved servers run with the host user's OS
//! privileges and should only be trusted when configured by the user.

use koren_documents::{DocumentId, DocumentSnapshot};
use koren_protocol::{TextPosition, TextRange};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt, fs,
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    thread,
    time::{Duration, Instant},
};

/// Maximum LSP message body accepted from a language server.
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
/// Default request timeout used for initialization and language requests.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

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
        let version = 1;
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
            .get_mut(&id)
            .ok_or(LspError::DocumentNotOpen(id.0))?;
        if snapshot.info.revision <= document.revision {
            return Err(LspError::StaleDocumentRevision {
                id: id.0,
                current: document.revision,
                received: snapshot.info.revision,
            });
        }
        document.version = document
            .version
            .checked_add(1)
            .ok_or(LspError::VersionExhausted)?;
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
        mut config: ServerConfig,
        authorizer: &mut impl ServerAuthorizer,
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
        server.initialize(DEFAULT_REQUEST_TIMEOUT)?;
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
                    "publishDiagnostics": { "relatedInformation": true }
                }
            }
        }), timeout)?;
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
        self.ensure_capability("hoverProvider")?;
        let result = self.request(
            "textDocument/hover",
            text_position_params(uri, position),
            timeout,
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
        self.ensure_capability("completionProvider")?;
        let result = self.request(
            "textDocument/completion",
            text_position_params(uri, position),
            timeout,
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
        self.ensure_capability("definitionProvider")?;
        let result = self.request(
            "textDocument/definition",
            text_position_params(uri, position),
            timeout,
        )?;
        Ok(VersionedResult {
            uri: uri.into(),
            document_version: version,
            value: parse_locations(result)?,
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
        let _ = self.request("shutdown", Value::Null, Duration::from_millis(300));
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

    fn notify(&mut self, method: &str, params: Value) -> Result<(), LspError> {
        write_message(&mut self.input, &rpc_envelope(method, None, params))
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, LspError> {
        let id = self.next_request_id;
        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .ok_or(LspError::IdentifierExhausted)?;
        write_message(&mut self.input, &rpc_envelope(method, Some(id), params))?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(LspError::RequestTimeout(method.into()));
            }
            match self.incoming.recv_timeout(remaining) {
                Ok(Ok(message)) if message.get("id").and_then(Value::as_u64) == Some(id) => {
                    if let Some(error) = message.get("error") {
                        return Err(LspError::ServerResponse(error.to_string()));
                    }
                    return Ok(message.get("result").cloned().unwrap_or(Value::Null));
                }
                Ok(message) => self.consume(message),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(LspError::RequestTimeout(method.into()));
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
