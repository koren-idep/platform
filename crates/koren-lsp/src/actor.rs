//! Bounded, per-process actors for hosts that must keep LSP work off request threads.

use crate::{
    CompletionItem, DEFAULT_REQUEST_TIMEOUT, FormattingOptions, HoverResult, Location,
    LspCodeAction, LspError, LspServerManager, RenamePreparation, ServerAuthorizer, ServerConfig,
    ServerEvent, ServerId, SyncNotification, VersionedResult, WorkspaceEdit, parse_code_actions,
    parse_locations, parse_text_edits, parse_workspace_edit, text_position_params,
};
use koren_protocol::{TextPosition, TextRange};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
    time::Duration,
};

const ACTOR_QUEUE_CAPACITY: usize = 512;
const EVENT_QUEUE_CAPACITY: usize = 4096;

struct AuthorizedByHost;
impl ServerAuthorizer for AuthorizedByHost {
    fn authorize(&mut self, _configuration: &ServerConfig) -> bool {
        true
    }
}

/// One typed language request sent to a server actor.
#[derive(Clone, Debug)]
pub enum LspRequest {
    /// Request hover information.
    Hover {
        /// File URI from the synchronized document.
        uri: String,
        /// Monotonic server document version.
        version: i32,
        /// Zero-based UTF-16 position.
        position: TextPosition,
        /// Maximum server response wait.
        timeout: Duration,
    },
    /// Request completion items.
    Completion {
        /// File URI from the synchronized document.
        uri: String,
        /// Monotonic server document version.
        version: i32,
        /// Zero-based UTF-16 position.
        position: TextPosition,
        /// Maximum server response wait.
        timeout: Duration,
    },
    /// Request definition locations.
    Definition {
        /// File URI from the synchronized document.
        uri: String,
        /// Monotonic server document version.
        version: i32,
        /// Zero-based UTF-16 position.
        position: TextPosition,
        /// Maximum server response wait.
        timeout: Duration,
    },
    /// Request code actions, including intentions when diagnostics is empty.
    CodeActions {
        /// Synchronized file URI.
        uri: String,
        /// Current server document version.
        version: i32,
        /// Requested UTF-16 range.
        range: TextRange,
        /// Diagnostics in LSP wire form.
        diagnostics: Vec<Value>,
        /// Optional action kind filter.
        only: Vec<String>,
        /// Response deadline.
        timeout: Duration,
    },
    /// Find symbol references.
    References {
        /// Synchronized file URI.
        uri: String,
        /// Current server document version.
        version: i32,
        /// Query position.
        position: TextPosition,
        /// Whether to include the declaration.
        include_declaration: bool,
        /// Response deadline.
        timeout: Duration,
    },
    /// Prepare a rename request.
    PrepareRename {
        /// Synchronized file URI.
        uri: String,
        /// Current server document version.
        version: i32,
        /// Query position.
        position: TextPosition,
        /// Response deadline.
        timeout: Duration,
    },
    /// Request a rename workspace edit.
    Rename {
        /// Synchronized file URI.
        uri: String,
        /// Current server document version.
        version: i32,
        /// Query position.
        position: TextPosition,
        /// Proposed symbol name.
        new_name: String,
        /// Response deadline.
        timeout: Duration,
    },
    /// Request whole-document formatting edits.
    Formatting {
        /// Synchronized file URI.
        uri: String,
        /// Current server document version.
        version: i32,
        /// Formatting preferences.
        options: FormattingOptions,
        /// Response deadline.
        timeout: Duration,
    },
    /// Request range formatting edits.
    RangeFormatting {
        /// Synchronized file URI.
        uri: String,
        /// Current server document version.
        version: i32,
        /// Requested UTF-16 range.
        range: TextRange,
        /// Formatting preferences.
        options: FormattingOptions,
        /// Response deadline.
        timeout: Duration,
    },
}

/// Typed response from one language request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LspResponse {
    /// Hover content and version.
    Hover(VersionedResult<Option<HoverResult>>),
    /// Completion items and version.
    Completion(VersionedResult<Vec<CompletionItem>>),
    /// Definition locations and version.
    Definition(VersionedResult<Vec<Location>>),
    /// Code actions and intentions.
    CodeActions(VersionedResult<Vec<LspCodeAction>>),
    /// Symbol references.
    References(VersionedResult<Vec<Location>>),
    /// Rename preparation range.
    PrepareRename(VersionedResult<Option<RenamePreparation>>),
    /// Rename edit proposal.
    Rename(VersionedResult<Option<WorkspaceEdit>>),
    /// Full-document formatting edit proposal.
    Formatting(VersionedResult<Vec<koren_protocol::TextEdit>>),
    /// Range-formatting edit proposal.
    RangeFormatting(VersionedResult<Vec<koren_protocol::TextEdit>>),
}

/// A queued request response. Dropping or timing out the ticket cancels work that has not begun.
pub struct LspRequestTicket {
    receiver: Receiver<Result<LspResponse, LspError>>,
    cancelled: Arc<AtomicBool>,
}

impl LspRequestTicket {
    /// Checks for a completed response without waiting.
    ///
    /// # Errors
    /// Returns the server-side request error when the actor completed with failure.
    pub fn try_recv(&mut self) -> Result<Option<LspResponse>, LspError> {
        match self.receiver.try_recv() {
            Ok(result) => result.map(Some),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(LspError::ServerTerminated),
        }
    }

    /// Waits up to `timeout` for this request without blocking its server actor's host.
    ///
    /// # Errors
    /// Returns a transport, protocol, queue, or timeout error.
    pub fn recv_timeout(self, timeout: Duration) -> Result<LspResponse, LspError> {
        match self.receiver.recv_timeout(timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.cancelled.store(true, Ordering::Release);
                Err(LspError::RequestTimeout("queued language request".into()))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(LspError::ServerTerminated),
        }
    }
}

impl Drop for LspRequestTicket {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

/// Lifecycle or output event from one asynchronous language-server process.
#[derive(Clone, Debug, PartialEq)]
pub enum LspActorEvent {
    /// The server initialized and accepts document and feature requests.
    Initialized(ServerId),
    /// An LSP notification or diagnostic from the server.
    Server(ServerId, ServerEvent),
    /// Startup or process operation failed.
    Failed(ServerId, String),
    /// The actor and child process have stopped.
    Stopped(ServerId),
}

enum ActorCommand {
    Notification(SyncNotification),
    Request {
        request: LspRequest,
        cancelled: Arc<AtomicBool>,
        reply: SyncSender<Result<LspResponse, LspError>>,
    },
}

/// Cloneable, nonblocking access to one independently owned server actor.
#[derive(Clone)]
pub struct LspServerHandle {
    id: ServerId,
    commands: SyncSender<ActorCommand>,
    initialized: Arc<AtomicBool>,
    stop_requested: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
}

impl std::fmt::Debug for LspServerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspServerHandle")
            .field("id", &self.id)
            .field("initialized", &self.is_initialized())
            .finish_non_exhaustive()
    }
}

impl LspServerHandle {
    /// Returns this actor's manager-local identity.
    #[must_use]
    pub const fn id(&self) -> ServerId {
        self.id
    }

    /// Returns whether LSP initialization completed successfully.
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire) && !self.failed.load(Ordering::Acquire)
    }

    /// Enqueues a document notification without waiting for server I/O.
    ///
    /// Notifications share a bounded FIFO with requests. Queue overflow retires this server
    /// actor rather than silently leaving its document state inconsistent.
    ///
    /// # Errors
    /// Returns an error when the actor stopped or its bounded queue is full.
    pub fn notify(&self, notification: SyncNotification) -> Result<(), LspError> {
        match self
            .commands
            .try_send(ActorCommand::Notification(notification))
        {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.failed.store(true, Ordering::Release);
                self.stop_requested.store(true, Ordering::Release);
                Err(LspError::RequestQueueFull)
            }
            Err(TrySendError::Disconnected(_)) => Err(LspError::ServerTerminated),
        }
    }

    /// Enqueues a feature request and returns immediately with a response ticket.
    ///
    /// # Errors
    /// Returns an error when the actor stopped or its bounded request queue is full.
    pub fn request(&self, request: LspRequest) -> Result<LspRequestTicket, LspError> {
        if !self.is_initialized() {
            return Err(LspError::ServerNotReady(self.id));
        }
        let (reply, receiver) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let command = ActorCommand::Request {
            request,
            cancelled: cancelled.clone(),
            reply,
        };
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                TrySendError::Full(_) => LspError::RequestQueueFull,
                TrySendError::Disconnected(_) => LspError::ServerTerminated,
            })?;
        Ok(LspRequestTicket {
            receiver,
            cancelled,
        })
    }

    /// Requests actor shutdown without waiting for a stalled language request.
    pub fn stop(&self) {
        self.stop_requested.store(true, Ordering::Release);
    }
}

struct ActorRecord {
    config: ServerConfig,
    handle: LspServerHandle,
}

struct ActorStartup {
    id: ServerId,
    config: ServerConfig,
    receiver: Receiver<ActorCommand>,
    events: SyncSender<LspActorEvent>,
    initialized: Arc<AtomicBool>,
    stop_requested: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    initialization_timeout: Duration,
}

/// Owns independent server actors and their bounded lifecycle event queue.
///
/// Process startup and JSON-RPC are performed on one thread per server. All host-facing sends
/// are nonblocking. A stalled server can occupy only its own actor for the configured request
/// deadline; other servers and platform request workers remain independent.
pub struct LspServerActors {
    next_id: u64,
    records: BTreeMap<ServerId, ActorRecord>,
    approved: BTreeSet<ServerConfig>,
    event_tx: SyncSender<LspActorEvent>,
    event_rx: Receiver<LspActorEvent>,
}

impl std::fmt::Debug for LspServerActors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspServerActors")
            .field("server_count", &self.records.len())
            .field("approved_configuration_count", &self.approved.len())
            .finish_non_exhaustive()
    }
}

impl Default for LspServerActors {
    fn default() -> Self {
        let (event_tx, event_rx) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
        Self {
            next_id: 0,
            records: BTreeMap::new(),
            approved: BTreeSet::new(),
            event_tx,
            event_rx,
        }
    }
}

impl LspServerActors {
    /// Creates an empty actor manager.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts an authorized language server asynchronously and returns its identity immediately.
    ///
    /// The host must consume [`Self::drain_events`] to observe initialization and failure.
    /// An exact configuration approval is reused until the workspace is closed.
    ///
    /// # Errors
    /// Returns an error when configuration validation or host authorization fails, or identifiers
    /// are exhausted. Process startup and initialization failures arrive as actor events.
    pub fn start_server(
        &mut self,
        config: ServerConfig,
        authorizer: &mut impl ServerAuthorizer,
    ) -> Result<ServerId, LspError> {
        self.start_server_with_timeout(config, authorizer, DEFAULT_REQUEST_TIMEOUT)
    }

    /// Starts an authorized server actor with a host-selected initialization deadline.
    ///
    /// Startup dispatch still returns immediately; failure to initialize by the deadline is
    /// reported through [`Self::drain_events`].
    ///
    /// # Errors
    /// Returns an error when configuration validation or host authorization fails, or identifiers
    /// are exhausted. Process startup and initialization failures arrive as actor events.
    pub fn start_server_with_timeout(
        &mut self,
        mut config: ServerConfig,
        authorizer: &mut impl ServerAuthorizer,
        initialization_timeout: Duration,
    ) -> Result<ServerId, LspError> {
        config.validate()?;
        if let Some((id, _)) = self
            .records
            .iter()
            .find(|(_, record)| record.config == config)
        {
            return Ok(*id);
        }
        if !self.approved.contains(&config) && !authorizer.authorize(&config) {
            return Err(LspError::AuthorizationDenied(config.id));
        }
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(LspError::IdentifierExhausted)?;
        let id = ServerId(self.next_id);
        let (commands, receiver) = mpsc::sync_channel(ACTOR_QUEUE_CAPACITY);
        let initialized = Arc::new(AtomicBool::new(false));
        let stop_requested = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let handle = LspServerHandle {
            id,
            commands,
            initialized: initialized.clone(),
            stop_requested: stop_requested.clone(),
            failed: failed.clone(),
        };
        if let Err(error) = spawn_actor(ActorStartup {
            id,
            config: config.clone(),
            receiver,
            events: self.event_tx.clone(),
            initialized,
            stop_requested,
            failed,
            initialization_timeout,
        }) {
            return Err(LspError::Transport(format!(
                "could not start language-server actor thread: {error}"
            )));
        }
        self.approved.insert(config.clone());
        self.records.insert(id, ActorRecord { config, handle });
        Ok(id)
    }

    /// Returns a cloneable handle to one active or initializing actor.
    ///
    /// # Errors
    /// Returns [`LspError::ServerNotFound`] when the identifier is unknown.
    pub fn handle(&self, id: ServerId) -> Result<LspServerHandle, LspError> {
        self.records
            .get(&id)
            .map(|record| record.handle.clone())
            .ok_or(LspError::ServerNotFound(id))
    }

    /// Stops and removes a server without waiting for initialization or requests to finish.
    ///
    /// # Errors
    /// Returns [`LspError::ServerNotFound`] when the identifier is unknown.
    pub fn stop_server(&mut self, id: ServerId) -> Result<(), LspError> {
        let record = self
            .records
            .remove(&id)
            .ok_or(LspError::ServerNotFound(id))?;
        record.handle.stop();
        Ok(())
    }

    /// Stops servers for a workspace and discards their cached startup approvals.
    ///
    /// # Errors
    /// Returns an error if the workspace cannot be canonicalized.
    pub fn close_workspace(&mut self, workspace: &Path) -> Result<(), LspError> {
        let canonical = std::fs::canonicalize(workspace)
            .map_err(|error| LspError::InvalidConfiguration(error.to_string()))?;
        let ids = self
            .records
            .iter()
            .filter_map(|(id, record)| (record.config.workspace_root == canonical).then_some(*id))
            .collect::<Vec<_>>();
        for id in ids {
            let _ = self.stop_server(id);
        }
        self.approved
            .retain(|config| config.workspace_root != canonical);
        Ok(())
    }

    /// Drains queued lifecycle and server output events without blocking.
    #[must_use]
    pub fn drain_events(&self) -> Vec<LspActorEvent> {
        self.event_rx.try_iter().collect()
    }

    /// Requests shutdown of all actors without waiting for server I/O.
    pub fn shutdown(&mut self) {
        for (_, record) in std::mem::take(&mut self.records) {
            record.handle.stop();
        }
        self.approved.clear();
    }
}

fn spawn_actor(startup: ActorStartup) -> std::io::Result<()> {
    let id = startup.id;
    thread::Builder::new()
        .name(format!("koren-lsp-{}", id.get()))
        .spawn(move || run_actor(startup))
        .map(|_| ())
}

// This thread owns the receiver and cancellation state for its process. Keeping startup,
// dispatch, failure reporting, and teardown in one function makes every process exit path local.
#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
fn run_actor(startup: ActorStartup) {
    let ActorStartup {
        id,
        config,
        receiver,
        events,
        initialized,
        stop_requested,
        failed,
        initialization_timeout,
    } = startup;
    let mut manager = LspServerManager::new();
    let mut authorize = AuthorizedByHost;
    let server_id =
        match manager.start_server_with_timeout(config, &mut authorize, initialization_timeout) {
            Ok(server_id) => server_id,
            Err(error) => {
                failed.store(true, Ordering::Release);
                emit(
                    &events,
                    &failed,
                    LspActorEvent::Failed(id, error.to_string()),
                );
                emit(&events, &failed, LspActorEvent::Stopped(id));
                return;
            }
        };
    initialized.store(true, Ordering::Release);
    emit(&events, &failed, LspActorEvent::Initialized(id));
    loop {
        if stop_requested.load(Ordering::Acquire) {
            if failed.load(Ordering::Acquire) {
                emit(
                    &events,
                    &failed,
                    LspActorEvent::Failed(id, "language-server actor mailbox overflow".into()),
                );
            }
            break;
        }
        match receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(ActorCommand::Notification(notification)) => {
                let result = manager
                    .server_mut(server_id)
                    .and_then(|server| server.send_sync_notification(&notification));
                if let Err(error) = result {
                    failed.store(true, Ordering::Release);
                    emit(
                        &events,
                        &failed,
                        LspActorEvent::Failed(id, error.to_string()),
                    );
                    break;
                }
            }
            Ok(ActorCommand::Request {
                request,
                cancelled,
                reply,
            }) => {
                if cancelled.load(Ordering::Acquire) {
                    continue;
                }
                let result = manager
                    .server_mut(server_id)
                    .and_then(|server| match request {
                        LspRequest::Hover {
                            uri,
                            version,
                            position,
                            timeout,
                        } => server
                            .hover_with_cancellation(&uri, version, position, timeout, &cancelled)
                            .map(LspResponse::Hover),
                        LspRequest::Completion {
                            uri,
                            version,
                            position,
                            timeout,
                        } => server
                            .completion_with_cancellation(
                                &uri, version, position, timeout, &cancelled,
                            )
                            .map(LspResponse::Completion),
                        LspRequest::Definition {
                            uri,
                            version,
                            position,
                            timeout,
                        } => server
                            .definition_with_cancellation(
                                &uri, version, position, timeout, &cancelled,
                            )
                            .map(LspResponse::Definition),
                        LspRequest::CodeActions { uri, version, range, diagnostics, only, timeout } => {
                            let mut context = serde_json::json!({ "diagnostics": diagnostics });
                            if !only.is_empty() { context["only"] = serde_json::json!(only); }
                            let params = serde_json::json!({ "textDocument": { "uri": uri }, "range": range, "context": context });
                            server.request_feature_with_cancellation("textDocument/codeAction", params, timeout, &cancelled)
                                .and_then(|value| parse_code_actions(server.provider_id(), &value))
                                .map(|value| LspResponse::CodeActions(VersionedResult { uri, document_version: version, value }))
                        }
                        LspRequest::References { uri, version, position, include_declaration, timeout } => {
                            let mut params = text_position_params(&uri, position);
                            params["context"] = serde_json::json!({ "includeDeclaration": include_declaration });
                            server.request_feature_with_cancellation("textDocument/references", params, timeout, &cancelled)
                                .and_then(parse_locations)
                                .map(|value| LspResponse::References(VersionedResult { uri, document_version: version, value }))
                        }
                        LspRequest::PrepareRename { uri, version, position, timeout } => {
                            server.request_feature_with_cancellation("textDocument/prepareRename", text_position_params(&uri, position), timeout, &cancelled)
                                .and_then(|value| {
                                    if value.is_null() { return Ok(None); }
                                    let range = value.get("range").cloned().ok_or_else(|| LspError::Protocol("prepareRename omitted range".into()))?;
                                    let range = serde_json::from_value(range).map_err(|error| LspError::Protocol(error.to_string()))?;
                                    Ok(Some(RenamePreparation { range, placeholder: value.get("placeholder").and_then(Value::as_str).map(str::to_owned) }))
                                })
                                .map(|value| LspResponse::PrepareRename(VersionedResult { uri, document_version: version, value }))
                        }
                        LspRequest::Rename { uri, version, position, new_name, timeout } => {
                            let mut params = text_position_params(&uri, position);
                            params["newName"] = serde_json::json!(new_name);
                            server.request_feature_with_cancellation("textDocument/rename", params, timeout, &cancelled)
                                .and_then(|value| if value.is_null() { Ok(None) } else { parse_workspace_edit(&value).map(Some) })
                                .map(|value| LspResponse::Rename(VersionedResult { uri, document_version: version, value }))
                        }
                        LspRequest::Formatting { uri, version, options, timeout } => {
                            let params = serde_json::json!({ "textDocument": { "uri": uri }, "options": { "tabSize": options.tab_size, "insertSpaces": options.insert_spaces } });
                            server.request_feature_with_cancellation("textDocument/formatting", params, timeout, &cancelled)
                                .and_then(|value| parse_text_edits(&value))
                                .map(|value| LspResponse::Formatting(VersionedResult { uri, document_version: version, value }))
                        }
                        LspRequest::RangeFormatting { uri, version, range, options, timeout } => {
                            let params = serde_json::json!({ "textDocument": { "uri": uri }, "range": range, "options": { "tabSize": options.tab_size, "insertSpaces": options.insert_spaces } });
                            server.request_feature_with_cancellation("textDocument/rangeFormatting", params, timeout, &cancelled)
                                .and_then(|value| parse_text_edits(&value))
                                .map(|value| LspResponse::RangeFormatting(VersionedResult { uri, document_version: version, value }))
                        }
                    });
                if !cancelled.load(Ordering::Acquire) {
                    let _ = reply.send(result);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        let Ok(server) = manager.server_mut(server_id) else {
            break;
        };
        for event in server.drain_events() {
            emit(&events, &failed, LspActorEvent::Server(id, event));
        }
        match server.is_running() {
            Ok(true) => {}
            Ok(false) => {
                failed.store(true, Ordering::Release);
                emit(
                    &events,
                    &failed,
                    LspActorEvent::Failed(id, "language-server process exited".into()),
                );
                break;
            }
            Err(error) => {
                failed.store(true, Ordering::Release);
                emit(
                    &events,
                    &failed,
                    LspActorEvent::Failed(id, error.to_string()),
                );
                break;
            }
        }
    }
    initialized.store(false, Ordering::Release);
    let _ = manager.stop_server(server_id);
    emit(&events, &failed, LspActorEvent::Stopped(id));
}

fn emit(events: &SyncSender<LspActorEvent>, failed: &AtomicBool, event: LspActorEvent) {
    if events.try_send(event).is_err() {
        failed.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LspDocumentSync;
    use koren_documents::{DocumentId, DocumentInfo, DocumentSnapshot};
    use std::{
        cell::RefCell,
        collections::{BTreeMap, VecDeque},
        fs,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
        time::{Instant, SystemTime, UNIX_EPOCH},
    };

    struct Allow(AtomicUsize);
    impl ServerAuthorizer for Allow {
        fn authorize(&mut self, _: &ServerConfig) -> bool {
            self.0.fetch_add(1, Ordering::SeqCst);
            true
        }
    }

    fn root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("koren-lsp-actor-{label}-{nonce}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn config(workspace_root: &Path, id: &str) -> ServerConfig {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fake_lsp_server.py");
        ServerConfig {
            id: id.into(),
            command: if cfg!(windows) { "python" } else { "python3" }.into(),
            arguments: vec![script.to_string_lossy().into_owned()],
            workspace_root: workspace_root.to_path_buf(),
            environment: BTreeMap::new(),
        }
    }

    fn wait_for_event(
        actors: &LspServerActors,
        id: ServerId,
        matches: impl Fn(&LspActorEvent) -> bool,
    ) -> LspActorEvent {
        thread_local! {
            static PENDING: RefCell<VecDeque<LspActorEvent>> = const { RefCell::new(VecDeque::new()) };
        }
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            PENDING.with(|pending| pending.borrow_mut().extend(actors.drain_events()));
            let found = PENDING.with(|pending| {
                let mut pending = pending.borrow_mut();
                pending
                    .iter()
                    .position(&matches)
                    .and_then(|index| pending.remove(index))
            });
            if let Some(event) = found {
                return event;
            }
            assert!(Instant::now() < deadline, "language actor event timed out");
            thread::sleep(Duration::from_millis(5));
            let _ = id;
        }
    }

    fn notification(id: DocumentId) -> SyncNotification {
        let snapshot = DocumentSnapshot {
            info: DocumentInfo {
                id,
                path: Some("main.demo".into()),
                revision: 0,
                dirty: false,
            },
            text: "hello 🦀".into(),
        };
        LspDocumentSync::default()
            .open(id, "file:///main.demo", "demo", &snapshot)
            .unwrap()
    }

    #[test]
    fn initialization_is_asynchronous_and_independent_actors_progress_concurrently() {
        let workspace = root("init");
        let init_gate = workspace.join("release-init");
        let init_reached = workspace.join("init-reached");
        let mut blocked = config(&workspace, "blocked");
        blocked.environment.insert(
            "FAKE_LSP_INIT_GATE".into(),
            init_gate.to_string_lossy().into_owned(),
        );
        blocked.environment.insert(
            "FAKE_LSP_INIT_REACHED".into(),
            init_reached.to_string_lossy().into_owned(),
        );
        let mut actors = LspServerActors::new();
        let start = Instant::now();
        let blocked_id = actors
            .start_server(blocked, &mut Allow(AtomicUsize::new(0)))
            .unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "startup dispatch waited for initialize"
        );
        let blocked_handle = actors.handle(blocked_id).unwrap();
        wait_for_file(&init_reached);
        assert!(!blocked_handle.is_initialized());

        let fast_id = actors
            .start_server(config(&workspace, "fast"), &mut Allow(AtomicUsize::new(0)))
            .unwrap();
        wait_for_event(&actors, fast_id, |event| {
            *event == LspActorEvent::Initialized(fast_id)
        });
        assert!(actors.handle(fast_id).unwrap().is_initialized());
        assert!(!blocked_handle.is_initialized());

        fs::write(&init_gate, "continue").unwrap();
        wait_for_event(&actors, blocked_id, |event| {
            *event == LspActorEvent::Initialized(blocked_id)
        });
        actors.shutdown();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn timed_out_request_is_cancelled_and_does_not_poison_the_next_response() {
        let workspace = root("hover");
        let gate = workspace.join("release-hover");
        let reached = workspace.join("hover-reached");
        let cancelled = workspace.join("cancel-reached");
        let mut configuration = config(&workspace, "slow-hover");
        configuration.environment.insert(
            "FAKE_LSP_HOVER_GATE".into(),
            gate.to_string_lossy().into_owned(),
        );
        configuration.environment.insert(
            "FAKE_LSP_HOVER_REACHED".into(),
            reached.to_string_lossy().into_owned(),
        );
        configuration.environment.insert(
            "FAKE_LSP_CANCEL_REACHED".into(),
            cancelled.to_string_lossy().into_owned(),
        );
        let mut actors = LspServerActors::new();
        let id = actors
            .start_server(configuration, &mut Allow(AtomicUsize::new(0)))
            .unwrap();
        wait_for_event(&actors, id, |event| {
            *event == LspActorEvent::Initialized(id)
        });
        let handle = actors.handle(id).unwrap();
        handle.notify(notification(DocumentId(1))).unwrap();
        let request = LspRequest::Hover {
            uri: "file:///main.demo".into(),
            version: 1,
            position: TextPosition {
                line: 0,
                character: 6,
            },
            timeout: Duration::from_millis(80),
        };
        let ticket = handle.request(request.clone()).unwrap();
        wait_for_file(&reached);
        assert!(matches!(
            ticket.recv_timeout(Duration::from_millis(300)),
            Err(LspError::RequestTimeout(_))
        ));
        fs::write(&gate, "continue").unwrap();
        wait_for_file(&cancelled);
        let result = handle
            .request(request)
            .unwrap()
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert!(
            matches!(result, LspResponse::Hover(value) if value.value.as_ref().is_some_and(|hover| hover.contents == "hover info"))
        );
        actors.shutdown();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn initialization_timeout_stops_actor_and_terminates_child() {
        let workspace = root("init-timeout");
        let gate = workspace.join("never-release-init");
        let reached = workspace.join("init-reached");
        let mut configuration = config(&workspace, "never-initializes");
        configuration.environment.insert(
            "FAKE_LSP_INIT_GATE".into(),
            gate.to_string_lossy().into_owned(),
        );
        configuration.environment.insert(
            "FAKE_LSP_INIT_REACHED".into(),
            reached.to_string_lossy().into_owned(),
        );
        let mut actors = LspServerActors::new();
        let id = actors
            .start_server_with_timeout(
                configuration,
                &mut Allow(AtomicUsize::new(0)),
                Duration::from_millis(100),
            )
            .unwrap();
        let handle = actors.handle(id).unwrap();
        wait_for_file(&reached);
        wait_for_event(
            &actors,
            id,
            |event| matches!(event, LspActorEvent::Failed(event_id, _) if *event_id == id),
        );
        wait_for_event(&actors, id, |event| *event == LspActorEvent::Stopped(id));
        assert!(!handle.is_initialized());
        assert!(!gate.exists());
        actors.shutdown();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn delayed_completion_is_correlated_and_crashed_server_is_isolated() {
        let workspace = root("completion-crash");
        let completion_gate = workspace.join("release-completion");
        let completion_reached = workspace.join("completion-reached");
        let mut slow = config(&workspace, "slow-completion");
        slow.environment.insert(
            "FAKE_LSP_COMPLETION_GATE".into(),
            completion_gate.to_string_lossy().into_owned(),
        );
        slow.environment.insert(
            "FAKE_LSP_COMPLETION_REACHED".into(),
            completion_reached.to_string_lossy().into_owned(),
        );
        let mut crashing = config(&workspace, "crash-on-hover");
        crashing
            .environment
            .insert("FAKE_LSP_CRASH_ON_HOVER".into(), "1".into());
        let mut actors = LspServerActors::new();
        let slow_id = actors
            .start_server(slow, &mut Allow(AtomicUsize::new(0)))
            .unwrap();
        let crash_id = actors
            .start_server(crashing, &mut Allow(AtomicUsize::new(0)))
            .unwrap();
        wait_for_event(&actors, slow_id, |event| {
            *event == LspActorEvent::Initialized(slow_id)
        });
        wait_for_event(&actors, crash_id, |event| {
            *event == LspActorEvent::Initialized(crash_id)
        });
        let slow_handle = actors.handle(slow_id).unwrap();
        let crash_handle = actors.handle(crash_id).unwrap();
        slow_handle.notify(notification(DocumentId(1))).unwrap();
        crash_handle.notify(notification(DocumentId(2))).unwrap();
        let completion = slow_handle
            .request(LspRequest::Completion {
                uri: "file:///main.demo".into(),
                version: 1,
                position: TextPosition {
                    line: 0,
                    character: 6,
                },
                timeout: Duration::from_secs(2),
            })
            .unwrap();
        wait_for_file(&completion_reached);
        let hover = crash_handle
            .request(LspRequest::Hover {
                uri: "file:///main.demo".into(),
                version: 1,
                position: TextPosition {
                    line: 0,
                    character: 6,
                },
                timeout: Duration::from_secs(2),
            })
            .unwrap();
        assert!(hover.recv_timeout(Duration::from_secs(2)).is_err());
        wait_for_event(
            &actors,
            crash_id,
            |event| matches!(event, LspActorEvent::Failed(event_id, _) if *event_id == crash_id),
        );
        wait_for_event(&actors, crash_id, |event| {
            *event == LspActorEvent::Stopped(crash_id)
        });
        fs::write(&completion_gate, "continue").unwrap();
        let result = completion.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            matches!(result, LspResponse::Completion(value) if value.value.iter().any(|item| item.label == "complete"))
        );
        assert!(slow_handle.is_initialized());
        let still_works = slow_handle
            .request(LspRequest::Hover {
                uri: "file:///main.demo".into(),
                version: 1,
                position: TextPosition {
                    line: 0,
                    character: 6,
                },
                timeout: Duration::from_secs(1),
            })
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert!(matches!(still_works, LspResponse::Hover(_)));
        actors.shutdown();
        fs::remove_dir_all(workspace).unwrap();
    }

    fn wait_for_file(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(4);
        while !path.exists() {
            assert!(
                Instant::now() < deadline,
                "fake server did not reach its gate"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}
