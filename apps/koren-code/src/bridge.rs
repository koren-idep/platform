use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use koren_cargo::CargoProvider;
use koren_commands::{CommandError, CommandRegistry};
use koren_core::{EventQueue, Platform, PlatformEvent};
use koren_debug::{
    BreakpointSpec, DebugAdapterDefinition, DebugAuthorizationDetails, DebugAuthorizer,
    DebugConfiguration, DebugEvent, DebugManager, DebugRequestType, VariableReference,
};
use koren_documents::{DocumentId, DocumentManager};
use koren_extensions::{
    CommandContext, CommandEnablement, DebugRequestContribution, ExtensionContributions,
    ExtensionManager, ExtensionState, MenuLocation, SettingKind, SettingValue, ThemeBase,
    UiDocument, UiElement,
};
use koren_git::GitService;
use koren_inspections::{
    CancellationToken, Finding, FindingOrigin, InspectionProfile, InspectionProfiles,
    InspectionProvider, InspectionService, MAX_SARIF_BYTES, RuleProfile, Severity, export_sarif,
    import_sarif,
};
use koren_lsp::{
    DiagnosticSet, FormattingOptions, LspActorEvent, LspDocumentSync, LspRequest, LspResponse,
    LspServerActors, LspServerHandle, ServerAuthorizer, ServerConfig, ServerEvent, ServerId,
    path_to_uri,
};
use koren_lua::{LuaDocumentContext, LuaExtensionRuntime, LuaHostContext, LuaLimits};
use koren_projects::{DiscoveryLimits, ProjectId, ProjectInfo, ProjectManager};
use koren_protocol::{
    CargoPackageSummary, CargoProjectSummary, CargoTargetSummary, CommandCatalogSummary,
    DebugApprovalSummary, DebugConfigurationSummary, DirectoryEntry, DirectoryEntryKind, ErrorCode,
    ExecutionApprovalSummary, ExtensionCatalog, ExtensionCommandSummary,
    ExtensionDiagnosticDetailSummary, ExtensionMenuSummary, ExtensionProjectWizardSummary,
    ExtensionSettingKind, ExtensionSettingSummary, ExtensionSettingValue,
    ExtensionSummary as ProtocolExtensionSummary, ExtensionThemeSummary,
    ExtensionToolWindowSummary, ExtensionUiDocument, ExtensionUiElement, GitChangedFile,
    GitRepositorySummary, GitStatusSummary, InspectionCodeAction, InspectionConfigurationSummary,
    InspectionFinding, InspectionProposedTextEdit, InspectionRelatedLocation,
    InspectionRuleSummary, LanguageServerApprovalSummary, LanguageServerSummary,
    ProjectInspectionFindingPage, ProjectInspectionSummary, ProjectLifecycle, ProjectSummary,
    ProtocolError, Request, RequestMethod, Response, ResponseValue, SearchMatch,
    SearchProgress as ProtocolSearchProgress, SearchSummary as ProtocolSearchSummary,
    TaskLifecycle, TaskSummary, TerminalApprovalSummary, TerminalLifecycle, TerminalSessionSummary,
    TextEdit, validate_version,
};
use koren_search::{SearchEvent, SearchJob, SearchOptions, SearchService};
use koren_tasks::{
    ExecutionRequestId, TaskApprovalManager, TaskAuthorizationDecision, TaskCaller, TaskDefinition,
    TaskExecutionPolicy, TaskExecutionRequest, TaskExecutionRequestOutcome, TaskId, TaskManager,
    TaskOptions, TaskRunId, TaskSnapshot, TaskState,
};
use koren_terminal::{
    MAX_TERMINAL_INPUT_BYTES, TerminalApprovalDetails, TerminalEvent, TerminalId, TerminalManager,
    TerminalSessionInfo,
};
use koren_workspace::{EntryKind, WorkspaceManager};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const LANGUAGE_FEATURE_TIMEOUT: Duration = Duration::from_millis(150);
const LANGUAGE_FEATURE_WORKERS: usize = 4;
const LANGUAGE_FEATURE_QUEUE_CAPACITY: usize = 64;
const MAX_EXTENSION_PANEL_STATE_BYTES: usize = 256 * 1024;
type FeatureWork = Box<dyn FnOnce() + Send + 'static>;

struct LanguageFeaturePool {
    sender: SyncSender<FeatureWork>,
}

impl LanguageFeaturePool {
    fn new() -> Self {
        let (sender, receiver) = mpsc::sync_channel::<FeatureWork>(LANGUAGE_FEATURE_QUEUE_CAPACITY);
        let receiver = Arc::new(Mutex::new(receiver));
        for index in 0..LANGUAGE_FEATURE_WORKERS {
            let receiver = receiver.clone();
            let _ = thread::Builder::new()
                .name(format!("koren-language-request-{index}"))
                .spawn(move || {
                    loop {
                        let work = receiver.lock().ok().and_then(|rx| rx.recv().ok());
                        match work {
                            Some(work) => work(),
                            None => break,
                        }
                    }
                })
                .expect("failed to start language feature worker");
        }
        Self { sender }
    }

    fn submit(&self, work: FeatureWork) -> Result<(), FeatureWork> {
        self.sender.try_send(work).map_err(|error| match error {
            TrySendError::Full(work) | TrySendError::Disconnected(work) => work,
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct DesktopEvent {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_process_group_active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_event: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<DiagnosticSet>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

pub(crate) enum BackendCommand {
    Request(Box<Request>, Sender<Response>),
    Subscribe(SyncSender<Vec<DesktopEvent>>),
    Shutdown,
}

struct EventSubscriber {
    sender: SyncSender<Vec<DesktopEvent>>,
    pending: Option<Vec<DesktopEvent>>,
}

pub(crate) type BackendSender = Sender<BackendCommand>;

pub(crate) fn spawn_backend() -> BackendSender {
    let (sender, receiver) = mpsc::channel();
    let _worker = thread::Builder::new()
        .name("koren-code-platform".into())
        .spawn(move || backend_loop(receiver))
        .expect("failed to start Koren platform worker");
    sender
}

#[cfg(test)]
fn spawn_backend_with_state(state: DesktopBackend) -> BackendSender {
    let (sender, receiver) = mpsc::channel();
    let _worker = thread::Builder::new()
        .name("koren-code-platform-test".into())
        .spawn(move || backend_loop_with_state(receiver, state))
        .expect("failed to start test platform worker");
    sender
}

pub(crate) fn send_request(sender: &BackendSender, request: Request) -> Result<Response, String> {
    let (reply_tx, reply_rx) = mpsc::channel();
    sender
        .send(BackendCommand::Request(Box::new(request), reply_tx))
        .map_err(|_| "platform worker stopped".to_owned())?;
    reply_rx
        .recv_timeout(Duration::from_secs(60))
        .map_err(|error| format!("platform request did not complete: {error}"))
}

pub(crate) fn subscribe(
    sender: &BackendSender,
    events: SyncSender<Vec<DesktopEvent>>,
) -> Result<(), String> {
    sender
        .send(BackendCommand::Subscribe(events))
        .map_err(|_| "platform worker stopped".to_owned())
}

pub(crate) fn shutdown(sender: &BackendSender) {
    let _ = sender.send(BackendCommand::Shutdown);
}

fn backend_loop(receiver: Receiver<BackendCommand>) {
    let state = match DesktopBackend::new() {
        Ok(state) => state,
        Err(error) => {
            eprintln!("failed to initialize Koren Code Editor platform: {error}");
            return;
        }
    };
    backend_loop_with_state(receiver, state);
}

fn backend_loop_with_state(receiver: Receiver<BackendCommand>, mut state: DesktopBackend) {
    let language_features = LanguageFeaturePool::new();
    let mut subscribers: Vec<EventSubscriber> = Vec::new();
    let mut ticks_without_events = 0u16;
    loop {
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(BackendCommand::Request(request, reply)) => {
                if !state.defer_document_inspection(&request, reply.clone())
                    && !state.defer_language_feature(&request, reply.clone(), &language_features)
                {
                    let _ = reply.send(state.dispatch(*request));
                }
            }
            Ok(BackendCommand::Subscribe(sender)) => {
                if subscribers.len() < 8 {
                    subscribers.push(EventSubscriber {
                        sender,
                        pending: None,
                    });
                } else {
                    let _ = sender.try_send(Vec::new());
                }
            }
            Ok(BackendCommand::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        let was_blocked = subscribers
            .iter()
            .any(|subscriber| subscriber.pending.is_some());
        subscribers.retain_mut(|subscriber| {
            if let Some(pending) = subscriber.pending.as_ref() {
                match subscriber.sender.try_send(pending.clone()) {
                    Ok(()) => subscriber.pending = None,
                    Err(TrySendError::Full(_)) => return true,
                    Err(TrySendError::Disconnected(_)) => return false,
                }
            }
            true
        });
        if was_blocked {
            continue;
        }
        let events = state.drain_events_for_subscribers(!subscribers.is_empty());
        if events.is_empty() {
            ticks_without_events = ticks_without_events.saturating_add(1);
            if ticks_without_events < 400 {
                continue;
            }
            ticks_without_events = 0;
        } else {
            ticks_without_events = 0;
        }
        if !subscribers.is_empty() {
            subscribers.retain_mut(
                |subscriber| match subscriber.sender.try_send(events.clone()) {
                    Ok(()) => true,
                    Err(TrySendError::Full(events)) => {
                        subscriber.pending = Some(events);
                        true
                    }
                    Err(TrySendError::Disconnected(_)) => false,
                },
            );
        }
    }
    state.shutdown();
}

struct AlwaysRequireApproval;
impl TaskExecutionPolicy for AlwaysRequireApproval {
    fn decide(&self, _request: &koren_tasks::TaskExecutionSnapshot) -> TaskAuthorizationDecision {
        TaskAuthorizationDecision::RequireApproval
    }
}

struct DesktopBackend {
    platform: Platform,
    commands: CommandRegistry,
    workspace: WorkspaceManager,
    documents: DocumentManager,
    projects: ProjectManager,
    cargo: CargoProvider,
    tasks: TaskManager,
    approvals: TaskApprovalManager,
    terminals: TerminalManager,
    terminal_approvals: BTreeMap<u64, PendingTerminalApproval>,
    next_terminal_approval_id: u64,
    debugger: DebugManager,
    debug_approvals: BTreeMap<u64, PendingDebugApproval>,
    next_debug_approval_id: u64,
    debug_extension_profiles: BTreeSet<String>,
    known_debug_extension_profiles: BTreeSet<String>,
    known_debug_extension_adapters: BTreeSet<String>,
    extensions: ExtensionManager,
    inspections: InspectionService,
    inspection_profiles: InspectionProfiles,
    settings_path: Option<PathBuf>,
    extension_root: Option<PathBuf>,
    extensions_discovered: bool,
    extension_setting_values: BTreeMap<String, ExtensionSettingValue>,
    updated_panel_text: BTreeMap<String, String>,
    lua_runtimes: BTreeMap<String, LuaExtensionRuntime>,
    active_document: Option<DocumentId>,
    language_servers: LspServerActors,
    active_language_servers: BTreeMap<u64, ServerId>,
    initialized_language_servers: BTreeSet<u64>,
    language_server_owners: BTreeMap<u64, (String, String)>,
    language_document_sync: BTreeMap<u64, LspDocumentSync>,
    open_documents: BTreeSet<DocumentId>,
    document_cancellations: BTreeMap<DocumentId, Arc<AtomicBool>>,
    inspection_cancellations: Arc<Mutex<BTreeMap<(u64, u64), CancellationToken>>>,
    inspection_results: InspectionResultStore,
    project_inspection_jobs: Arc<Mutex<BTreeMap<u64, ProjectInspectionState>>>,
    next_project_inspection_id: u64,
    pending_language_servers: BTreeMap<u64, PendingLanguageServer>,
    next_language_server_approval: u64,
    search_jobs: BTreeMap<u64, SearchJob>,
    next_search_job_id: u64,
    search_service: Option<SearchService>,
    git_events: EventQueue,
}

struct PendingDebugApproval {
    configuration_id: String,
    details: DebugAuthorizationDetails,
    expires_at: Instant,
}

struct DebugInspectionRequest {
    session_id: u64,
    operation: String,
    thread_id: Option<i64>,
    frame_id: Option<i64>,
    reference: Option<VariableReference>,
    start: usize,
    count: usize,
}

struct ApprovedDebugRequest;
impl DebugAuthorizer for ApprovedDebugRequest {
    fn authorize_adapter(&mut self, _adapter: &DebugAdapterDefinition) -> bool {
        true
    }

    fn authorize_target(&mut self, _configuration: &DebugConfiguration) -> bool {
        true
    }
}

struct PendingLanguageServer {
    extension_id: String,
    configuration_id: String,
    configuration: ServerConfig,
}

struct PendingTerminalApproval {
    session_id: TerminalId,
    workspace: Option<PathBuf>,
    expires_at: Instant,
}

struct ProjectInspectionState {
    state: String,
    completed_documents: usize,
    total_documents: usize,
    current_path: Option<String>,
    findings: Vec<Finding>,
    error: Option<String>,
    cancellation: CancellationToken,
}

type InspectionResultKey = (u64, u64);
type InspectionResultStore = Arc<Mutex<BTreeMap<InspectionResultKey, Vec<Finding>>>>;

struct DenyLanguageServer;
impl ServerAuthorizer for DenyLanguageServer {
    fn authorize(&mut self, _configuration: &ServerConfig) -> bool {
        false
    }
}

struct ApproveLanguageServer;
impl ServerAuthorizer for ApproveLanguageServer {
    fn authorize(&mut self, _configuration: &ServerConfig) -> bool {
        true
    }
}

impl DesktopBackend {
    fn new() -> Result<Self, String> {
        let mut platform = Platform::new();
        platform.initialize().map_err(|error| error.to_string())?;
        let mut commands = CommandRegistry::new();
        commands
            .register(
                "core.ping",
                "Check that the platform is running",
                command_ping,
            )
            .map_err(|error| error.to_string())?;
        commands
            .register(
                "editor.new_file",
                "Create a file in the active workspace",
                command_new_file,
            )
            .map_err(|error| error.to_string())?;
        commands
            .register(
                "editor.search",
                "Search text across the active workspace",
                command_search,
            )
            .map_err(|error| error.to_string())?;
        commands
            .register(
                "editor.source_control",
                "Open local Git source control",
                command_source_control,
            )
            .map_err(|error| error.to_string())?;
        let mut projects = ProjectManager::new();
        let cargo = CargoProvider::new();
        cargo
            .register(&mut projects)
            .map_err(|error| error.to_string())?;
        let settings_path = extension_settings_path();
        let persisted_settings = load_desktop_settings(settings_path.as_deref());
        let mut inspection_profiles = persisted_settings.inspection_profiles;
        if !inspection_profiles.profiles.contains_key("Default") {
            inspection_profiles.insert(InspectionProfile::new("Default"));
        }
        if inspection_profiles.selected.is_none() {
            inspection_profiles
                .select(None, "Default")
                .map_err(|error| error.to_string())?;
        }
        Ok(Self {
            platform,
            commands,
            workspace: WorkspaceManager::new(),
            documents: DocumentManager::new(),
            projects,
            cargo,
            tasks: TaskManager::new(TaskOptions::default()).map_err(|error| error.to_string())?,
            approvals: TaskApprovalManager::new(),
            terminals: TerminalManager::default(),
            terminal_approvals: BTreeMap::new(),
            next_terminal_approval_id: 0,
            debugger: DebugManager::new(),
            debug_approvals: BTreeMap::new(),
            next_debug_approval_id: 0,
            debug_extension_profiles: BTreeSet::new(),
            known_debug_extension_profiles: BTreeSet::new(),
            known_debug_extension_adapters: BTreeSet::new(),
            extensions: ExtensionManager::new(),
            inspections: InspectionService::default(),
            inspection_profiles,
            settings_path,
            extension_root: extension_install_root(),
            extensions_discovered: false,
            extension_setting_values: persisted_settings.extension_settings,
            updated_panel_text: BTreeMap::new(),
            lua_runtimes: BTreeMap::new(),
            active_document: None,
            language_servers: LspServerActors::new(),
            active_language_servers: BTreeMap::new(),
            initialized_language_servers: BTreeSet::new(),
            language_server_owners: BTreeMap::new(),
            language_document_sync: BTreeMap::new(),
            open_documents: BTreeSet::new(),
            document_cancellations: BTreeMap::new(),
            inspection_cancellations: Arc::new(Mutex::new(BTreeMap::new())),
            inspection_results: Arc::new(Mutex::new(BTreeMap::new())),
            project_inspection_jobs: Arc::new(Mutex::new(BTreeMap::new())),
            next_project_inspection_id: 0,
            pending_language_servers: BTreeMap::new(),
            next_language_server_approval: 0,
            search_jobs: BTreeMap::new(),
            next_search_job_id: 0,
            search_service: None,
            git_events: EventQueue::new(),
        })
    }

    fn shutdown(&mut self) {
        self.debug_approvals.clear();
        self.debugger.shutdown();
        self.terminal_approvals.clear();
        self.terminals.shutdown();
        for job in self.search_jobs.values() {
            job.cancel();
        }
        self.search_jobs.clear();
        self.search_service = None;
        self.approvals.shutdown();
        self.language_servers.shutdown();
        self.tasks.shutdown();
        for (id, runtime) in std::mem::take(&mut self.lua_runtimes) {
            let _ = runtime.deactivate();
            let _ = self.extensions.deactivate(&id);
        }
        self.updated_panel_text.clear();
        if let Ok(root) = self.workspace.root() {
            let root = root.to_path_buf();
            self.projects.close_workspace(&root);
            let _ = self.workspace.close();
        }
        let _ = self.platform.shutdown();
    }

    fn dispatch(&mut self, request: Request) -> Response {
        let id = request.request_id;
        let result = validate_version(request.version).and_then(|()| self.method(request.method));
        Response::new(id, result)
    }

    fn defer_language_feature(
        &self,
        request: &Request,
        reply: Sender<Response>,
        pool: &LanguageFeaturePool,
    ) -> bool {
        let code_action_diagnostics = match &request.method {
            RequestMethod::LanguageCodeActions { diagnostics, .. } => {
                diagnostics.clone().unwrap_or_default()
            }
            _ => Vec::new(),
        };
        let (kind, document_id, expected_revision, position, include_declaration, range, new_name) =
            match &request.method {
                RequestMethod::LanguageHover {
                    document_id,
                    expected_revision,
                    position,
                } => (
                    FeatureKind::Hover,
                    *document_id,
                    *expected_revision,
                    *position,
                    false,
                    single_position_range(*position),
                    None,
                ),
                RequestMethod::LanguageCompletion {
                    document_id,
                    expected_revision,
                    position,
                } => (
                    FeatureKind::Completion,
                    *document_id,
                    *expected_revision,
                    *position,
                    false,
                    single_position_range(*position),
                    None,
                ),
                RequestMethod::LanguageDefinition {
                    document_id,
                    expected_revision,
                    position,
                } => (
                    FeatureKind::Definition,
                    *document_id,
                    *expected_revision,
                    *position,
                    false,
                    single_position_range(*position),
                    None,
                ),
                RequestMethod::LanguageReferences {
                    document_id,
                    expected_revision,
                    position,
                    include_declaration,
                } => (
                    FeatureKind::References,
                    *document_id,
                    *expected_revision,
                    *position,
                    *include_declaration,
                    single_position_range(*position),
                    None,
                ),
                RequestMethod::LanguageFormat {
                    document_id,
                    expected_revision,
                } => (
                    FeatureKind::Formatting,
                    *document_id,
                    *expected_revision,
                    koren_protocol::TextPosition {
                        line: 0,
                        character: 0,
                    },
                    false,
                    single_position_range(koren_protocol::TextPosition {
                        line: 0,
                        character: 0,
                    }),
                    None,
                ),
                RequestMethod::LanguageCodeActions {
                    document_id,
                    expected_revision,
                    range,
                    ..
                } => (
                    FeatureKind::CodeActions,
                    *document_id,
                    *expected_revision,
                    range.start,
                    false,
                    *range,
                    None,
                ),
                RequestMethod::LanguageRename {
                    document_id,
                    expected_revision,
                    position,
                    new_name,
                } => (
                    FeatureKind::Rename,
                    *document_id,
                    *expected_revision,
                    *position,
                    false,
                    single_position_range(*position),
                    Some(new_name.clone()),
                ),
                _ => return false,
            };
        let request_id = request.request_id;
        let result = validate_version(request.version)
            .and_then(|()| self.language_targets(document_id, expected_revision));
        let targets = match result {
            Ok(targets) => targets,
            Err(error) => {
                let _ = reply.send(Response::new(request_id, Err(error)));
                return true;
            }
        };
        let cancelled = self
            .document_cancellations
            .get(&DocumentId(document_id))
            .cloned()
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let worker_reply = reply.clone();
        let work = Box::new(move || {
            let result = run_language_feature(
                LanguageFeatureParams {
                    kind,
                    document_id,
                    expected_revision,
                    position,
                    include_declaration,
                    range,
                    new_name,
                    code_action_diagnostics,
                },
                targets,
                cancelled,
            );
            let _ = worker_reply.send(Response::new(request_id, result));
        });
        if pool.submit(work).is_err() {
            let _ = reply.send(Response::new(
                request_id,
                Err(ProtocolError {
                    code: ErrorCode::Internal,
                    message: "language request workers are busy".into(),
                }),
            ));
        }
        true
    }

    fn defer_document_inspection(&self, request: &Request, reply: Sender<Response>) -> bool {
        let RequestMethod::InspectDocument {
            document_id,
            expected_revision,
            language,
        } = &request.method
        else {
            return false;
        };
        let request_id = request.request_id;
        let result = validate_version(request.version).and_then(|()| {
            if language.trim().is_empty() || language.len() > 128 {
                return Err(invalid("inspection language identifier is invalid"));
            }
            let snapshot = self
                .documents
                .snapshot(DocumentId(*document_id))
                .map_err(document_error)?;
            if snapshot.info.revision != *expected_revision {
                return Err(ProtocolError {
                    code: ErrorCode::Conflict,
                    message: "inspection request refers to an obsolete document revision".into(),
                });
            }
            let workspace = self
                .workspace
                .root()
                .ok()
                .map(|root| root.to_string_lossy().into_owned());
            let profile = self
                .inspection_profiles
                .selected_for(workspace.as_deref())
                .cloned()
                .unwrap_or_else(|| InspectionProfile::new("Default"));
            let path = snapshot.info.path.clone();
            self.inspections
                .inspect_document(
                    koren_inspections::InspectionDocument {
                        path: path
                            .as_ref()
                            .map_or_else(String::new, |path| path.to_string_lossy().into_owned()),
                        document_id: Some(*document_id),
                        revision: Some(*expected_revision),
                        language: language.clone(),
                        text: Arc::from(snapshot.text),
                    },
                    profile,
                    workspace,
                )
                .map(|job| (path, job))
                .map_err(inspection_error)
        });
        let (path, job) = match result {
            Ok(value) => value,
            Err(error) => {
                let _ = reply.send(Response::new(request_id, Err(error)));
                return true;
            }
        };
        let document_id = *document_id;
        let revision = *expected_revision;
        let cancellation_map = self.inspection_cancellations.clone();
        let result_store = self.inspection_results.clone();
        let token = job.cancellation_token();
        let mut active = match cancellation_map.lock() {
            Ok(active) => active,
            Err(_) => {
                let _ = reply.send(Response::new(
                    request_id,
                    Err(ProtocolError {
                        code: ErrorCode::Internal,
                        message: "inspection cancellation registry is unavailable".into(),
                    }),
                ));
                return true;
            }
        };
        let stale_keys = active
            .keys()
            .filter(|(active_document, _)| *active_document == document_id)
            .copied()
            .collect::<Vec<_>>();
        for key in stale_keys {
            if let Some(previous) = active.remove(&key) {
                previous.cancel();
            }
        }
        active.insert((document_id, request_id), token.clone());
        drop(active);
        let worker_reply = reply.clone();
        let spawn = thread::Builder::new()
            .name("koren-inspection-response".into())
            .spawn(move || {
                let results = job
                    .recv_timeout(Duration::from_secs(30))
                    .map_err(inspection_error)
                    .and_then(|findings| {
                        if token.is_cancelled() {
                            return Err(ProtocolError {
                                code: ErrorCode::Conflict,
                                message: "inspection results became stale before publication"
                                    .into(),
                            });
                        }
                        if let Ok(mut stored) = result_store.lock() {
                            stored.insert((document_id, revision), findings.clone());
                        }
                        Ok(ResponseValue::InspectionFindings {
                            document_revision: revision,
                            findings: findings
                                .into_iter()
                                .map(|finding| {
                                    inspection_finding(finding, path.as_deref(), Some(revision))
                                })
                                .collect(),
                        })
                    });
                if let Ok(mut active) = cancellation_map.lock() {
                    active.remove(&(document_id, request_id));
                }
                let _ = worker_reply.send(Response::new(request_id, results));
            });
        if let Err(error) = spawn {
            self.cancel_document_inspections(document_id);
            let _ = reply.send(Response::new(
                request_id,
                Err(ProtocolError {
                    code: ErrorCode::Internal,
                    message: format!("could not start inspection response worker: {error}"),
                }),
            ));
        }
        true
    }

    fn cancel_document_inspections(&self, document_id: u64) {
        if let Ok(mut active) = self.inspection_cancellations.lock() {
            let keys = active
                .keys()
                .filter(|(active_document, _)| *active_document == document_id)
                .copied()
                .collect::<Vec<_>>();
            for key in keys {
                if let Some(token) = active.remove(&key) {
                    token.cancel();
                }
            }
        }
    }

    fn cancel_all_document_inspections(&self) {
        if let Ok(mut active) = self.inspection_cancellations.lock() {
            for token in active.values() {
                token.cancel();
            }
            active.clear();
        }
    }

    fn cancel_active_project_jobs(&self) {
        if let Ok(mut jobs) = self.project_inspection_jobs.lock() {
            for state in jobs.values_mut().filter(|state| state.state == "running") {
                state.state = "cancelling".into();
                state.cancellation.cancel();
            }
        }
    }

    fn clear_document_inspection_results(&self, document_id: u64) {
        if let Ok(mut stored) = self.inspection_results.lock() {
            stored.retain(|(stored_document, _), _| *stored_document != document_id);
        }
    }

    fn method(&mut self, method: RequestMethod) -> Result<ResponseValue, ProtocolError> {
        match method {
            RequestMethod::Ping => Ok(ResponseValue::Pong),
            RequestMethod::ListCommands => {
                let mut ids = self
                    .commands
                    .ids()
                    .into_iter()
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                ids.extend(
                    self.active_extension_commands()
                        .into_iter()
                        .map(|command| command.id),
                );
                ids.sort();
                Ok(ResponseValue::Commands(ids))
            }
            RequestMethod::ExecuteCommand { command_id } => self.execute_command(&command_id),
            RequestMethod::ListExtensions => self.list_extensions(),
            RequestMethod::ListInspectionRules => self.list_inspection_rules(),
            RequestMethod::SelectInspectionProfile {
                profile_name,
                workspace_specific,
            } => self.select_inspection_profile(&profile_name, workspace_specific),
            RequestMethod::CreateInspectionProfile {
                profile_name,
                workspace_specific,
            } => self.create_inspection_profile(&profile_name, workspace_specific),
            RequestMethod::SetInspectionRule {
                rule_id,
                enabled,
                severity,
                workspace_specific,
                reset,
            } => self.set_inspection_rule(
                &rule_id,
                enabled,
                severity.as_deref(),
                workspace_specific,
                reset,
            ),
            RequestMethod::ImportSarif { path } => self.import_sarif_file(&path),
            RequestMethod::ExportSarif => self.export_sarif_report(),
            RequestMethod::StartProjectInspection => self.start_project_inspection(),
            RequestMethod::ProjectInspectionStatus { job_id } => {
                self.project_inspection_status(job_id)
            }
            RequestMethod::CancelProjectInspection { job_id } => {
                self.cancel_project_inspection(job_id)
            }
            RequestMethod::ProjectInspectionFindings {
                job_id,
                offset,
                limit,
            } => self.project_inspection_findings(job_id, offset, limit),
            RequestMethod::SetExtensionSetting { setting_id, value } => {
                self.set_extension_setting(&setting_id, value)
            }
            RequestMethod::SetExtensionEnabled {
                extension_id,
                enabled,
            } => self.set_extension_enabled(&extension_id, enabled),
            RequestMethod::ActivateExtension { extension_id } => {
                self.activate_extension(&extension_id)
            }
            RequestMethod::StartLanguageServer {
                extension_id,
                configuration_id,
            } => self.request_language_server(&extension_id, &configuration_id),
            RequestMethod::ApproveLanguageServer { approval_id } => {
                self.approve_language_server(approval_id)
            }
            RequestMethod::DenyLanguageServer { approval_id } => {
                self.deny_language_server(approval_id)
            }
            RequestMethod::StopLanguageServer { server_id } => self.stop_language_server(server_id),
            RequestMethod::LanguageHover {
                document_id,
                expected_revision,
                position,
            } => self.language_hover(document_id, expected_revision, position),
            RequestMethod::LanguageCompletion {
                document_id,
                expected_revision,
                position,
            } => self.language_completion(document_id, expected_revision, position),
            RequestMethod::LanguageDefinition {
                document_id,
                expected_revision,
                position,
            } => self.language_definition(document_id, expected_revision, position),
            RequestMethod::LanguageReferences { .. }
            | RequestMethod::LanguageFormat { .. }
            | RequestMethod::LanguageCodeActions { .. }
            | RequestMethod::LanguageRename { .. }
            | RequestMethod::InspectDocument { .. } => Err(ProtocolError {
                code: ErrorCode::Internal,
                message: "language feature request was not dispatched to its worker".into(),
            }),
            RequestMethod::OpenWorkspace { path } => {
                self.workspace.open(path).map_err(workspace_error)?;
                Ok(ResponseValue::WorkspaceOpened {
                    root: root_string(&self.workspace)?,
                })
            }
            RequestMethod::CloseWorkspace => {
                let root = self
                    .workspace
                    .root()
                    .map_err(workspace_error)?
                    .to_path_buf();
                if self.documents.has_documents_for_workspace(&root) {
                    return Err(invalid(
                        "close open documents before closing this workspace",
                    ));
                }
                self.cancel_active_project_jobs();
                for job in self.search_jobs.values() {
                    job.cancel();
                }
                self.search_jobs.clear();
                self.search_service = None;
                self.approvals.invalidate_workspace(&root);
                self.debug_approvals.clear();
                self.language_servers
                    .close_workspace(&root)
                    .map_err(lsp_error)?;
                self.active_language_servers.clear();
                self.initialized_language_servers.clear();
                self.language_server_owners.clear();
                self.language_document_sync.clear();
                self.pending_language_servers
                    .retain(|_, pending| pending.configuration.workspace_root != root);
                self.projects.close_workspace(&root);
                self.tasks.shutdown();
                self.tasks = TaskManager::new(TaskOptions::default()).map_err(task_error)?;
                self.approvals = TaskApprovalManager::new();
                self.workspace.close().map_err(workspace_error)?;
                Ok(ResponseValue::WorkspaceClosed {
                    root: root.display().to_string(),
                })
            }
            RequestMethod::ListDirectory { path } => {
                let entries = self.workspace.list_dir(path).map_err(workspace_error)?;
                Ok(ResponseValue::DirectoryEntries(
                    entries.into_iter().map(protocol_directory_entry).collect(),
                ))
            }
            RequestMethod::CreateWorkspaceFile { path } => {
                let entry = self.workspace.create_file(&path).map_err(workspace_error)?;
                Ok(ResponseValue::WorkspaceEntry(protocol_directory_entry(
                    entry,
                )))
            }
            RequestMethod::CreateWorkspaceDirectory { path } => {
                let entry = self
                    .workspace
                    .create_directory(&path)
                    .map_err(workspace_error)?;
                Ok(ResponseValue::WorkspaceEntry(protocol_directory_entry(
                    entry,
                )))
            }
            RequestMethod::WorkspaceEntryMetadata { path } => {
                let entry = self
                    .workspace
                    .entry_metadata(&path)
                    .map_err(workspace_error)?;
                Ok(ResponseValue::WorkspaceEntry(protocol_directory_entry(
                    entry,
                )))
            }
            RequestMethod::DiscoverGitRepositories => {
                let root = self
                    .workspace
                    .root()
                    .map_err(workspace_error)?
                    .to_path_buf();
                let repositories = GitService::new(&root)
                    .and_then(|service| service.discover())
                    .map_err(git_error)?;
                Ok(ResponseValue::GitRepositories(
                    repositories
                        .into_iter()
                        .map(|repo| GitRepositorySummary {
                            root: repo.root.display().to_string(),
                            branch: repo.branch,
                            has_head: repo.has_head,
                        })
                        .collect(),
                ))
            }
            RequestMethod::GitStatus { path } => {
                let root = self
                    .workspace
                    .root()
                    .map_err(workspace_error)?
                    .to_path_buf();
                let service = GitService::new(&root).map_err(git_error)?;
                let repository = service.open(&path).map_err(git_error)?;
                let info = repository.info();
                let files = repository
                    .status()
                    .map_err(git_error)?
                    .into_iter()
                    .map(|file| GitChangedFile {
                        path: file.path,
                        staged: file.staged,
                        unstaged: file.unstaged,
                        untracked: file.untracked,
                        deleted: file.deleted,
                        conflicted: file.conflicted,
                    })
                    .collect();
                Ok(ResponseValue::GitStatus(GitStatusSummary {
                    root: info.root.display().to_string(),
                    branch: info.branch,
                    has_head: info.has_head,
                    files,
                }))
            }
            RequestMethod::GitDiff {
                repository,
                path,
                staged,
            } => {
                let root = self
                    .workspace
                    .root()
                    .map_err(workspace_error)?
                    .to_path_buf();
                let repository = GitService::new(&root)
                    .and_then(|service| service.open(&repository))
                    .map_err(git_error)?;
                let text = repository.diff(&path, staged).map_err(git_error)?;
                Ok(ResponseValue::GitDiff { path, staged, text })
            }
            RequestMethod::GitStage { repository, path } => {
                let root = self
                    .workspace
                    .root()
                    .map_err(workspace_error)?
                    .to_path_buf();
                let repository = GitService::new(&root)
                    .and_then(|service| service.open(&repository))
                    .map_err(git_error)?;
                repository.stage(&path).map_err(git_error)?;
                self.git_events.push(PlatformEvent::GitStatusChanged {
                    root: repository.info().root.clone(),
                });
                self.git_events.push(PlatformEvent::GitOperationCompleted {
                    root: repository.info().root,
                    operation: "staged".into(),
                    commit: None,
                });
                Ok(ResponseValue::GitOperation {
                    operation: "staged".into(),
                    commit: None,
                })
            }
            RequestMethod::GitUnstage { repository, path } => {
                let root = self
                    .workspace
                    .root()
                    .map_err(workspace_error)?
                    .to_path_buf();
                let repository = GitService::new(&root)
                    .and_then(|service| service.open(&repository))
                    .map_err(git_error)?;
                repository.unstage(&path).map_err(git_error)?;
                self.git_events.push(PlatformEvent::GitStatusChanged {
                    root: repository.info().root.clone(),
                });
                self.git_events.push(PlatformEvent::GitOperationCompleted {
                    root: repository.info().root,
                    operation: "unstaged".into(),
                    commit: None,
                });
                Ok(ResponseValue::GitOperation {
                    operation: "unstaged".into(),
                    commit: None,
                })
            }
            RequestMethod::GitCommit {
                repository,
                message,
            } => {
                let root = self
                    .workspace
                    .root()
                    .map_err(workspace_error)?
                    .to_path_buf();
                let repository = GitService::new(&root)
                    .and_then(|service| service.open(&repository))
                    .map_err(git_error)?;
                let commit = repository.commit(&message).map_err(git_error)?;
                self.git_events.push(PlatformEvent::GitStatusChanged {
                    root: repository.info().root.clone(),
                });
                self.git_events.push(PlatformEvent::GitOperationCompleted {
                    root: repository.info().root,
                    operation: "committed".into(),
                    commit: Some(commit.clone()),
                });
                Ok(ResponseValue::GitOperation {
                    operation: "committed".into(),
                    commit: Some(commit),
                })
            }
            RequestMethod::StartWorkspaceSearch {
                query,
                case_sensitive,
                regular_expression,
                file_filter,
            } => {
                if self.search_service.is_none() {
                    let root = self
                        .workspace
                        .root()
                        .map_err(workspace_error)?
                        .to_path_buf();
                    self.search_service = Some(SearchService::new(root).map_err(search_error)?);
                }
                let job = self
                    .search_service
                    .as_ref()
                    .expect("service initialized")
                    .search(
                        &query,
                        SearchOptions {
                            case_sensitive,
                            regular_expression,
                            file_filter,
                            ..SearchOptions::default()
                        },
                    )
                    .map_err(search_error)?;
                self.next_search_job_id = self.next_search_job_id.saturating_add(1);
                let job_id = self.next_search_job_id;
                self.search_jobs.insert(job_id, job);
                Ok(ResponseValue::SearchStarted { job_id })
            }
            RequestMethod::CancelWorkspaceSearch { job_id } => {
                let job = self
                    .search_jobs
                    .get(&job_id)
                    .ok_or_else(|| not_found(format!("search job {job_id} was not found")))?;
                job.cancel();
                Ok(ResponseValue::SearchCancelled { job_id })
            }
            RequestMethod::PollWorkspaceSearch { job_id } => self.poll_workspace_search(job_id),
            RequestMethod::RenameWorkspaceEntry {
                source,
                destination,
            } => {
                self.documents
                    .ensure_workspace_path_clean(&self.workspace, &source)
                    .map_err(document_error)?;
                self.documents
                    .ensure_workspace_path_relocation_available(
                        &self.workspace,
                        &source,
                        &destination,
                    )
                    .map_err(document_error)?;
                let entry = self
                    .workspace
                    .rename(&source, &destination)
                    .map_err(workspace_error)?;
                let moved_documents = self
                    .documents
                    .relocate_workspace_path(&self.workspace, &source, &destination)
                    .map_err(document_error)?;
                for info in moved_documents {
                    self.sync_document_close(info.id);
                    self.sync_document_open(info.id);
                }
                Ok(ResponseValue::WorkspaceEntryChanged {
                    operation: "renamed".into(),
                    path: entry.path.to_string_lossy().into_owned(),
                    previous_path: Some(source),
                })
            }
            RequestMethod::DeleteWorkspaceEntry {
                path,
                recursive,
                confirmed,
            } => {
                self.documents
                    .ensure_workspace_path_clean(&self.workspace, &path)
                    .map_err(document_error)?;
                self.workspace
                    .delete(&path, recursive, confirmed)
                    .map_err(workspace_error)?;
                let closed_documents = self
                    .documents
                    .close_workspace_path(&self.workspace, &path)
                    .map_err(document_error)?;
                for id in closed_documents {
                    self.cancel_document_inspections(id.0);
                    self.clear_document_inspection_results(id.0);
                    if let Some(cancelled) = self.document_cancellations.remove(&id) {
                        cancelled.store(true, Ordering::Release);
                    }
                    self.sync_document_close(id);
                    if self.active_document == Some(id) {
                        self.active_document = None;
                    }
                    self.open_documents.remove(&id);
                }
                Ok(ResponseValue::WorkspaceEntryChanged {
                    operation: "deleted".into(),
                    path,
                    previous_path: None,
                })
            }
            RequestMethod::OpenDocument { path } => {
                let id = self
                    .documents
                    .open_file(&self.workspace, path)
                    .map_err(document_error)?;
                self.open_documents.insert(id);
                self.document_cancellations
                    .entry(id)
                    .or_insert_with(|| Arc::new(AtomicBool::new(false)));
                self.sync_document_open(id);
                let info = self.documents.info(id).map_err(document_error)?;
                Ok(ResponseValue::DocumentOpened {
                    document_id: id.0,
                    path: info.path.map(|path| path.to_string_lossy().into_owned()),
                    revision: info.revision,
                    dirty: info.dirty,
                })
            }
            RequestMethod::ReadDocument { document_id } => {
                self.active_document = Some(DocumentId(document_id));
                let snapshot = self
                    .documents
                    .snapshot(DocumentId(document_id))
                    .map_err(document_error)?;
                Ok(ResponseValue::DocumentContents {
                    document_id,
                    contents: snapshot.text,
                    revision: snapshot.info.revision,
                    dirty: snapshot.info.dirty,
                })
            }
            RequestMethod::ReadDocumentConflict { document_id } => {
                let info = self
                    .documents
                    .info(DocumentId(document_id))
                    .map_err(document_error)?;
                let contents = self
                    .documents
                    .conflict_contents(DocumentId(document_id))
                    .map_err(document_error)?;
                Ok(ResponseValue::DocumentConflictContents {
                    document_id,
                    path: info.path.map(|path| path.to_string_lossy().into_owned()),
                    contents,
                })
            }
            RequestMethod::ApplyDocumentEdits {
                document_id,
                expected_revision,
                edits,
            } => {
                self.cancel_document_inspections(document_id);
                let info = self
                    .documents
                    .apply_edits(DocumentId(document_id), expected_revision, &edits)
                    .map_err(document_error)?;
                self.clear_document_inspection_results(document_id);
                self.sync_document_change(DocumentId(document_id));
                Ok(ResponseValue::DocumentChanged {
                    document_id,
                    revision: info.revision,
                    dirty: info.dirty,
                })
            }
            RequestMethod::SaveDocument {
                document_id,
                expected_revision,
            } => {
                let info = self
                    .documents
                    .save(DocumentId(document_id), expected_revision)
                    .map_err(document_error)?;
                Ok(ResponseValue::DocumentSaved {
                    document_id,
                    revision: info.revision,
                    dirty: info.dirty,
                })
            }
            RequestMethod::ResolveDocumentConflict {
                document_id,
                expected_revision,
                decision,
            } => {
                let info = self
                    .documents
                    .resolve_conflict(DocumentId(document_id), expected_revision, decision)
                    .map_err(document_error)?;
                self.clear_document_inspection_results(document_id);
                Ok(ResponseValue::DocumentSaved {
                    document_id,
                    revision: info.revision,
                    dirty: info.dirty,
                })
            }
            RequestMethod::CloseDocument {
                document_id,
                discard_unsaved_changes,
            } => {
                self.cancel_document_inspections(document_id);
                self.documents
                    .close(DocumentId(document_id), discard_unsaved_changes)
                    .map_err(document_error)?;
                self.clear_document_inspection_results(document_id);
                if let Some(cancelled) =
                    self.document_cancellations.remove(&DocumentId(document_id))
                {
                    cancelled.store(true, Ordering::Release);
                }
                self.sync_document_close(DocumentId(document_id));
                if self.active_document == Some(DocumentId(document_id)) {
                    self.active_document = None;
                }
                self.open_documents.remove(&DocumentId(document_id));
                Ok(ResponseValue::DocumentClosed { document_id })
            }
            RequestMethod::ListProjects => self
                .projects
                .list()
                .into_iter()
                .map(project_summary)
                .collect::<Result<Vec<_>, _>>()
                .map(ResponseValue::Projects),
            RequestMethod::DiscoverCargoProjects {
                max_directories,
                max_projects,
                max_depth,
            } => {
                let discovered = self
                    .cargo
                    .discover(
                        &self.workspace,
                        &mut self.projects,
                        DiscoveryLimits {
                            max_directories,
                            max_projects,
                            max_depth,
                        },
                    )
                    .map_err(|error| ProtocolError {
                        code: ErrorCode::InvalidRequest,
                        message: error.to_string(),
                    })?;
                for project in &discovered {
                    let info = self
                        .projects
                        .info(project.project_id)
                        .map_err(project_error)?;
                    for task in self.cargo.tasks_for_project(info) {
                        let already_registered = self.tasks.list().iter().any(|(_, existing)| {
                            existing.key == task.key && existing.project_id == task.project_id
                        });
                        if !already_registered {
                            self.tasks.register(task).map_err(task_error)?;
                        }
                    }
                }
                let summaries = discovered
                    .into_iter()
                    .map(|item| {
                        let info = self.projects.info(item.project_id).map_err(project_error)?;
                        cargo_summary(info, &item.metadata)
                    })
                    .collect::<Result<Vec<_>, ProtocolError>>()?;
                Ok(ResponseValue::CargoProjects(summaries))
            }
            RequestMethod::CargoProjectInfo { project_id } => {
                let info = self
                    .projects
                    .info(ProjectId::from_raw(project_id))
                    .map_err(project_error)?;
                let metadata =
                    self.cargo
                        .project_metadata(info)
                        .map_err(|error| ProtocolError {
                            code: ErrorCode::InvalidRequest,
                            message: error.to_string(),
                        })?;
                Ok(ResponseValue::CargoProject(cargo_summary(info, &metadata)?))
            }
            RequestMethod::ListTasks => Ok(ResponseValue::Tasks(
                self.tasks
                    .list()
                    .into_iter()
                    .map(|(id, task)| task_summary(id.get(), task))
                    .collect(),
            )),
            RequestMethod::RequestTaskExecution { task_id } => {
                self.ensure_task_scope(TaskId::from_raw(task_id))?;
                let root = self
                    .workspace
                    .root()
                    .map_err(workspace_error)?
                    .to_path_buf();
                match self
                    .approvals
                    .request_execution(
                        &mut self.tasks,
                        TaskId::from_raw(task_id),
                        TaskCaller::InteractiveUser,
                        root,
                        &AlwaysRequireApproval,
                        Duration::from_secs(120),
                    )
                    .map_err(task_error)?
                {
                    TaskExecutionRequestOutcome::Started(run) => {
                        Ok(ResponseValue::TaskStarted { run_id: run.get() })
                    }
                    TaskExecutionRequestOutcome::ApprovalRequired(request) => {
                        Ok(ResponseValue::ApprovalRequired(approval_summary(
                            &request,
                            &self.approvals,
                        )?))
                    }
                }
            }
            RequestMethod::ListPendingApprovals => {
                let requests = self
                    .approvals
                    .pending_requests()
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>();
                requests
                    .into_iter()
                    .map(|request| approval_summary(&request, &self.approvals))
                    .collect::<Result<Vec<_>, _>>()
                    .map(ResponseValue::PendingApprovals)
            }
            RequestMethod::ApproveTaskExecution { request_id } => {
                let id = ExecutionRequestId::from_raw(request_id);
                let scope = self
                    .approvals
                    .pending_requests()
                    .into_iter()
                    .find(|request| request.id == id)
                    .map(|request| {
                        (
                            request.snapshot.workspace_root.clone(),
                            request.snapshot.definition.project_id,
                        )
                    })
                    .ok_or_else(|| ProtocolError {
                        code: ErrorCode::NotFound,
                        message: "approval request was not found or expired".into(),
                    })?;
                if scope.0 != *self.workspace.root().map_err(workspace_error)? {
                    self.approvals.invalidate_workspace(&scope.0);
                    return Err(ProtocolError {
                        code: ErrorCode::NotOpen,
                        message: "approval workspace is no longer active".into(),
                    });
                }
                if let Some(project_id) = scope.1 {
                    if self.projects.info(ProjectId::from_raw(project_id)).is_err() {
                        self.approvals.invalidate_project(project_id);
                        return Err(ProtocolError {
                            code: ErrorCode::NotFound,
                            message: "approval project is no longer active".into(),
                        });
                    }
                }
                let run_id = self
                    .approvals
                    .approve(&mut self.tasks, id)
                    .map_err(task_error)?;
                Ok(ResponseValue::TaskApproved {
                    request_id,
                    run_id: run_id.get(),
                })
            }
            RequestMethod::DenyTaskExecution { request_id } => {
                self.approvals
                    .deny(ExecutionRequestId::from_raw(request_id))
                    .map_err(task_error)?;
                Ok(ResponseValue::TaskDenied { request_id })
            }
            RequestMethod::TaskStatus { run_id } | RequestMethod::TaskOutput { run_id } => self
                .tasks
                .output(TaskRunId::from_raw(run_id))
                .map(task_response)
                .map_err(task_error),
            RequestMethod::CancelTask { run_id } => self
                .tasks
                .cancel(TaskRunId::from_raw(run_id))
                .map(|()| ResponseValue::TaskCancelled { run_id })
                .map_err(task_error),
            RequestMethod::CleanupTask { run_id } => self
                .tasks
                .cleanup(TaskRunId::from_raw(run_id))
                .map(|()| ResponseValue::TaskCleaned { run_id })
                .map_err(task_error),
            RequestMethod::RequestTerminalStart => self.request_terminal_start(),
            RequestMethod::ListTerminalSessions => self
                .terminals
                .sessions()
                .map(|sessions| {
                    ResponseValue::TerminalSessions(sessions.iter().map(terminal_summary).collect())
                })
                .map_err(terminal_error),
            RequestMethod::ApproveTerminalStart { request_id } => {
                self.approve_terminal_start(request_id)
            }
            RequestMethod::DenyTerminalStart { request_id } => self.deny_terminal_start(request_id),
            RequestMethod::TerminalWrite {
                session_id,
                input_base64,
            } => {
                if input_base64.len() > MAX_TERMINAL_INPUT_BYTES.div_ceil(3) * 4 {
                    return Err(invalid("terminal input exceeds the 64 KiB limit"));
                }
                let bytes = BASE64.decode(input_base64).map_err(|error| {
                    invalid(format!("invalid terminal input encoding: {error}"))
                })?;
                self.terminals
                    .write(TerminalId::from_raw(session_id), &bytes)
                    .map_err(terminal_error)?;
                Ok(ResponseValue::TerminalAction { session_id })
            }
            RequestMethod::TerminalResize {
                session_id,
                rows,
                columns,
            } => {
                self.terminals
                    .resize(TerminalId::from_raw(session_id), rows, columns)
                    .map_err(terminal_error)?;
                Ok(ResponseValue::TerminalAction { session_id })
            }
            RequestMethod::CloseTerminal {
                session_id,
                terminate_active,
            } => {
                self.terminals
                    .close_session(TerminalId::from_raw(session_id), terminate_active)
                    .map_err(terminal_error)?;
                Ok(ResponseValue::TerminalAction { session_id })
            }
            RequestMethod::RegisterDebugConfiguration {
                adapter,
                configuration,
            } => {
                configuration.validate().map_err(debug_error)?;
                if adapter.id != configuration.adapter_id
                    || self
                        .debugger
                        .adapters()
                        .iter()
                        .any(|existing| existing.id == adapter.id)
                    || self
                        .debugger
                        .configurations()
                        .iter()
                        .any(|existing| existing.id == configuration.id)
                {
                    return Err(invalid(
                        "debug profile identifiers must be unique and matched",
                    ));
                }
                self.debugger
                    .register_adapter(adapter)
                    .map_err(debug_error)?;
                self.debugger
                    .register_configuration(configuration)
                    .map_err(debug_error)?;
                Ok(ResponseValue::DebugConfigurations(
                    self.debug_configuration_summaries(),
                ))
            }
            RequestMethod::ListDebugConfigurations => {
                self.sync_debug_extension_profiles()?;
                Ok(ResponseValue::DebugConfigurations(
                    self.debug_configuration_summaries(),
                ))
            }
            RequestMethod::RequestDebugStart { configuration_id } => {
                self.request_debug_start(configuration_id)
            }
            RequestMethod::ApproveDebugStart { approval_id } => {
                self.approve_debug_start(approval_id)
            }
            RequestMethod::DenyDebugStart { approval_id } => {
                self.debug_approvals
                    .remove(&approval_id)
                    .ok_or_else(|| ProtocolError {
                        code: ErrorCode::NotFound,
                        message: "debug approval was not found or expired".into(),
                    })?;
                Ok(ResponseValue::DebugDenied { approval_id })
            }
            RequestMethod::ListDebugSessions => {
                Ok(ResponseValue::DebugSessions(self.debugger.sessions()))
            }
            RequestMethod::DebugAction {
                session_id,
                action,
                thread_id,
            } => self.debug_action(session_id, &action, thread_id),
            RequestMethod::DebugInspect {
                session_id,
                operation,
                thread_id,
                frame_id,
                reference,
                start,
                count,
            } => self.debug_inspect(DebugInspectionRequest {
                session_id,
                operation,
                thread_id,
                frame_id,
                reference,
                start,
                count,
            }),
            RequestMethod::SetDebugBreakpoints {
                source,
                breakpoints,
            } => self.set_debug_breakpoints(&source, breakpoints),
            RequestMethod::StopDebugSession { session_id } => {
                self.debugger
                    .session(session_id)
                    .and_then(|session| session.stop())
                    .map_err(debug_error)?;
                Ok(ResponseValue::DebugRequestAccepted {
                    session_id,
                    request_id: 0,
                })
            }
            _ => Err(invalid("operation is not supported by Koren Code Editor")),
        }
    }

    fn debug_configuration_summaries(&self) -> Vec<DebugConfigurationSummary> {
        let adapters = self
            .debugger
            .adapters()
            .into_iter()
            .map(|adapter| (adapter.id.clone(), adapter))
            .collect::<BTreeMap<_, _>>();
        self.debugger
            .configurations()
            .into_iter()
            .filter(|configuration| {
                !self
                    .known_debug_extension_profiles
                    .contains(&configuration.id)
                    || self.debug_extension_profiles.contains(&configuration.id)
            })
            .filter_map(|configuration| {
                let adapter = adapters.get(&configuration.adapter_id)?;
                Some(DebugConfigurationSummary {
                    id: configuration.id,
                    name: configuration.name,
                    adapter_id: configuration.adapter_id,
                    adapter_name: adapter.name.clone(),
                    request: configuration.request,
                    program: configuration
                        .program
                        .map(|path| path.to_string_lossy().into_owned()),
                    process_id: configuration.process_id,
                })
            })
            .collect()
    }

    fn request_debug_start(
        &mut self,
        configuration_id: String,
    ) -> Result<ResponseValue, ProtocolError> {
        self.sync_debug_extension_profiles()?;
        if self
            .known_debug_extension_profiles
            .contains(&configuration_id)
            && !self.debug_extension_profiles.contains(&configuration_id)
        {
            return Err(ProtocolError {
                code: ErrorCode::NotFound,
                message: "the declaring extension is not active".into(),
            });
        }
        self.expire_debug_approvals();
        if self.debug_approvals.len() >= 16 {
            return Err(invalid("too many pending debug approvals"));
        }
        let details = self
            .debugger
            .authorization_details(&configuration_id)
            .map_err(debug_error)?;
        self.next_debug_approval_id = self
            .next_debug_approval_id
            .checked_add(1)
            .ok_or_else(|| invalid("debug approval identifiers exhausted"))?;
        let approval_id = self.next_debug_approval_id;
        let expires_at = Instant::now() + Duration::from_secs(120);
        self.debug_approvals.insert(
            approval_id,
            PendingDebugApproval {
                configuration_id,
                details: details.clone(),
                expires_at,
            },
        );
        Ok(ResponseValue::DebugApprovalRequired(
            debug_approval_summary(
                approval_id,
                &details.adapter,
                &details.configuration,
                expires_at,
            ),
        ))
    }

    fn approve_debug_start(&mut self, approval_id: u64) -> Result<ResponseValue, ProtocolError> {
        self.expire_debug_approvals();
        let pending = self
            .debug_approvals
            .remove(&approval_id)
            .ok_or_else(|| ProtocolError {
                code: ErrorCode::NotFound,
                message: "debug approval was not found or expired".into(),
            })?;
        self.sync_debug_extension_profiles()?;
        if self
            .known_debug_extension_profiles
            .contains(&pending.configuration_id)
            && !self
                .debug_extension_profiles
                .contains(&pending.configuration_id)
        {
            return Err(ProtocolError {
                code: ErrorCode::Conflict,
                message: "the extension that provided this debug profile is no longer active"
                    .into(),
            });
        }
        let current_details = self
            .debugger
            .authorization_details(&pending.configuration_id)
            .map_err(debug_error)?;
        if current_details != pending.details {
            return Err(ProtocolError {
                code: ErrorCode::Conflict,
                message: "the debug configuration changed after approval was requested".into(),
            });
        }
        let mut authorizer = ApprovedDebugRequest;
        let session = self
            .debugger
            .start_session(&pending.configuration_id, &mut authorizer)
            .map_err(debug_error)?;
        Ok(ResponseValue::DebugSessionStarted(session))
    }

    fn expire_debug_approvals(&mut self) {
        let now = Instant::now();
        self.debug_approvals
            .retain(|_, approval| approval.expires_at > now);
    }

    fn sync_debug_extension_profiles(&mut self) -> Result<(), ProtocolError> {
        self.ensure_extensions_discovered()?;
        for id in &self.known_debug_extension_profiles {
            self.debugger
                .unregister_configuration(id)
                .map_err(debug_error)?;
        }
        for id in &self.known_debug_extension_adapters {
            self.debugger.unregister_adapter(id).map_err(debug_error)?;
        }
        let workspace = self.workspace.root().ok().map(Path::to_path_buf);
        if workspace.is_none() {
            self.debug_extension_profiles.clear();
            return Ok(());
        }
        let mut active_profiles = BTreeSet::new();
        let mut contributions = Vec::<(String, ExtensionContributions)>::new();
        for extension in self.extensions.list() {
            if extension.state == ExtensionState::Active {
                if let Ok(items) = self.extensions.contributions(&extension.id) {
                    contributions.push((extension.id, items.clone()));
                }
            }
        }
        for (owner, items) in contributions {
            let adapter_ids = items
                .debug_adapters
                .iter()
                .map(|adapter| (adapter.id.clone(), qualify_debug_id(&owner, &adapter.id)))
                .collect::<BTreeMap<_, _>>();
            for adapter in &items.debug_adapters {
                let qualified_id = qualify_debug_id(&owner, &adapter.id);
                let working_directory =
                    resolve_workspace_template(&adapter.working_directory, workspace.as_deref())?;
                let adapter_is_registered = self
                    .debugger
                    .adapters()
                    .iter()
                    .any(|registered| registered.id == qualified_id);
                if adapter_is_registered
                    && !self.known_debug_extension_adapters.contains(&qualified_id)
                {
                    return Err(invalid(
                        "extension debug adapter identifier conflicts with an existing adapter",
                    ));
                }
                if !adapter_is_registered {
                    self.debugger
                        .register_adapter(DebugAdapterDefinition {
                            id: qualified_id.clone(),
                            name: adapter.name.clone(),
                            command: adapter.command.clone(),
                            arguments: adapter.arguments.clone(),
                            working_directory,
                            environment: adapter.environment.clone(),
                        })
                        .map_err(debug_error)?;
                    self.known_debug_extension_adapters
                        .insert(qualified_id.clone());
                }
            }
            for profile in &items.debug_configurations {
                let id = qualify_debug_id(&owner, &profile.id);
                active_profiles.insert(id.clone());
                self.known_debug_extension_profiles.insert(id.clone());
                let adapter_id = adapter_ids.get(&profile.adapter_id).ok_or_else(|| {
                    invalid("active debug profile references an unavailable adapter contribution")
                })?;
                let program = profile
                    .program
                    .as_deref()
                    .map(|path| resolve_workspace_template(path, workspace.as_deref()))
                    .transpose()?;
                let configuration = DebugConfiguration {
                    id,
                    name: profile.name.clone(),
                    adapter_id: adapter_id.clone(),
                    request: match profile.request {
                        DebugRequestContribution::Launch => DebugRequestType::Launch,
                        DebugRequestContribution::Attach => DebugRequestType::Attach,
                    },
                    program,
                    process_id: profile.process_id,
                    arguments: profile.arguments.clone(),
                    working_directory: resolve_workspace_template(
                        &profile.working_directory,
                        workspace.as_deref(),
                    )?,
                    environment: profile.environment.clone(),
                    adapter_settings: profile.adapter_settings.clone(),
                    stop_on_entry: profile.stop_on_entry,
                };
                self.debugger
                    .register_configuration(configuration)
                    .map_err(debug_error)?;
                self.known_debug_extension_profiles
                    .insert(qualify_debug_id(&owner, &profile.id));
            }
        }
        self.debug_extension_profiles = active_profiles;
        Ok(())
    }

    fn debug_action(
        &self,
        session_id: u64,
        action: &str,
        thread_id: Option<i64>,
    ) -> Result<ResponseValue, ProtocolError> {
        let session = self.debugger.session(session_id).map_err(debug_error)?;
        let request_id = match action {
            "continue" => session.continue_execution(),
            "pause" => session.pause(required_thread_id(thread_id)?),
            "step_over" => session.step_over(required_thread_id(thread_id)?),
            "step_into" => session.step_into(required_thread_id(thread_id)?),
            "step_out" => session.step_out(required_thread_id(thread_id)?),
            _ => return Err(invalid("unsupported debug action")),
        }
        .map_err(debug_error)?;
        Ok(ResponseValue::DebugRequestAccepted {
            session_id,
            request_id,
        })
    }

    fn debug_inspect(
        &self,
        request: DebugInspectionRequest,
    ) -> Result<ResponseValue, ProtocolError> {
        let session = self
            .debugger
            .session(request.session_id)
            .map_err(debug_error)?;
        let request_id = match request.operation.as_str() {
            "threads" => session.threads(),
            "stack_trace" => session.stack_trace(
                required_thread_id(request.thread_id)?,
                request.start,
                request.count,
            ),
            "scopes" => session.scopes(
                request
                    .frame_id
                    .ok_or_else(|| invalid("frame_id is required"))?,
            ),
            "variables" => session.variables(
                request
                    .reference
                    .ok_or_else(|| invalid("variable reference is required"))?,
                None,
                request.start,
                request.count,
            ),
            _ => return Err(invalid("unsupported debug inspection request")),
        }
        .map_err(debug_error)?;
        Ok(ResponseValue::DebugRequestAccepted {
            session_id: request.session_id,
            request_id,
        })
    }

    fn set_debug_breakpoints(
        &mut self,
        source: &str,
        breakpoints: Vec<BreakpointSpec>,
    ) -> Result<ResponseValue, ProtocolError> {
        let source = PathBuf::from(source);
        if !source.is_absolute() {
            return Err(invalid("debug breakpoint source must be absolute"));
        }
        let root = self.workspace.root().map_err(workspace_error)?;
        if !source.starts_with(root) {
            return Err(invalid(
                "debug breakpoint source is outside the active workspace",
            ));
        }
        self.debugger
            .set_source_breakpoints(&source, breakpoints)
            .map(ResponseValue::DebugBreakpoints)
            .map_err(debug_error)
    }

    fn request_terminal_start(&mut self) -> Result<ResponseValue, ProtocolError> {
        let now = Instant::now();
        let expired = self
            .terminal_approvals
            .iter()
            .filter_map(|(id, request)| (request.expires_at <= now).then_some(*id))
            .collect::<Vec<_>>();
        for id in expired {
            if let Some(request) = self.terminal_approvals.remove(&id) {
                let _ = self.terminals.close_session(request.session_id, false);
            }
        }
        if self.terminal_approvals.len() >= 16 {
            return Err(invalid("too many pending terminal approvals"));
        }
        let workspace = self.workspace.root().ok().map(Path::to_path_buf);
        let created = self
            .terminals
            .create_session("default", workspace.as_deref(), None, 24, 80)
            .map_err(terminal_error)?;
        let details = self
            .terminals
            .approval_details(created.id)
            .map_err(terminal_error)?;
        self.next_terminal_approval_id = self
            .next_terminal_approval_id
            .checked_add(1)
            .ok_or_else(|| invalid("terminal approval identifiers exhausted"))?;
        let request_id = self.next_terminal_approval_id;
        let summary = terminal_approval_summary(request_id, &details, workspace.as_deref());
        self.terminal_approvals.insert(
            request_id,
            PendingTerminalApproval {
                session_id: created.id,
                workspace,
                expires_at: now + Duration::from_secs(120),
            },
        );
        Ok(ResponseValue::TerminalApprovalRequired(summary))
    }

    fn approve_terminal_start(&mut self, request_id: u64) -> Result<ResponseValue, ProtocolError> {
        let pending = self.terminal_approvals.remove(&request_id).ok_or_else(|| {
            not_found("terminal approval request was not found or expired".into())
        })?;
        if pending.expires_at <= Instant::now()
            || pending.workspace.as_deref() != self.workspace.root().ok()
        {
            let _ = self.terminals.close_session(pending.session_id, false);
            return Err(ProtocolError {
                code: ErrorCode::NotOpen,
                message: "terminal approval expired or its workspace changed".into(),
            });
        }
        let session = match self.terminals.start_session(pending.session_id) {
            Ok(session) => session,
            Err(error) => {
                let _ = self.terminals.close_session(pending.session_id, false);
                return Err(terminal_error(error));
            }
        };
        Ok(ResponseValue::TerminalApproved {
            request_id,
            session: terminal_summary(&session),
        })
    }

    fn deny_terminal_start(&mut self, request_id: u64) -> Result<ResponseValue, ProtocolError> {
        let pending = self.terminal_approvals.remove(&request_id).ok_or_else(|| {
            not_found("terminal approval request was not found or expired".into())
        })?;
        self.terminals
            .close_session(pending.session_id, false)
            .map_err(terminal_error)?;
        Ok(ResponseValue::TerminalDenied { request_id })
    }

    fn poll_workspace_search(&mut self, job_id: u64) -> Result<ResponseValue, ProtocolError> {
        let job = self
            .search_jobs
            .remove(&job_id)
            .ok_or_else(|| not_found(format!("search job {job_id} was not found")))?;
        let mut matches = Vec::new();
        let mut progress = None;
        let mut summary = None;
        let mut error = None;
        let mut complete = false;
        for _ in 0..256 {
            match job.try_recv() {
                Some(SearchEvent::Match(found)) => matches.push(SearchMatch {
                    path: found.path,
                    line: found.line,
                    column_utf16: found.column_utf16,
                    match_length_utf16: found.match_length_utf16,
                    preview: found.preview,
                }),
                Some(SearchEvent::Progress(update)) => {
                    progress = Some(ProtocolSearchProgress {
                        files_scanned: update.files_scanned,
                        matches_found: update.matches_found,
                        current_path: update.current_path,
                    })
                }
                Some(SearchEvent::Completed(done)) => {
                    complete = true;
                    summary = Some(ProtocolSearchSummary {
                        files_scanned: done.files_scanned,
                        matches_found: done.matches_found,
                        skipped_binary_files: done.skipped_binary_files,
                        truncated: done.truncated,
                        cancelled: done.cancelled,
                    });
                    break;
                }
                Some(SearchEvent::Error(message)) => {
                    complete = true;
                    error = Some(message);
                    break;
                }
                None => break,
            }
        }
        if !complete {
            self.search_jobs.insert(job_id, job);
        }
        Ok(ResponseValue::SearchBatch {
            job_id,
            matches,
            progress,
            complete,
            summary,
            error,
        })
    }

    fn list_extensions(&mut self) -> Result<ResponseValue, ProtocolError> {
        self.ensure_extensions_discovered()?;
        let extensions = self
            .extensions
            .list()
            .into_iter()
            .map(protocol_extension)
            .collect();
        Ok(ResponseValue::Extensions(ExtensionCatalog {
            extensions,
            commands: self.active_extension_commands(),
            built_in_commands: self.built_in_commands(),
            menu_items: self.active_extension_menus(),
            language_servers: self.declared_language_servers(),
            tool_windows: self.active_tool_windows(),
            settings: self.active_extension_settings(),
            themes: self.active_extension_themes(),
            diagnostic_details: self.active_diagnostic_details(),
            project_wizards: self.active_project_wizards(),
        }))
    }

    fn list_inspection_rules(&self) -> Result<ResponseValue, ProtocolError> {
        let workspace = self
            .workspace
            .root()
            .ok()
            .map(|root| root.to_string_lossy().into_owned());
        let profile = self
            .inspection_profiles
            .selected_for(workspace.as_deref())
            .ok_or_else(|| invalid("no inspection profile is selected"))?;
        let rules = self
            .inspections
            .rules()
            .into_iter()
            .map(|rule| {
                let effective = profile.effective_rule(workspace.as_deref(), &rule);
                InspectionRuleSummary {
                    id: rule.id.clone(),
                    provider_id: rule.provider_id.clone(),
                    title: rule.title,
                    description: rule.description,
                    category: rule.category,
                    languages: rule.languages.into_iter().collect(),
                    default_severity: severity_name(rule.default_severity).into(),
                    enabled: effective.enabled,
                    severity: severity_name(effective.severity.unwrap_or(rule.default_severity))
                        .into(),
                    workspace_override: workspace
                        .as_ref()
                        .and_then(|id| profile.workspace_overrides.get(id))
                        .is_some_and(|settings| settings.contains_key(&rule.id)),
                }
            })
            .collect();
        Ok(ResponseValue::InspectionConfiguration(
            InspectionConfigurationSummary {
                profiles: self.inspection_profiles.profiles.keys().cloned().collect(),
                selected_profile: profile.name.clone(),
                workspace_specific: workspace.as_ref().is_some_and(|id| {
                    self.inspection_profiles
                        .workspace_selection
                        .contains_key(id)
                }),
                rules,
            },
        ))
    }

    fn select_inspection_profile(
        &mut self,
        profile_name: &str,
        workspace_specific: bool,
    ) -> Result<ResponseValue, ProtocolError> {
        let workspace = if workspace_specific {
            Some(root_string(&self.workspace)?)
        } else {
            None
        };
        let previous = self.inspection_profiles.clone();
        self.inspection_profiles
            .select(workspace.as_deref(), profile_name)
            .map_err(inspection_error)?;
        if let Err(error) = persist_desktop_settings(
            self.settings_path.as_deref(),
            &self.extension_setting_values,
            &self.inspection_profiles,
        ) {
            self.inspection_profiles = previous;
            return Err(settings_error(error));
        }
        self.list_inspection_rules()
    }

    fn create_inspection_profile(
        &mut self,
        profile_name: &str,
        workspace_specific: bool,
    ) -> Result<ResponseValue, ProtocolError> {
        let name = profile_name.trim();
        if name.is_empty() || name.len() > 64 || name.chars().any(char::is_control) {
            return Err(invalid(
                "inspection profile name must be 1–64 printable characters",
            ));
        }
        if self.inspection_profiles.profiles.contains_key(name) {
            return Err(invalid(
                "an inspection profile with this name already exists",
            ));
        }
        let workspace = if workspace_specific {
            Some(root_string(&self.workspace)?)
        } else {
            None
        };
        let previous = self.inspection_profiles.clone();
        self.inspection_profiles
            .insert(InspectionProfile::new(name));
        self.inspection_profiles
            .select(workspace.as_deref(), name)
            .map_err(inspection_error)?;
        if let Err(error) = persist_desktop_settings(
            self.settings_path.as_deref(),
            &self.extension_setting_values,
            &self.inspection_profiles,
        ) {
            self.inspection_profiles = previous;
            return Err(settings_error(error));
        }
        self.list_inspection_rules()
    }

    fn set_inspection_rule(
        &mut self,
        rule_id: &str,
        enabled: bool,
        severity: Option<&str>,
        workspace_specific: bool,
        reset: bool,
    ) -> Result<ResponseValue, ProtocolError> {
        let rule = self
            .inspections
            .rules()
            .into_iter()
            .find(|rule| rule.id == rule_id)
            .ok_or_else(|| not_found(format!("inspection rule '{rule_id}' is not active")))?;
        let severity_override = severity
            .map(parse_severity_name)
            .transpose()
            .map_err(|()| {
                invalid("inspection severity must be error, warning, information, or hint")
            })?;
        let workspace = if workspace_specific {
            Some(root_string(&self.workspace)?)
        } else {
            None
        };
        let profile_name = self
            .inspection_profiles
            .selected_for(workspace.as_deref())
            .map(|profile| profile.name.clone())
            .ok_or_else(|| invalid("no inspection profile is selected"))?;
        let previous = self.inspection_profiles.clone();
        let profile = self
            .inspection_profiles
            .profiles
            .get_mut(&profile_name)
            .expect("selected profile is present");
        if reset {
            let destination = if let Some(workspace) = workspace.as_deref() {
                profile.workspace_overrides.get_mut(workspace)
            } else {
                Some(&mut profile.rules)
            };
            if let Some(settings) = destination {
                settings.remove(rule_id);
            }
        } else {
            let effective = profile.effective_rule(workspace.as_deref(), &rule);
            let setting = RuleProfile {
                enabled,
                severity: severity_override.or(effective.severity),
                options: effective.options,
            };
            profile
                .set_rule(workspace.as_deref(), &rule, setting)
                .map_err(inspection_error)?;
        }
        if let Err(error) = persist_desktop_settings(
            self.settings_path.as_deref(),
            &self.extension_setting_values,
            &self.inspection_profiles,
        ) {
            self.inspection_profiles = previous;
            return Err(settings_error(error));
        }
        self.list_inspection_rules()
    }

    fn import_sarif_file(&self, path: &str) -> Result<ResponseValue, ProtocolError> {
        let selected = Path::new(path);
        if !selected.is_absolute() {
            return Err(invalid(
                "SARIF import requires the absolute path from the native file picker",
            ));
        }
        let canonical = std::fs::canonicalize(selected)
            .map_err(|error| invalid(format!("cannot open selected SARIF report: {error}")))?;
        let metadata = std::fs::metadata(&canonical)
            .map_err(|error| invalid(format!("cannot inspect selected SARIF report: {error}")))?;
        if !metadata.is_file() {
            return Err(invalid("selected SARIF path is not a regular file"));
        }
        if metadata.len() > MAX_SARIF_BYTES as u64 {
            return Err(inspection_error(
                koren_inspections::InspectionError::SarifSize(MAX_SARIF_BYTES),
            ));
        }
        let mut bytes = Vec::new();
        std::fs::File::open(canonical)
            .and_then(|file| {
                file.take((MAX_SARIF_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
            })
            .map_err(|error| invalid(format!("cannot read selected SARIF report: {error}")))?;
        if bytes.len() > MAX_SARIF_BYTES {
            return Err(inspection_error(
                koren_inspections::InspectionError::SarifSize(MAX_SARIF_BYTES),
            ));
        }
        let report = String::from_utf8(bytes)
            .map_err(|error| invalid(format!("SARIF report is not UTF-8: {error}")))?;
        let report_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_string();
        let imported = import_sarif(&report, &report_id, None).map_err(inspection_error)?;
        Ok(ResponseValue::ImportedSarif {
            report_id,
            findings: imported
                .findings
                .into_iter()
                .map(|finding| inspection_finding(finding, None, None))
                .collect(),
            warnings: imported.warnings,
        })
    }

    fn export_sarif_report(&self) -> Result<ResponseValue, ProtocolError> {
        let mut findings = Vec::new();
        let stored = self
            .inspection_results
            .lock()
            .map_err(|_| invalid("inspection result cache is unavailable"))?;
        for document_id in &self.open_documents {
            let snapshot = self
                .documents
                .snapshot(*document_id)
                .map_err(document_error)?;
            if let Some(current) = stored.get(&(document_id.0, snapshot.info.revision)) {
                findings.extend(current.iter().cloned());
            }
        }
        let report = export_sarif("Koren Code Editor", &findings).map_err(inspection_error)?;
        Ok(ResponseValue::SarifReport {
            document: report.document,
            warnings: report.warnings,
        })
    }

    fn start_project_inspection(&mut self) -> Result<ResponseValue, ProtocolError> {
        let workspace = self
            .workspace
            .root()
            .map_err(workspace_error)?
            .to_path_buf();
        let workspace_id = workspace.to_string_lossy().into_owned();
        let mut documents = Vec::new();
        for id in &self.open_documents {
            let snapshot = self.documents.snapshot(*id).map_err(document_error)?;
            let Some(path) = snapshot.info.path else {
                continue;
            };
            documents.push(koren_inspections::InspectionDocument {
                path: path.to_string_lossy().into_owned(),
                document_id: Some(id.0),
                revision: Some(snapshot.info.revision),
                language: self.inspection_language_for_path(&path),
                text: Arc::from(snapshot.text),
            });
        }
        if documents.is_empty() {
            return Err(invalid(
                "open at least one project document before starting inspection",
            ));
        }
        let profile = self
            .inspection_profiles
            .selected_for(Some(&workspace_id))
            .cloned()
            .ok_or_else(|| invalid("no inspection profile is selected"))?;
        let total_documents = documents.len();
        self.next_project_inspection_id = self
            .next_project_inspection_id
            .checked_add(1)
            .ok_or_else(|| invalid("project inspection ID space is exhausted"))?;
        let job_id = self.next_project_inspection_id;
        let snapshot_id = format!("project-{job_id}");
        let job = self
            .inspections
            .inspect_project(snapshot_id, documents, profile, Some(workspace_id))
            .map_err(inspection_error)?;
        let cancellation = job.cancellation_token();
        self.project_inspection_jobs
            .lock()
            .map_err(|_| invalid("project inspection job registry is unavailable"))?
            .insert(
                job_id,
                ProjectInspectionState {
                    state: "running".into(),
                    completed_documents: 0,
                    total_documents,
                    current_path: None,
                    findings: Vec::new(),
                    error: None,
                    cancellation,
                },
            );
        let jobs = self.project_inspection_jobs.clone();
        let results = self.inspection_results.clone();
        let worker = thread::Builder::new()
            .name(format!("koren-project-inspection-{job_id}"))
            .spawn(move || {
                loop {
                    if let Some(progress) = job.try_progress() {
                        if let Ok(mut states) = jobs.lock() {
                            if let Some(state) = states.get_mut(&job_id) {
                                state.completed_documents = progress.completed_documents;
                                state.total_documents = progress.total_documents;
                                state.current_path = Some(progress.current_path);
                            }
                        }
                    }
                    match job.try_result() {
                        Ok(Some(findings)) => {
                            if let Ok(mut stored) = results.lock() {
                                for finding in &findings {
                                    if let (Some(document_id), Some(revision)) =
                                        (finding.document_id, finding.document_revision)
                                    {
                                        stored
                                            .entry((document_id, revision))
                                            .or_default()
                                            .push(finding.clone());
                                    }
                                }
                            }
                            if let Ok(mut states) = jobs.lock() {
                                if let Some(state) = states.get_mut(&job_id) {
                                    state.state = "completed".into();
                                    state.findings = findings;
                                }
                            }
                            break;
                        }
                        Err(error) => {
                            if let Ok(mut states) = jobs.lock() {
                                if let Some(state) = states.get_mut(&job_id) {
                                    state.state =
                                        if error == koren_inspections::InspectionError::Cancelled {
                                            "cancelled"
                                        } else {
                                            "failed"
                                        }
                                        .into();
                                    state.error = Some(error.to_string());
                                }
                            }
                            break;
                        }
                        Ok(None) => thread::sleep(Duration::from_millis(25)),
                    }
                }
            });
        if let Err(error) = worker {
            if let Ok(mut jobs) = self.project_inspection_jobs.lock() {
                jobs.remove(&job_id);
            }
            return Err(ProtocolError {
                code: ErrorCode::Internal,
                message: format!("could not start project inspection monitor: {error}"),
            });
        }
        Ok(ResponseValue::ProjectInspectionStarted { job_id })
    }

    fn project_inspection_status(&self, job_id: u64) -> Result<ResponseValue, ProtocolError> {
        let jobs = self
            .project_inspection_jobs
            .lock()
            .map_err(|_| invalid("project inspection job registry is unavailable"))?;
        let state = jobs
            .get(&job_id)
            .ok_or_else(|| not_found(format!("project inspection job '{job_id}' was not found")))?;
        Ok(ResponseValue::ProjectInspectionStatus(
            ProjectInspectionSummary {
                job_id,
                state: state.state.clone(),
                completed_documents: state.completed_documents,
                total_documents: state.total_documents,
                current_path: state.current_path.clone(),
                result_count: state.findings.len(),
                error: state.error.clone(),
            },
        ))
    }

    fn cancel_project_inspection(&self, job_id: u64) -> Result<ResponseValue, ProtocolError> {
        let mut jobs = self
            .project_inspection_jobs
            .lock()
            .map_err(|_| invalid("project inspection job registry is unavailable"))?;
        let state = jobs
            .get_mut(&job_id)
            .ok_or_else(|| not_found(format!("project inspection job '{job_id}' was not found")))?;
        if state.state == "running" {
            state.state = "cancelling".into();
            state.cancellation.cancel();
        }
        Ok(ResponseValue::ProjectInspectionCancelled { job_id })
    }

    fn project_inspection_findings(
        &self,
        job_id: u64,
        offset: usize,
        limit: usize,
    ) -> Result<ResponseValue, ProtocolError> {
        let jobs = self
            .project_inspection_jobs
            .lock()
            .map_err(|_| invalid("project inspection job registry is unavailable"))?;
        let state = jobs
            .get(&job_id)
            .ok_or_else(|| not_found(format!("project inspection job '{job_id}' was not found")))?;
        if state.state != "completed" {
            return Err(invalid("project inspection results are not ready"));
        }
        let limit = limit.clamp(1, 100);
        let end = offset.saturating_add(limit).min(state.findings.len());
        let findings = state
            .findings
            .get(offset.min(state.findings.len())..end)
            .unwrap_or_default()
            .iter()
            .cloned()
            .map(|finding| inspection_finding(finding, None, None))
            .collect();
        Ok(ResponseValue::ProjectInspectionFindings(
            ProjectInspectionFindingPage {
                job_id,
                offset,
                next_offset: (end < state.findings.len()).then_some(end),
                findings,
            },
        ))
    }

    fn inspection_language_for_path(&self, path: &Path) -> String {
        let suffix = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        for extension in self
            .extensions
            .list()
            .into_iter()
            .filter(|item| item.state == ExtensionState::Active)
        {
            if let Ok(contributions) = self.extensions.contributions(&extension.id) {
                if let Some(language) = contributions
                    .languages
                    .iter()
                    .find(|language| language.extensions.iter().any(|value| value == suffix))
                {
                    return language.id.clone();
                }
            }
        }
        "plaintext".into()
    }

    fn active_tool_windows(&self) -> Vec<ExtensionToolWindowSummary> {
        let mut panels = Vec::new();
        for extension in self
            .extensions
            .list()
            .into_iter()
            .filter(|item| item.state == ExtensionState::Active)
        {
            if let Ok(items) = self.extensions.contributions(&extension.id) {
                for panel in &items.tool_windows {
                    let mut view = protocol_ui_document(&panel.initial_view);
                    if let Some(text) = self.updated_panel_text.get(&panel.id) {
                        if let Some(ExtensionUiElement::Text { text: contents, .. }) = view
                            .elements
                            .iter_mut()
                            .find(|element| matches!(element, ExtensionUiElement::Text { .. }))
                        {
                            contents.clone_from(text);
                        }
                    }
                    panels.push(ExtensionToolWindowSummary {
                        id: panel.id.clone(),
                        title: panel.title.clone(),
                        initial_view: view,
                    });
                }
            }
        }
        panels.sort_by(|left, right| left.id.cmp(&right.id));
        panels
    }

    fn active_extension_settings(&self) -> Vec<ExtensionSettingSummary> {
        let mut settings = Vec::new();
        for extension in self
            .extensions
            .list()
            .into_iter()
            .filter(|item| item.state == ExtensionState::Active)
        {
            if let Ok(items) = self.extensions.contributions(&extension.id) {
                for setting in &items.settings {
                    let (kind, default) = protocol_setting_kind(&setting.kind);
                    let value = self
                        .extension_setting_values
                        .get(&setting.id)
                        .cloned()
                        .filter(|value| valid_setting_value(&setting.kind, value))
                        .unwrap_or(default);
                    settings.push(ExtensionSettingSummary {
                        id: setting.id.clone(),
                        title: setting.title.clone(),
                        description: setting.description.clone(),
                        kind,
                        value,
                    });
                }
            }
        }
        settings.sort_by(|left, right| left.id.cmp(&right.id));
        settings
    }

    fn active_extension_themes(&self) -> Vec<ExtensionThemeSummary> {
        let mut themes = Vec::new();
        for extension in self
            .extensions
            .list()
            .into_iter()
            .filter(|item| item.state == ExtensionState::Active)
        {
            if let Ok(items) = self.extensions.contributions(&extension.id) {
                for theme in &items.themes {
                    themes.push(ExtensionThemeSummary {
                        id: theme.id.clone(),
                        title: theme.title.clone(),
                        base: match theme.base {
                            ThemeBase::Dark => "dark",
                            ThemeBase::Light => "light",
                        }
                        .into(),
                        tokens: theme
                            .tokens
                            .iter()
                            .map(|(key, value)| (format!("{key:?}").to_lowercase(), value.clone()))
                            .collect(),
                    });
                }
            }
        }
        themes.sort_by(|left, right| left.id.cmp(&right.id));
        themes
    }

    fn active_diagnostic_details(&self) -> Vec<ExtensionDiagnosticDetailSummary> {
        let mut details = Vec::new();
        for extension in self
            .extensions
            .list()
            .into_iter()
            .filter(|item| item.state == ExtensionState::Active)
        {
            if let Ok(items) = self.extensions.contributions(&extension.id) {
                details.extend(items.diagnostic_details.iter().map(|detail| {
                    ExtensionDiagnosticDetailSummary {
                        id: detail.id.clone(),
                        source: detail.source.clone(),
                        code: detail.code.clone(),
                        title: detail.title.clone(),
                        explanation: detail.explanation.clone(),
                        suggested_command: detail.suggested_command.clone(),
                        related: detail.related.clone(),
                    }
                }));
            }
        }
        details.sort_by(|left, right| left.id.cmp(&right.id));
        details
    }

    fn active_project_wizards(&self) -> Vec<ExtensionProjectWizardSummary> {
        let mut wizards = Vec::new();
        for extension in self
            .extensions
            .list()
            .into_iter()
            .filter(|item| item.state == ExtensionState::Active)
        {
            if let Ok(items) = self.extensions.contributions(&extension.id) {
                wizards.extend(items.project_wizards.iter().map(|wizard| {
                    ExtensionProjectWizardSummary {
                        id: wizard.id.clone(),
                        title: wizard.title.clone(),
                        form: protocol_ui_document(&wizard.form),
                    }
                }));
            }
        }
        wizards.sort_by(|left, right| left.id.cmp(&right.id));
        wizards
    }

    fn store_panel_text(&mut self, id: String, text: String) {
        let current_bytes = self
            .updated_panel_text
            .values()
            .map(String::len)
            .sum::<usize>();
        let replaced_bytes = self.updated_panel_text.get(&id).map_or(0, String::len);
        if current_bytes
            .saturating_sub(replaced_bytes)
            .saturating_add(text.len())
            <= MAX_EXTENSION_PANEL_STATE_BYTES
        {
            self.updated_panel_text.insert(id, text);
        }
    }

    fn set_extension_setting(
        &mut self,
        setting_id: &str,
        value: ExtensionSettingValue,
    ) -> Result<ResponseValue, ProtocolError> {
        self.ensure_extensions_discovered()?;
        let mut definition = None;
        for extension in self
            .extensions
            .list()
            .into_iter()
            .filter(|item| item.state == ExtensionState::Active)
        {
            if let Ok(items) = self.extensions.contributions(&extension.id) {
                if let Some(setting) = items
                    .settings
                    .iter()
                    .find(|setting| setting.id == setting_id)
                {
                    definition = Some(setting.kind.clone());
                    break;
                }
            }
        }
        let kind = definition.ok_or_else(|| {
            not_found(format!(
                "active extension setting '{setting_id}' was not found"
            ))
        })?;
        if !valid_setting_value(&kind, &value) {
            return Err(invalid(
                "setting value does not match its declared type or range",
            ));
        }
        let previous = self
            .extension_setting_values
            .insert(setting_id.into(), value);
        if let Err(error) = persist_desktop_settings(
            self.settings_path.as_deref(),
            &self.extension_setting_values,
            &self.inspection_profiles,
        ) {
            if let Some(previous) = previous {
                self.extension_setting_values
                    .insert(setting_id.into(), previous);
            } else {
                self.extension_setting_values.remove(setting_id);
            }
            return Err(ProtocolError {
                code: ErrorCode::Internal,
                message: format!("could not save extension settings: {error}"),
            });
        }
        if let Some((owner, _)) = setting_id.rsplit_once('.') {
            if let Some(runtime) = self.lua_runtimes.get(owner) {
                let _ = runtime.update_context(self.lua_context());
                let payload = serde_json::to_string(&serde_json::json!({ "id": setting_id, "value": self.extension_setting_values.get(setting_id) })).unwrap_or_default();
                let _ = runtime.dispatch_event("settings_changed", &payload);
            }
        }
        Ok(ResponseValue::ExtensionSettingUpdated {
            setting_id: setting_id.into(),
        })
    }

    fn ensure_extensions_discovered(&mut self) -> Result<(), ProtocolError> {
        if self.extensions_discovered {
            return Ok(());
        }
        let root = self
            .extension_root
            .as_ref()
            .ok_or_else(|| invalid("no absolute extension install directory is configured"))?;
        std::fs::create_dir_all(root).map_err(|error| ProtocolError {
            code: ErrorCode::Internal,
            message: format!("cannot prepare extension directory: {error}"),
        })?;
        self.extensions
            .discover_installed(root)
            .map_err(extension_error)?;
        self.extensions_discovered = true;
        Ok(())
    }

    fn set_extension_enabled(
        &mut self,
        id: &str,
        enabled: bool,
    ) -> Result<ResponseValue, ProtocolError> {
        self.ensure_extensions_discovered()?;
        if enabled {
            self.extensions.enable(id).map_err(extension_error)?;
        } else {
            self.cancel_all_document_inspections();
            self.cancel_active_project_jobs();
            self.inspections.unregister(id);
            if let Ok(mut stored) = self.inspection_results.lock() {
                stored.retain(|_, findings| {
                    findings.retain(|finding| finding.provider_id != id);
                    !findings.is_empty()
                });
            }
            if let Some(runtime) = self.lua_runtimes.remove(id) {
                let _ = runtime.deactivate();
            }
            let server_ids = self
                .language_server_owners
                .iter()
                .filter_map(|(server_id, (owner, _))| (owner == id).then_some(*server_id))
                .collect::<Vec<_>>();
            for server_id in server_ids {
                let _ = self.stop_running_language_server(server_id);
            }
            self.pending_language_servers
                .retain(|_, pending| pending.extension_id != id);
            self.updated_panel_text
                .retain(|panel_id, _| !panel_id.starts_with(&format!("{id}.")));
            self.extensions.disable(id).map_err(extension_error)?;
        }
        let summary = self
            .extensions
            .list()
            .into_iter()
            .find(|item| item.id == id)
            .ok_or_else(|| not_found(format!("extension '{id}' was not found")))?;
        Ok(ResponseValue::ExtensionChanged(protocol_extension(summary)))
    }

    fn activate_extension(&mut self, id: &str) -> Result<ResponseValue, ProtocolError> {
        self.ensure_extensions_discovered()?;
        let (_, path) = self.extensions.manifest(id).map_err(extension_error)?;
        let path = path.to_path_buf();
        let context = self.lua_context();
        let mut runtime = None;
        self.extensions
            .activate(id, |manifest, contributions| {
                runtime = Some(
                    LuaExtensionRuntime::activate_file(
                        manifest,
                        &path,
                        context,
                        LuaLimits::default(),
                        contributions,
                    )
                    .map_err(|error| error.to_string())?,
                );
                Ok(())
            })
            .map_err(extension_error)?;
        if let Some(runtime) = runtime {
            if !runtime.rules().is_empty() {
                if let Err(error) = self.inspections.register(Arc::new(runtime.clone())) {
                    let _ = runtime.deactivate();
                    let _ = self.extensions.disable(id);
                    return Err(ProtocolError {
                        code: ErrorCode::InvalidRequest,
                        message: format!("could not register extension inspections: {error}"),
                    });
                }
            }
            self.lua_runtimes.insert(id.to_owned(), runtime);
        }
        let summary = self
            .extensions
            .list()
            .into_iter()
            .find(|item| item.id == id)
            .ok_or_else(|| not_found(format!("extension '{id}' was not found")))?;
        Ok(ResponseValue::ExtensionChanged(protocol_extension(summary)))
    }

    fn execute_command(&mut self, id: &str) -> Result<ResponseValue, ProtocolError> {
        match self.commands.invoke(id, &[]) {
            Ok(output) => return Ok(ResponseValue::CommandOutput(output)),
            Err(CommandError::UnknownCommand(_)) => {}
            Err(error) => return Err(command_error(error)),
        }
        let extension_id = self
            .extensions
            .list()
            .into_iter()
            .map(|extension| extension.id)
            .filter(|extension_id| id.starts_with(&format!("{extension_id}.")))
            .max_by_key(String::len)
            .ok_or_else(|| not_found(format!("command '{id}' was not found")))?;
        let context = self.command_context();
        if let Some(runtime) = self.lua_runtimes.get(&extension_id) {
            runtime
                .update_context(self.lua_context())
                .map_err(lua_error)?;
        }
        let output = self
            .extensions
            .invoke_command(id, &context, &[])
            .map_err(extension_error)?;
        if let Some(runtime) = self.lua_runtimes.get(&extension_id) {
            for edit in runtime.take_edit_requests().map_err(lua_error)? {
                self.documents
                    .apply_edits(
                        DocumentId(edit.document_id),
                        edit.expected_revision,
                        &[TextEdit {
                            range: edit.range,
                            new_text: edit.new_text,
                        }],
                    )
                    .map_err(document_error)?;
            }
            for (panel_id, text) in runtime.take_ui_updates().map_err(lua_error)? {
                let qualified = format!("{extension_id}.{panel_id}");
                if self
                    .extensions
                    .contributions(&extension_id)
                    .is_ok_and(|items| items.tool_windows.iter().any(|panel| panel.id == qualified))
                {
                    self.store_panel_text(qualified, text);
                }
            }
        }
        Ok(ResponseValue::CommandOutput(output))
    }

    fn command_context(&self) -> CommandContext {
        let active = self
            .active_document
            .and_then(|id| self.documents.snapshot(id).ok());
        CommandContext {
            workspace_root: self.workspace.root().ok().map(Path::to_path_buf),
            active_document_id: active.as_ref().map(|snapshot| snapshot.info.id.0),
            active_document_path: active
                .as_ref()
                .and_then(|snapshot| snapshot.info.path.clone()),
            active_document_revision: active.as_ref().map(|snapshot| snapshot.info.revision),
            active_document_contents: active.map(|snapshot| snapshot.text),
        }
    }

    fn lua_context(&self) -> LuaHostContext {
        let active_document = self
            .active_document
            .and_then(|id| self.documents.snapshot(id).ok())
            .map(|snapshot| LuaDocumentContext {
                id: snapshot.info.id.0,
                path: snapshot.info.path,
                revision: snapshot.info.revision,
                contents: snapshot.text,
            });
        LuaHostContext {
            workspace_root: self.workspace.root().ok().map(Path::to_path_buf),
            active_document,
            settings: self.effective_extension_settings(),
        }
    }

    fn effective_extension_settings(&self) -> BTreeMap<String, SettingValue> {
        let mut values = BTreeMap::new();
        for extension in self
            .extensions
            .list()
            .into_iter()
            .filter(|item| item.state == ExtensionState::Active)
        {
            if let Ok(items) = self.extensions.contributions(&extension.id) {
                for setting in &items.settings {
                    let value = self
                        .extension_setting_values
                        .get(&setting.id)
                        .cloned()
                        .filter(|value| valid_setting_value(&setting.kind, value));
                    let effective = match value {
                        Some(ExtensionSettingValue::Boolean(value)) => SettingValue::Boolean(value),
                        Some(ExtensionSettingValue::Integer(value)) => SettingValue::Integer(value),
                        Some(ExtensionSettingValue::Text(value)) => SettingValue::Text(value),
                        None => match &setting.kind {
                            SettingKind::Boolean { default } => SettingValue::Boolean(*default),
                            SettingKind::Integer { default, .. } => SettingValue::Integer(*default),
                            SettingKind::Text { default, .. }
                            | SettingKind::Choice { default, .. } => {
                                SettingValue::Text(default.clone())
                            }
                        },
                    };
                    values.insert(setting.id.clone(), effective);
                }
            }
        }
        values
    }

    fn active_extension_commands(&self) -> Vec<ExtensionCommandSummary> {
        let mut commands = Vec::new();
        let context = self.command_context();
        for extension in self.extensions.list() {
            if extension.state != ExtensionState::Active {
                continue;
            }
            if let Ok(contributions) = self.extensions.contributions(&extension.id) {
                commands.extend(contributions.commands.iter().map(|(id, command)| {
                    ExtensionCommandSummary {
                        id: id.clone(),
                        title: command.title.clone(),
                        description: command.description.clone(),
                        shortcut: command.shortcut.clone(),
                        enabled: match command.enablement {
                            CommandEnablement::Always => true,
                            CommandEnablement::WorkspaceOpen => context.workspace_root.is_some(),
                            CommandEnablement::ActiveDocument => {
                                context.active_document_id.is_some()
                            }
                        },
                    }
                }));
            }
        }
        commands.sort_by(|left, right| left.id.cmp(&right.id));
        commands
    }

    fn built_in_commands(&self) -> Vec<CommandCatalogSummary> {
        self.commands
            .ids()
            .into_iter()
            .map(|id| {
                let raw_title = id.rsplit('.').next().unwrap_or(id).replace(['-', '_'], " ");
                let mut characters = raw_title.chars();
                let title = characters.next().map_or_else(String::new, |first| {
                    first.to_uppercase().collect::<String>() + characters.as_str()
                });
                CommandCatalogSummary {
                    id: id.into(),
                    title,
                    description: self.commands.description(id).unwrap_or_default().into(),
                    shortcut: None,
                    enabled: true,
                }
            })
            .collect()
    }

    fn active_extension_menus(&self) -> Vec<ExtensionMenuSummary> {
        let mut menus = Vec::new();
        let context = self.command_context();
        for extension in self.extensions.list() {
            if extension.state != ExtensionState::Active {
                continue;
            }
            if let Ok(contributions) = self.extensions.contributions(&extension.id) {
                menus.extend(contributions.menus.iter().map(|menu| {
                    let enabled =
                        contributions
                            .commands
                            .get(&menu.command_id)
                            .is_some_and(|command| match command.enablement {
                                CommandEnablement::Always => true,
                                CommandEnablement::WorkspaceOpen => {
                                    context.workspace_root.is_some()
                                }
                                CommandEnablement::ActiveDocument => {
                                    context.active_document_id.is_some()
                                }
                            });
                    ExtensionMenuSummary {
                        command_id: menu.command_id.clone(),
                        label: menu.label.clone(),
                        location: match menu.location {
                            MenuLocation::Tools => "tools",
                            MenuLocation::Navigate => "navigate",
                            MenuLocation::EditorContext => "editor_context",
                            MenuLocation::ProjectExplorerContext => "project_explorer_context",
                            MenuLocation::CommandPalette => "command_palette",
                        }
                        .into(),
                        enabled,
                    }
                }));
            }
        }
        menus.sort_by(|left, right| left.command_id.cmp(&right.command_id));
        menus
    }

    fn declared_language_servers(&self) -> Vec<LanguageServerSummary> {
        let mut servers = Vec::new();
        for extension in self.extensions.list() {
            if let Ok((manifest, _)) = self.extensions.manifest(&extension.id) {
                servers.extend(manifest.language_servers.iter().map(|server| {
                    LanguageServerSummary {
                        extension_id: extension.id.clone(),
                        configuration_id: server.id.clone(),
                        command: server.command.clone(),
                        arguments: server.arguments.clone(),
                        languages: server.languages.clone(),
                        file_extensions: server.file_extensions.clone(),
                        environment_keys: server.environment.keys().cloned().collect(),
                    }
                }));
            }
        }
        servers.sort_by(|left, right| {
            (&left.extension_id, &left.configuration_id)
                .cmp(&(&right.extension_id, &right.configuration_id))
        });
        servers
    }

    fn request_language_server(
        &mut self,
        extension_id: &str,
        configuration_id: &str,
    ) -> Result<ResponseValue, ProtocolError> {
        self.ensure_extensions_discovered()?;
        let enabled = self
            .extensions
            .list()
            .iter()
            .any(|extension| extension.id == extension_id && extension.enabled);
        if !enabled {
            return Err(invalid(
                "enable the extension before starting its language server",
            ));
        }
        let workspace_root = self
            .workspace
            .root()
            .map_err(workspace_error)?
            .to_path_buf();
        let (manifest, _) = self
            .extensions
            .manifest(extension_id)
            .map_err(extension_error)?;
        let server = manifest
            .language_servers
            .iter()
            .find(|server| server.id == configuration_id)
            .ok_or_else(|| {
                not_found(format!(
                    "language-server configuration '{configuration_id}' was not found"
                ))
            })?;
        let configuration = ServerConfig {
            id: format!("{extension_id}.{configuration_id}"),
            command: server.command.clone(),
            arguments: server.arguments.clone(),
            workspace_root,
            environment: server.environment.clone(),
        };
        self.try_start_language_server(extension_id, configuration_id, configuration)
    }

    fn try_start_language_server(
        &mut self,
        extension_id: &str,
        configuration_id: &str,
        configuration: ServerConfig,
    ) -> Result<ResponseValue, ProtocolError> {
        match self
            .language_servers
            .start_server(configuration.clone(), &mut DenyLanguageServer)
        {
            Ok(server_id) => {
                self.register_language_server(extension_id, configuration_id, server_id);
                Ok(ResponseValue::LanguageServerStarted {
                    server_id: server_id.get(),
                    extension_id: extension_id.into(),
                    configuration_id: configuration_id.into(),
                })
            }
            Err(koren_lsp::LspError::AuthorizationDenied(_)) => {
                self.next_language_server_approval = self
                    .next_language_server_approval
                    .checked_add(1)
                    .ok_or_else(|| invalid("language-server approval identifiers are exhausted"))?;
                let approval_id = self.next_language_server_approval;
                self.pending_language_servers.insert(
                    approval_id,
                    PendingLanguageServer {
                        extension_id: extension_id.into(),
                        configuration_id: configuration_id.into(),
                        configuration: configuration.clone(),
                    },
                );
                Ok(ResponseValue::LanguageServerApprovalRequired(
                    LanguageServerApprovalSummary {
                        approval_id,
                        extension_id: extension_id.into(),
                        configuration_id: configuration_id.into(),
                        command: configuration.command,
                        arguments: configuration.arguments,
                        working_directory: configuration.workspace_root.display().to_string(),
                        environment_keys: configuration.environment.keys().cloned().collect(),
                    },
                ))
            }
            Err(error) => Err(lsp_error(error)),
        }
    }

    fn approve_language_server(
        &mut self,
        approval_id: u64,
    ) -> Result<ResponseValue, ProtocolError> {
        let pending = self
            .pending_language_servers
            .remove(&approval_id)
            .ok_or_else(|| not_found("language-server approval was not found or expired".into()))?;
        let active_workspace = self.workspace.root().map_err(workspace_error)?;
        if pending.configuration.workspace_root != active_workspace {
            return Err(ProtocolError {
                code: ErrorCode::NotOpen,
                message: "language-server approval workspace is no longer active".into(),
            });
        }
        let server_id = self
            .language_servers
            .start_server(pending.configuration, &mut ApproveLanguageServer)
            .map_err(lsp_error)?;
        self.register_language_server(&pending.extension_id, &pending.configuration_id, server_id);
        Ok(ResponseValue::LanguageServerStarted {
            server_id: server_id.get(),
            extension_id: pending.extension_id,
            configuration_id: pending.configuration_id,
        })
    }

    fn deny_language_server(&mut self, approval_id: u64) -> Result<ResponseValue, ProtocolError> {
        self.pending_language_servers
            .remove(&approval_id)
            .ok_or_else(|| not_found("language-server approval was not found or expired".into()))?;
        Ok(ResponseValue::LanguageServerDenied { approval_id })
    }

    fn stop_language_server(&mut self, id: u64) -> Result<ResponseValue, ProtocolError> {
        self.stop_running_language_server(id)?;
        Ok(ResponseValue::LanguageServerStopped { server_id: id })
    }

    fn stop_running_language_server(&mut self, id: u64) -> Result<(), ProtocolError> {
        let server_id = self
            .active_language_servers
            .get(&id)
            .copied()
            .ok_or_else(|| not_found(format!("language server '{id}' was not found")))?;
        let documents = self
            .language_document_sync
            .get(&id)
            .map(LspDocumentSync::document_ids)
            .unwrap_or_default();
        for document_id in documents {
            let notification = self
                .language_document_sync
                .get_mut(&id)
                .and_then(|sync| sync.close(document_id).ok());
            if let Some(notification) = notification {
                if let Ok(server) = self.language_servers.handle(server_id) {
                    let _ = server.notify(notification);
                }
            }
        }
        self.active_language_servers.remove(&id);
        self.initialized_language_servers.remove(&id);
        self.language_server_owners.remove(&id);
        self.language_document_sync.remove(&id);
        self.language_servers
            .stop_server(server_id)
            .map_err(lsp_error)
    }

    fn language_targets(
        &self,
        document_id: u64,
        expected_revision: u64,
    ) -> Result<Vec<(u64, LspServerHandle, String, i32)>, ProtocolError> {
        let document = DocumentId(document_id);
        let snapshot = self.documents.snapshot(document).map_err(document_error)?;
        if snapshot.info.revision != expected_revision {
            return Err(ProtocolError {
                code: ErrorCode::Conflict,
                message: "language request refers to an obsolete document revision".into(),
            });
        }
        let mut targets = Vec::new();
        for (application_id, server_id) in &self.active_language_servers {
            if !self.initialized_language_servers.contains(application_id) {
                continue;
            }
            let Some(sync) = self.language_document_sync.get(application_id) else {
                continue;
            };
            let (Some(uri), Some((version, revision))) =
                (sync.uri(document), sync.version(document))
            else {
                continue;
            };
            if revision == expected_revision {
                if let Ok(server) = self.language_servers.handle(*server_id) {
                    targets.push((*application_id, server, uri.to_owned(), version));
                }
            }
        }
        Ok(targets)
    }

    fn language_hover(
        &mut self,
        document_id: u64,
        expected_revision: u64,
        position: koren_protocol::TextPosition,
    ) -> Result<ResponseValue, ProtocolError> {
        let targets = self.language_targets(document_id, expected_revision)?;
        for (_, server, uri, version) in targets {
            let request = LspRequest::Hover {
                uri,
                version,
                position,
                timeout: LANGUAGE_FEATURE_TIMEOUT,
            };
            match server.request(request).and_then(|ticket| {
                ticket.recv_timeout(LANGUAGE_FEATURE_TIMEOUT + Duration::from_millis(20))
            }) {
                Ok(LspResponse::Hover(result)) if result.is_current(version) => {
                    let value = result.value;
                    return Ok(ResponseValue::LanguageHover {
                        document_revision: expected_revision,
                        contents: value.as_ref().map(|hover| hover.contents.clone()),
                        range: value.and_then(|hover| hover.range),
                    });
                }
                Err(koren_lsp::LspError::UnsupportedFeature(_))
                | Err(koren_lsp::LspError::RequestTimeout(_))
                | Err(koren_lsp::LspError::RequestCancelled)
                | Err(koren_lsp::LspError::RequestQueueFull)
                | Err(koren_lsp::LspError::ServerNotReady(_)) => continue,
                Err(error) => return Err(lsp_error(error)),
                _ => continue,
            }
        }
        Ok(ResponseValue::LanguageHover {
            document_revision: expected_revision,
            contents: None,
            range: None,
        })
    }

    fn language_completion(
        &mut self,
        document_id: u64,
        expected_revision: u64,
        position: koren_protocol::TextPosition,
    ) -> Result<ResponseValue, ProtocolError> {
        let targets = self.language_targets(document_id, expected_revision)?;
        for (_, server, uri, version) in targets {
            let request = LspRequest::Completion {
                uri,
                version,
                position,
                timeout: LANGUAGE_FEATURE_TIMEOUT,
            };
            match server.request(request).and_then(|ticket| {
                ticket.recv_timeout(LANGUAGE_FEATURE_TIMEOUT + Duration::from_millis(20))
            }) {
                Ok(LspResponse::Completion(result)) if result.is_current(version) => {
                    return Ok(ResponseValue::LanguageCompletions {
                        document_revision: expected_revision,
                        items: result
                            .value
                            .into_iter()
                            .map(|item| koren_protocol::LanguageCompletionItem {
                                label: item.label,
                                detail: item.detail,
                                insert_text: item.insert_text,
                                documentation: item.documentation,
                            })
                            .collect(),
                    });
                }
                Err(koren_lsp::LspError::UnsupportedFeature(_))
                | Err(koren_lsp::LspError::RequestTimeout(_))
                | Err(koren_lsp::LspError::RequestCancelled)
                | Err(koren_lsp::LspError::RequestQueueFull)
                | Err(koren_lsp::LspError::ServerNotReady(_)) => continue,
                Err(error) => return Err(lsp_error(error)),
                _ => continue,
            }
        }
        Ok(ResponseValue::LanguageCompletions {
            document_revision: expected_revision,
            items: Vec::new(),
        })
    }

    fn language_definition(
        &mut self,
        document_id: u64,
        expected_revision: u64,
        position: koren_protocol::TextPosition,
    ) -> Result<ResponseValue, ProtocolError> {
        let targets = self.language_targets(document_id, expected_revision)?;
        for (_, server, uri, version) in targets {
            let request = LspRequest::Definition {
                uri,
                version,
                position,
                timeout: LANGUAGE_FEATURE_TIMEOUT,
            };
            match server.request(request).and_then(|ticket| {
                ticket.recv_timeout(LANGUAGE_FEATURE_TIMEOUT + Duration::from_millis(20))
            }) {
                Ok(LspResponse::Definition(result)) if result.is_current(version) => {
                    return Ok(ResponseValue::LanguageDefinitions {
                        document_revision: expected_revision,
                        locations: result
                            .value
                            .into_iter()
                            .map(|location| koren_protocol::LanguageLocation {
                                uri: location.uri,
                                range: location.range,
                            })
                            .collect(),
                    });
                }
                Err(koren_lsp::LspError::UnsupportedFeature(_))
                | Err(koren_lsp::LspError::RequestTimeout(_))
                | Err(koren_lsp::LspError::RequestCancelled)
                | Err(koren_lsp::LspError::RequestQueueFull)
                | Err(koren_lsp::LspError::ServerNotReady(_)) => continue,
                Err(error) => return Err(lsp_error(error)),
                _ => continue,
            }
        }
        Ok(ResponseValue::LanguageDefinitions {
            document_revision: expected_revision,
            locations: Vec::new(),
        })
    }

    fn ensure_task_scope(&self, task_id: TaskId) -> Result<(), ProtocolError> {
        let root = self.workspace.root().map_err(workspace_error)?;
        let (_, definition) = self
            .tasks
            .list()
            .into_iter()
            .find(|(id, _)| *id == task_id)
            .ok_or_else(|| ProtocolError {
                code: ErrorCode::NotFound,
                message: "task was not found".into(),
            })?;
        if !definition.working_directory.starts_with(root) {
            return Err(invalid("task is outside the active workspace"));
        }
        if let Some(project_id) = definition.project_id {
            let project = self
                .projects
                .info(ProjectId::from_raw(project_id))
                .map_err(project_error)?;
            if project.workspace_root != root {
                return Err(invalid("task project is outside the active workspace"));
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn drain_events(&mut self) -> Vec<DesktopEvent> {
        self.drain_events_for_subscribers(true)
    }

    fn drain_events_for_subscribers(&mut self, has_subscribers: bool) -> Vec<DesktopEvent> {
        self.expire_terminal_approvals();
        self.expire_debug_approvals();
        let mut events = Vec::new();
        events.extend(self.debugger.drain_events().into_iter().map(|event| {
            let session_id = debug_event_session_id(&event);
            DesktopEvent {
                kind: "debug_event".into(),
                debug_id: Some(session_id),
                debug_event: Some(serde_json::to_value(event).unwrap_or(serde_json::Value::Null)),
                ..empty_event()
            }
        }));
        for event in self.terminals.drain_events(has_subscribers) {
            match event {
                TerminalEvent::Output {
                    id,
                    sequence,
                    bytes,
                } => events.push(DesktopEvent {
                    kind: "terminal_output".into(),
                    terminal_id: Some(id.get()),
                    sequence: Some(sequence),
                    data_base64: Some(BASE64.encode(bytes)),
                    ..empty_event()
                }),
                TerminalEvent::StateChanged {
                    id,
                    state,
                    exit_code,
                    process_group_active,
                    error,
                } => events.push(DesktopEvent {
                    kind: "terminal_state_changed".into(),
                    terminal_id: Some(id.get()),
                    terminal_state: Some(format!("{state:?}").to_lowercase()),
                    exit_code,
                    terminal_process_group_active: Some(process_group_active),
                    message: error,
                    ..empty_event()
                }),
                TerminalEvent::Closed { id } => events.push(DesktopEvent {
                    kind: "terminal_closed".into(),
                    terminal_id: Some(id.get()),
                    ..empty_event()
                }),
            }
        }
        events.extend(
            self.workspace
                .drain_events()
                .into_iter()
                .map(event_from_platform),
        );
        events.extend(
            self.documents
                .drain_events()
                .into_iter()
                .map(event_from_platform),
        );
        events.extend(
            self.projects
                .drain_events()
                .into_iter()
                .map(event_from_platform),
        );
        events.extend(
            self.tasks
                .drain_platform_events()
                .into_iter()
                .map(event_from_platform),
        );
        events.extend(self.git_events.drain().into_iter().map(event_from_platform));
        let _ = self.tasks.drain_events();
        for actor_event in self.language_servers.drain_events() {
            match actor_event {
                LspActorEvent::Initialized(server_id) => {
                    let id = server_id.get();
                    if self.active_language_servers.contains_key(&id) {
                        self.initialized_language_servers.insert(id);
                        events.push(DesktopEvent {
                            kind: "language_server_initialized".into(),
                            server_id: Some(id),
                            ..empty_event()
                        });
                    }
                }
                LspActorEvent::Server(server_id, service_event) => {
                    let application_id = server_id.get();
                    match service_event {
                        ServerEvent::Diagnostics(mut diagnostics) => {
                            let Some(sync) = self.language_document_sync.get(&application_id)
                            else {
                                continue;
                            };
                            let Some(document_id) = sync.document_id(&diagnostics.uri) else {
                                continue;
                            };
                            let Some((version, revision)) = sync.version(document_id) else {
                                continue;
                            };
                            if diagnostics
                                .document_version
                                .is_some_and(|seen| seen != version)
                            {
                                continue;
                            }
                            diagnostics.document_id = Some(document_id.0);
                            for diagnostic in &mut diagnostics.diagnostics {
                                diagnostic.document_revision = Some(revision);
                            }
                            events.push(DesktopEvent {
                                kind: "language_diagnostics".into(),
                                document_id: Some(document_id.0),
                                revision: Some(revision),
                                server_id: Some(application_id),
                                diagnostics: Some(diagnostics),
                                ..empty_event()
                            });
                        }
                        ServerEvent::Stderr(message) | ServerEvent::Error(message) => {
                            events.push(DesktopEvent {
                                kind: "language_server_error".into(),
                                server_id: Some(application_id),
                                message: Some(message),
                                ..empty_event()
                            });
                        }
                        ServerEvent::Notification { .. } => {}
                    }
                }
                LspActorEvent::Failed(server_id, message) => events.push(DesktopEvent {
                    kind: "language_server_error".into(),
                    server_id: Some(server_id.get()),
                    message: Some(message),
                    ..empty_event()
                }),
                LspActorEvent::Stopped(server_id) => {
                    let id = server_id.get();
                    if self.active_language_servers.remove(&id).is_some() {
                        self.initialized_language_servers.remove(&id);
                        self.language_server_owners.remove(&id);
                        self.language_document_sync.remove(&id);
                        let _ = self.language_servers.stop_server(server_id);
                        events.push(DesktopEvent {
                            kind: "language_server_stopped".into(),
                            server_id: Some(id),
                            message: Some("Language server exited".into()),
                            ..empty_event()
                        });
                    }
                }
            }
        }
        for event in &events {
            if event.kind.starts_with("terminal_") {
                continue;
            }
            if let Ok(payload) = serde_json::to_string(event) {
                for runtime in self.lua_runtimes.values() {
                    let _ = runtime.dispatch_event(&event.kind, &payload);
                }
            }
        }
        let runtimes = self.lua_runtimes.values().cloned().collect::<Vec<_>>();
        for runtime in runtimes {
            if let Ok(edits) = runtime.take_edit_requests() {
                for edit in edits {
                    let _ = self.documents.apply_edits(
                        DocumentId(edit.document_id),
                        edit.expected_revision,
                        &[TextEdit {
                            range: edit.range,
                            new_text: edit.new_text,
                        }],
                    );
                }
            }
        }
        events
    }

    fn expire_terminal_approvals(&mut self) {
        let active_workspace = self.workspace.root().ok().map(Path::to_path_buf);
        let now = Instant::now();
        let expired = self
            .terminal_approvals
            .iter()
            .filter_map(|(id, pending)| {
                (pending.expires_at <= now || pending.workspace != active_workspace).then_some(*id)
            })
            .collect::<Vec<_>>();
        for id in expired {
            if let Some(pending) = self.terminal_approvals.remove(&id) {
                let _ = self.terminals.close_session(pending.session_id, false);
            }
        }
    }

    fn register_language_server(
        &mut self,
        extension_id: &str,
        configuration_id: &str,
        server_id: ServerId,
    ) {
        let id = server_id.get();
        if self.active_language_servers.contains_key(&id) {
            return;
        }
        self.active_language_servers.insert(id, server_id);
        self.language_server_owners
            .insert(id, (extension_id.into(), configuration_id.into()));
        self.language_document_sync
            .insert(id, LspDocumentSync::default());
        for document_id in self.open_documents.clone() {
            self.sync_document_open(document_id);
        }
    }

    fn language_server_contribution(
        &self,
        application_id: u64,
    ) -> Option<koren_extensions::LanguageServerContribution> {
        let (extension_id, configuration_id) = self.language_server_owners.get(&application_id)?;
        self.extensions
            .manifest(extension_id)
            .ok()?
            .0
            .language_servers
            .iter()
            .find(|server| server.id == *configuration_id)
            .cloned()
    }

    fn sync_document_open(&mut self, document_id: DocumentId) {
        let Ok(snapshot) = self.documents.snapshot(document_id) else {
            return;
        };
        let (Some(root), Some(relative)) =
            (self.workspace.root().ok(), snapshot.info.path.as_ref())
        else {
            return;
        };
        let path = root.join(relative);
        let suffix = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_lowercase();
        let uri = path_to_uri(&path);
        let servers = self
            .active_language_servers
            .iter()
            .map(|(id, server)| (*id, *server))
            .collect::<Vec<_>>();
        for (application_id, server_id) in servers {
            let Some(config) = self.language_server_contribution(application_id) else {
                continue;
            };
            if !config
                .file_extensions
                .iter()
                .any(|extension| extension.eq_ignore_ascii_case(&suffix))
            {
                continue;
            }
            let language_id = self
                .extensions
                .manifest(&self.language_server_owners[&application_id].0)
                .ok()
                .and_then(|(manifest, _)| {
                    manifest
                        .languages
                        .iter()
                        .find(|language| {
                            language
                                .extensions
                                .iter()
                                .any(|extension| extension.eq_ignore_ascii_case(&suffix))
                        })
                        .map(|language| language.id.clone())
                })
                .or_else(|| config.languages.first().cloned())
                .unwrap_or_default();
            let Some(sync) = self.language_document_sync.get_mut(&application_id) else {
                continue;
            };
            let Ok(notification) = sync.open(document_id, uri.clone(), &language_id, &snapshot)
            else {
                continue;
            };
            if let Ok(server) = self.language_servers.handle(server_id) {
                let _ = server.notify(notification);
            }
        }
    }

    fn sync_document_change(&mut self, document_id: DocumentId) {
        let Ok(snapshot) = self.documents.snapshot(document_id) else {
            return;
        };
        let servers = self
            .active_language_servers
            .iter()
            .map(|(id, server)| (*id, *server))
            .collect::<Vec<_>>();
        for (application_id, server_id) in servers {
            let Some(sync) = self.language_document_sync.get_mut(&application_id) else {
                continue;
            };
            let Ok(notification) = sync.change(document_id, &snapshot) else {
                continue;
            };
            if let Ok(server) = self.language_servers.handle(server_id) {
                let _ = server.notify(notification);
            }
        }
    }

    fn sync_document_close(&mut self, document_id: DocumentId) {
        let servers = self
            .active_language_servers
            .iter()
            .map(|(id, server)| (*id, *server))
            .collect::<Vec<_>>();
        for (application_id, server_id) in servers {
            let Some(sync) = self.language_document_sync.get_mut(&application_id) else {
                continue;
            };
            let Ok(notification) = sync.close(document_id) else {
                continue;
            };
            if let Ok(server) = self.language_servers.handle(server_id) {
                let _ = server.notify(notification);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum FeatureKind {
    Hover,
    Completion,
    Definition,
    References,
    Formatting,
    CodeActions,
    Rename,
}

struct LanguageFeatureParams {
    kind: FeatureKind,
    document_id: u64,
    expected_revision: u64,
    position: koren_protocol::TextPosition,
    include_declaration: bool,
    range: koren_protocol::TextRange,
    new_name: Option<String>,
    code_action_diagnostics: Vec<koren_protocol::LanguageDiagnosticContext>,
}

fn single_position_range(position: koren_protocol::TextPosition) -> koren_protocol::TextRange {
    koren_protocol::TextRange {
        start: position,
        end: position,
    }
}

fn run_language_feature(
    params: LanguageFeatureParams,
    targets: Vec<(u64, LspServerHandle, String, i32)>,
    cancelled: Arc<AtomicBool>,
) -> Result<ResponseValue, ProtocolError> {
    let LanguageFeatureParams {
        kind,
        document_id,
        expected_revision,
        position,
        include_declaration,
        range,
        new_name,
        code_action_diagnostics,
    } = params;
    let deadline = Instant::now() + LANGUAGE_FEATURE_TIMEOUT;
    let mut tickets = Vec::new();
    let mut aggregated_actions = Vec::new();
    let mut aggregated_references = Vec::new();
    for (_, server, uri, version) in targets {
        if cancelled.load(Ordering::Acquire) {
            return Err(not_found(format!("document '{document_id}' is closed")));
        }
        let action_uri = uri.clone();
        let request = match kind {
            FeatureKind::Hover => LspRequest::Hover {
                uri,
                version,
                position,
                timeout: LANGUAGE_FEATURE_TIMEOUT,
            },
            FeatureKind::Completion => LspRequest::Completion {
                uri,
                version,
                position,
                timeout: LANGUAGE_FEATURE_TIMEOUT,
            },
            FeatureKind::Definition => LspRequest::Definition {
                uri,
                version,
                position,
                timeout: LANGUAGE_FEATURE_TIMEOUT,
            },
            FeatureKind::References => LspRequest::References {
                uri,
                version,
                position,
                include_declaration,
                timeout: LANGUAGE_FEATURE_TIMEOUT,
            },
            FeatureKind::Formatting => LspRequest::Formatting {
                uri,
                version,
                options: FormattingOptions {
                    tab_size: 4,
                    insert_spaces: true,
                },
                timeout: LANGUAGE_FEATURE_TIMEOUT,
            },
            FeatureKind::CodeActions => LspRequest::CodeActions {
                uri,
                version,
                range,
                diagnostics: code_action_diagnostics
                    .iter()
                    .map(|diagnostic| {
                        serde_json::json!({
                            "range": diagnostic.range,
                            "severity": diagnostic.severity.filter(|value| (1..=4).contains(value)),
                            "code": diagnostic.code,
                            "source": diagnostic.source,
                            "message": diagnostic.message,
                        })
                    })
                    .collect(),
                only: if code_action_diagnostics.is_empty() {
                    Vec::new()
                } else {
                    vec!["quickfix".into()]
                },
                timeout: LANGUAGE_FEATURE_TIMEOUT,
            },
            FeatureKind::Rename => LspRequest::Rename {
                uri,
                version,
                position,
                new_name: new_name.clone().unwrap_or_default(),
                timeout: LANGUAGE_FEATURE_TIMEOUT,
            },
        };
        if let Ok(ticket) = server.request(request) {
            tickets.push((version, action_uri, Some(ticket)));
        }
    }

    while Instant::now() < deadline && tickets.iter().any(|(_, _, ticket)| ticket.is_some()) {
        if cancelled.load(Ordering::Acquire) {
            return Err(not_found(format!("document '{document_id}' is closed")));
        }
        for (version, action_uri, ticket) in &mut tickets {
            let Some(pending) = ticket.as_mut() else {
                continue;
            };
            match pending.try_recv() {
                Ok(Some(response)) => {
                    *ticket = None;
                    let result = match response {
                        LspResponse::Hover(result)
                            if matches!(kind, FeatureKind::Hover)
                                && result.is_current(*version) =>
                        {
                            let value = result.value;
                            Some(ResponseValue::LanguageHover {
                                document_revision: expected_revision,
                                contents: value.as_ref().map(|hover| hover.contents.clone()),
                                range: value.and_then(|hover| hover.range),
                            })
                        }
                        LspResponse::Completion(result)
                            if matches!(kind, FeatureKind::Completion)
                                && result.is_current(*version) =>
                        {
                            Some(ResponseValue::LanguageCompletions {
                                document_revision: expected_revision,
                                items: result
                                    .value
                                    .into_iter()
                                    .map(|item| koren_protocol::LanguageCompletionItem {
                                        label: item.label,
                                        detail: item.detail,
                                        insert_text: item.insert_text,
                                        documentation: item.documentation,
                                    })
                                    .collect(),
                            })
                        }
                        LspResponse::Definition(result)
                            if matches!(kind, FeatureKind::Definition)
                                && result.is_current(*version) =>
                        {
                            Some(ResponseValue::LanguageDefinitions {
                                document_revision: expected_revision,
                                locations: result
                                    .value
                                    .into_iter()
                                    .map(|location| koren_protocol::LanguageLocation {
                                        uri: location.uri,
                                        range: location.range,
                                    })
                                    .collect(),
                            })
                        }
                        LspResponse::References(result)
                            if matches!(kind, FeatureKind::References)
                                && result.is_current(*version) =>
                        {
                            Some(ResponseValue::LanguageReferences {
                                document_revision: expected_revision,
                                locations: result
                                    .value
                                    .into_iter()
                                    .map(|location| koren_protocol::LanguageLocation {
                                        uri: location.uri,
                                        range: location.range,
                                    })
                                    .collect(),
                            })
                        }
                        LspResponse::Formatting(result)
                            if matches!(kind, FeatureKind::Formatting)
                                && result.is_current(*version) =>
                        {
                            Some(ResponseValue::LanguageFormatting {
                                document_revision: expected_revision,
                                edits: result.value,
                            })
                        }
                        LspResponse::CodeActions(result)
                            if matches!(kind, FeatureKind::CodeActions)
                                && result.is_current(*version) =>
                        {
                            let actions = result.value.into_iter().map(|action| {
                                let unsupported_reason = if action.edit.as_ref().is_some_and(|edit| {
                                    !edit.unsupported_operations.is_empty()
                                        || edit.document_edits.len() != 1
                                        || edit.document_edits.first().is_none_or(|group| group.uri != *action_uri)
                                }) {
                                    Some("This action changes multiple files or uses unsupported filesystem operations.".to_owned())
                                } else if action.command.is_some() {
                                    Some("This action requires a provider command that Koren does not execute automatically.".to_owned())
                                } else if action.edit.is_none() {
                                    Some("This action does not contain a supported text edit.".to_owned())
                                } else { None };
                                let edits = if unsupported_reason.is_none() {
                                    action.edit.and_then(|edit| edit.document_edits.into_iter().next().map(|group| group.edits))
                                } else { None };
                                koren_protocol::LanguageCodeAction {
                                    id: action.id,
                                    title: action.title,
                                    provider_id: action.provider_id,
                                    kind: action.kind,
                                    related_diagnostics: action.related_diagnostics,
                                    edits,
                                    unsupported_reason,
                                    disabled_reason: action.disabled_reason,
                                }
                            }).collect();
                            Some(ResponseValue::LanguageCodeActions {
                                document_revision: expected_revision,
                                actions,
                            })
                        }
                        LspResponse::Rename(result)
                            if matches!(kind, FeatureKind::Rename)
                                && result.is_current(*version) =>
                        {
                            let edits = match result.value {
                                None => Vec::new(),
                                Some(edit)
                                    if edit.unsupported_operations.is_empty()
                                        && (edit.document_edits.is_empty()
                                            || (edit.document_edits.len() == 1
                                                && edit.document_edits[0].uri == *action_uri)) =>
                                {
                                    edit.document_edits
                                        .into_iter()
                                        .next()
                                        .map_or_else(Vec::new, |document| document.edits)
                                }
                                Some(_) => {
                                    return Err(invalid(
                                        "rename affects other files or uses unsupported filesystem operations; no edits were applied",
                                    ));
                                }
                            };
                            Some(ResponseValue::LanguageRename {
                                document_revision: expected_revision,
                                edits,
                            })
                        }
                        _ => None,
                    };
                    if let Some(result) = result {
                        match result {
                            ResponseValue::LanguageCodeActions { actions, .. }
                                if matches!(kind, FeatureKind::CodeActions) =>
                            {
                                aggregated_actions.extend(actions);
                            }
                            ResponseValue::LanguageReferences { locations, .. }
                                if matches!(kind, FeatureKind::References) =>
                            {
                                aggregated_references.extend(locations);
                            }
                            other => return Ok(other),
                        }
                    }
                }
                Ok(None) => {}
                Err(koren_lsp::LspError::UnsupportedFeature(_))
                | Err(koren_lsp::LspError::RequestTimeout(_))
                | Err(koren_lsp::LspError::RequestCancelled)
                | Err(koren_lsp::LspError::RequestQueueFull)
                | Err(koren_lsp::LspError::ServerNotReady(_))
                | Err(koren_lsp::LspError::ServerTerminated) => *ticket = None,
                Err(error) => {
                    *ticket = None;
                    return Err(lsp_error(error));
                }
            }
        }
        thread::park_timeout(Duration::from_millis(2));
    }
    if cancelled.load(Ordering::Acquire) {
        return Err(not_found(format!("document '{document_id}' is closed")));
    }
    if matches!(kind, FeatureKind::CodeActions) {
        return Ok(ResponseValue::LanguageCodeActions {
            document_revision: expected_revision,
            actions: aggregated_actions,
        });
    }
    if matches!(kind, FeatureKind::References) {
        aggregated_references.sort_by(|left, right| {
            left.uri
                .cmp(&right.uri)
                .then_with(|| left.range.start.line.cmp(&right.range.start.line))
                .then_with(|| left.range.start.character.cmp(&right.range.start.character))
        });
        aggregated_references.dedup();
        return Ok(ResponseValue::LanguageReferences {
            document_revision: expected_revision,
            locations: aggregated_references,
        });
    }
    Ok(match kind {
        FeatureKind::Hover => ResponseValue::LanguageHover {
            document_revision: expected_revision,
            contents: None,
            range: None,
        },
        FeatureKind::Completion => ResponseValue::LanguageCompletions {
            document_revision: expected_revision,
            items: Vec::new(),
        },
        FeatureKind::Definition => ResponseValue::LanguageDefinitions {
            document_revision: expected_revision,
            locations: Vec::new(),
        },
        FeatureKind::References => ResponseValue::LanguageReferences {
            document_revision: expected_revision,
            locations: Vec::new(),
        },
        FeatureKind::Formatting => ResponseValue::LanguageFormatting {
            document_revision: expected_revision,
            edits: Vec::new(),
        },
        FeatureKind::CodeActions => ResponseValue::LanguageCodeActions {
            document_revision: expected_revision,
            actions: Vec::new(),
        },
        FeatureKind::Rename => ResponseValue::LanguageRename {
            document_revision: expected_revision,
            edits: Vec::new(),
        },
    })
}

fn command_ping(arguments: &[String]) -> Result<String, CommandError> {
    if !arguments.is_empty() {
        return Err(CommandError::Execution(
            "core.ping takes no arguments".into(),
        ));
    }
    Ok("pong".into())
}

fn command_new_file(arguments: &[String]) -> Result<String, CommandError> {
    command_ui_action(arguments, "editor.new_file")
}
fn command_search(arguments: &[String]) -> Result<String, CommandError> {
    command_ui_action(arguments, "editor.search")
}
fn command_source_control(arguments: &[String]) -> Result<String, CommandError> {
    command_ui_action(arguments, "editor.source_control")
}
fn command_ui_action(arguments: &[String], action: &str) -> Result<String, CommandError> {
    if !arguments.is_empty() {
        return Err(CommandError::Execution(format!(
            "{action} takes no arguments"
        )));
    }
    Ok(action.into())
}

fn command_error(error: CommandError) -> ProtocolError {
    ProtocolError {
        code: match error {
            CommandError::UnknownCommand(_) => ErrorCode::NotFound,
            _ => ErrorCode::InvalidRequest,
        },
        message: error.to_string(),
    }
}

fn event_from_platform(event: PlatformEvent) -> DesktopEvent {
    match event {
        PlatformEvent::WorkspaceOpened { root } => DesktopEvent {
            kind: "workspace_opened".into(),
            root: Some(root.to_string_lossy().into_owned()),
            ..empty_event()
        },
        PlatformEvent::WorkspaceClosed { root } => DesktopEvent {
            kind: "workspace_closed".into(),
            root: Some(root.to_string_lossy().into_owned()),
            ..empty_event()
        },
        PlatformEvent::WorkspaceEntryChanged {
            operation,
            path,
            previous_path,
        } => DesktopEvent {
            kind: format!("workspace_entry_{operation}"),
            path: Some(path.to_string_lossy().into_owned()),
            previous_path: previous_path.map(|value| value.to_string_lossy().into_owned()),
            operation: Some(operation),
            ..empty_event()
        },
        PlatformEvent::DocumentOpened {
            document_id,
            revision,
            ..
        } => DesktopEvent {
            kind: "document_opened".into(),
            document_id: Some(document_id),
            revision: Some(revision),
            ..empty_event()
        },
        PlatformEvent::DocumentChanged {
            document_id,
            revision,
        } => DesktopEvent {
            kind: "document_changed".into(),
            document_id: Some(document_id),
            revision: Some(revision),
            ..empty_event()
        },
        PlatformEvent::DocumentSaved {
            document_id,
            revision,
        } => DesktopEvent {
            kind: "document_saved".into(),
            document_id: Some(document_id),
            revision: Some(revision),
            ..empty_event()
        },
        PlatformEvent::DocumentClosed { document_id } => DesktopEvent {
            kind: "document_closed".into(),
            document_id: Some(document_id),
            ..empty_event()
        },
        PlatformEvent::DocumentPathChanged { document_id, path } => DesktopEvent {
            kind: "document_path_changed".into(),
            document_id: Some(document_id),
            path: Some(path.to_string_lossy().into_owned()),
            ..empty_event()
        },
        PlatformEvent::GitStatusChanged { root } => DesktopEvent {
            kind: "git_status_changed".into(),
            root: Some(root.to_string_lossy().into_owned()),
            ..empty_event()
        },
        PlatformEvent::GitOperationCompleted {
            root,
            operation,
            commit,
        } => DesktopEvent {
            kind: "git_operation_completed".into(),
            root: Some(root.to_string_lossy().into_owned()),
            operation: Some(operation),
            commit,
            ..empty_event()
        },
        PlatformEvent::SaveConflictDetected {
            document_id,
            revision,
        } => DesktopEvent {
            kind: "save_conflict_detected".into(),
            document_id: Some(document_id),
            revision: Some(revision),
            ..empty_event()
        },
        PlatformEvent::ProjectRegistered { project_id } => DesktopEvent {
            kind: "project_registered".into(),
            project_id: Some(project_id),
            ..empty_event()
        },
        PlatformEvent::ProjectRemoved { project_id } => DesktopEvent {
            kind: "project_removed".into(),
            project_id: Some(project_id),
            ..empty_event()
        },
        PlatformEvent::TaskStarted { run_id } => DesktopEvent {
            kind: "task_started".into(),
            run_id: Some(run_id),
            ..empty_event()
        },
        PlatformEvent::TaskOutputAvailable { run_id } => DesktopEvent {
            kind: "task_output_available".into(),
            run_id: Some(run_id),
            ..empty_event()
        },
        PlatformEvent::TaskFinished { run_id } => DesktopEvent {
            kind: "task_finished".into(),
            run_id: Some(run_id),
            ..empty_event()
        },
        PlatformEvent::TaskCancelled { run_id } => DesktopEvent {
            kind: "task_cancelled".into(),
            run_id: Some(run_id),
            ..empty_event()
        },
        PlatformEvent::TaskFailed { run_id } => DesktopEvent {
            kind: "task_failed".into(),
            run_id: Some(run_id),
            ..empty_event()
        },
    }
}
fn empty_event() -> DesktopEvent {
    DesktopEvent {
        kind: String::new(),
        terminal_id: None,
        sequence: None,
        data_base64: None,
        terminal_state: None,
        exit_code: None,
        terminal_process_group_active: None,
        debug_id: None,
        debug_event: None,
        document_id: None,
        project_id: None,
        run_id: None,
        server_id: None,
        revision: None,
        root: None,
        path: None,
        previous_path: None,
        operation: None,
        commit: None,
        diagnostics: None,
        message: None,
    }
}

fn terminal_summary(info: &TerminalSessionInfo) -> TerminalSessionSummary {
    TerminalSessionSummary {
        session_id: info.id.get(),
        profile_id: info.profile_id.clone(),
        working_directory: info.working_directory.to_string_lossy().into_owned(),
        state: match info.state {
            koren_terminal::TerminalState::Created => TerminalLifecycle::Created,
            koren_terminal::TerminalState::Starting => TerminalLifecycle::Starting,
            koren_terminal::TerminalState::Running => TerminalLifecycle::Running,
            koren_terminal::TerminalState::Exited => TerminalLifecycle::Exited,
            koren_terminal::TerminalState::Failed => TerminalLifecycle::Failed,
            koren_terminal::TerminalState::Terminated => TerminalLifecycle::Terminated,
        },
        exit_code: info.exit_code,
        process_group_active: info.process_group_active,
        error: info.error.clone(),
        rows: info.rows,
        columns: info.columns,
    }
}

fn terminal_approval_summary(
    request_id: u64,
    details: &TerminalApprovalDetails,
    workspace: Option<&Path>,
) -> TerminalApprovalSummary {
    TerminalApprovalSummary {
        request_id,
        profile_id: details.profile_id.clone(),
        profile_name: details.profile_name.clone(),
        executable: details.executable.to_string_lossy().into_owned(),
        arguments: details.arguments.clone(),
        working_directory: details.working_directory.to_string_lossy().into_owned(),
        environment_keys: details.environment_keys.clone(),
        fallback_executable: details
            .fallback_executable
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        actor: "interactive_user".into(),
        workspace: workspace.map(|path| path.to_string_lossy().into_owned()),
    }
}

fn terminal_error(error: koren_terminal::TerminalError) -> ProtocolError {
    let code = match &error {
        koren_terminal::TerminalError::NotFound => ErrorCode::NotFound,
        koren_terminal::TerminalError::InvalidState(_)
        | koren_terminal::TerminalError::ActiveSession => ErrorCode::InvalidState,
        koren_terminal::TerminalError::Backpressure => ErrorCode::Busy,
        koren_terminal::TerminalError::Invalid(_) | koren_terminal::TerminalError::Io(_) => {
            ErrorCode::InvalidRequest
        }
    };
    ProtocolError {
        code,
        message: error.to_string(),
    }
}

fn debug_error(error: koren_debug::DebugError) -> ProtocolError {
    let code = match error {
        koren_debug::DebugError::NotFound => ErrorCode::NotFound,
        koren_debug::DebugError::AuthorizationDenied => ErrorCode::Unauthorized,
        koren_debug::DebugError::InvalidState(_) => ErrorCode::InvalidState,
        koren_debug::DebugError::Backpressure => ErrorCode::Busy,
        koren_debug::DebugError::InvalidConfiguration(_)
        | koren_debug::DebugError::Unsupported(_)
        | koren_debug::DebugError::StaleVariableReference => ErrorCode::InvalidRequest,
        koren_debug::DebugError::Transport(_) => ErrorCode::Internal,
    };
    ProtocolError {
        code,
        message: error.to_string(),
    }
}

fn required_thread_id(thread_id: Option<i64>) -> Result<i64, ProtocolError> {
    thread_id.ok_or_else(|| invalid("thread_id is required"))
}

fn qualify_debug_id(owner: &str, local_id: &str) -> String {
    if local_id.starts_with(&format!("{owner}.")) {
        local_id.to_owned()
    } else {
        format!("{owner}.{local_id}")
    }
}

fn resolve_workspace_template(
    value: &str,
    workspace: Option<&Path>,
) -> Result<PathBuf, ProtocolError> {
    if !value.contains("${workspaceFolder}") {
        return Ok(PathBuf::from(value));
    }
    let root = workspace.ok_or_else(|| invalid("open a workspace to use this debug profile"))?;
    Ok(PathBuf::from(
        value.replace("${workspaceFolder}", &root.to_string_lossy()),
    ))
}

fn debug_event_session_id(event: &DebugEvent) -> u64 {
    match event {
        DebugEvent::StateChanged { session_id, .. }
        | DebugEvent::Response { session_id, .. }
        | DebugEvent::Output { session_id, .. }
        | DebugEvent::AdapterDiagnostic { session_id, .. }
        | DebugEvent::BreakpointChanged { session_id, .. }
        | DebugEvent::AdapterEvent { session_id, .. }
        | DebugEvent::Error { session_id, .. }
        | DebugEvent::Closed { session_id } => *session_id,
    }
}

fn debug_approval_summary(
    approval_id: u64,
    adapter: &DebugAdapterDefinition,
    configuration: &DebugConfiguration,
    expires_at: Instant,
) -> DebugApprovalSummary {
    DebugApprovalSummary {
        approval_id,
        adapter_name: adapter.name.clone(),
        adapter_command: adapter.command.clone(),
        adapter_arguments: adapter.arguments.clone(),
        adapter_working_directory: adapter.working_directory.to_string_lossy().into_owned(),
        adapter_environment_keys: adapter.environment.keys().cloned().collect(),
        request: configuration.request,
        program: configuration
            .program
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        process_id: configuration.process_id,
        arguments: configuration.arguments.clone(),
        working_directory: configuration
            .working_directory
            .to_string_lossy()
            .into_owned(),
        environment_keys: configuration.environment.keys().cloned().collect(),
        expires_in_ms: expires_at
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(u128::from(u64::MAX)) as u64,
    }
}

fn protocol_directory_entry(entry: koren_workspace::WorkspaceEntry) -> DirectoryEntry {
    DirectoryEntry {
        name: entry.name.to_string_lossy().into_owned(),
        path: entry.path.to_string_lossy().into_owned(),
        kind: match entry.kind {
            EntryKind::File => DirectoryEntryKind::File,
            EntryKind::Directory => DirectoryEntryKind::Directory,
            EntryKind::Symlink => DirectoryEntryKind::Symlink,
            EntryKind::Other => DirectoryEntryKind::Other,
        },
        size_bytes: entry.size_bytes,
    }
}

fn project_summary(project: &ProjectInfo) -> Result<ProjectSummary, ProtocolError> {
    Ok(ProjectSummary {
        project_id: project.id.get(),
        display_name: project.display_name.clone(),
        root: project.root.to_string_lossy().into_owned(),
        provider_id: project.provider_id.as_str().into(),
        configuration_path: project
            .configuration_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        state: ProjectLifecycle::Registered,
    })
}
fn cargo_summary(
    project: &ProjectInfo,
    metadata: &koren_cargo::CargoProjectMetadata,
) -> Result<CargoProjectSummary, ProtocolError> {
    let package = metadata.package.as_ref().map(package_summary);
    let members = metadata.members.iter().map(package_summary).collect();
    Ok(CargoProjectSummary {
        project: project_summary(project)?,
        is_workspace: metadata.is_workspace,
        package,
        members,
    })
}
fn package_summary(package: &koren_cargo::CargoPackageMetadata) -> CargoPackageSummary {
    CargoPackageSummary {
        name: package.name.clone(),
        version: package.version.clone(),
        manifest_path: package.manifest_path.to_string_lossy().into_owned(),
        targets: package
            .targets
            .iter()
            .map(|target| CargoTargetSummary {
                name: target.name.clone(),
                kind: format!("{:?}", target.kind).to_lowercase(),
                source_path: target
                    .source_path
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
            })
            .collect(),
    }
}
fn task_summary(task_id: u64, task: &TaskDefinition) -> TaskSummary {
    TaskSummary {
        task_id,
        key: task.key.clone(),
        display_name: task.display_name.clone(),
        category: format!("{:?}", task.category).to_lowercase(),
        executable: task.executable.to_string_lossy().into_owned(),
        arguments: task.arguments.clone(),
        working_directory: task.working_directory.to_string_lossy().into_owned(),
        timeout_ms: task
            .timeout
            .map(|value| u64::try_from(value.as_millis()).unwrap_or(u64::MAX)),
        project_id: task.project_id,
    }
}
fn approval_summary(
    request: &TaskExecutionRequest,
    approvals: &TaskApprovalManager,
) -> Result<ExecutionApprovalSummary, ProtocolError> {
    let snapshot = &request.snapshot;
    let expires_in_ms = approvals
        .remaining_lifetime(request.id)
        .map(|value| u64::try_from(value.as_millis()).unwrap_or(u64::MAX))
        .map_err(task_error)?;
    Ok(ExecutionApprovalSummary {
        request_id: request.id.get(),
        task_id: snapshot.task_id.get(),
        task_key: snapshot.definition.key.clone(),
        display_name: snapshot.definition.display_name.clone(),
        executable: snapshot
            .definition
            .executable
            .to_string_lossy()
            .into_owned(),
        arguments: snapshot.definition.arguments.clone(),
        working_directory: snapshot
            .definition
            .working_directory
            .to_string_lossy()
            .into_owned(),
        environment_keys: snapshot.definition.environment.keys().cloned().collect(),
        project_id: snapshot.definition.project_id,
        workspace_root: snapshot.workspace_root.to_string_lossy().into_owned(),
        requesting_actor: match &snapshot.caller {
            TaskCaller::InteractiveUser => "interactive_user".into(),
            TaskCaller::Extension(id) => format!("extension:{id}"),
            TaskCaller::CodingAgent(id) => format!("agent:{id}"),
        },
        expires_in_ms,
    })
}
fn task_response(snapshot: TaskSnapshot) -> ResponseValue {
    let (state, exit_code, error) = match snapshot.state {
        TaskState::Pending => (TaskLifecycle::Pending, None, None),
        TaskState::Running => (TaskLifecycle::Running, None, None),
        TaskState::Succeeded { exit_code } => (TaskLifecycle::Succeeded, Some(exit_code), None),
        TaskState::Failed { exit_code } => (TaskLifecycle::Failed, exit_code, None),
        TaskState::Cancelled => (TaskLifecycle::Cancelled, None, None),
        TaskState::TimedOut => (TaskLifecycle::TimedOut, None, None),
        TaskState::SpawnFailed { message } => (TaskLifecycle::SpawnFailed, None, Some(message)),
    };
    ResponseValue::TaskSnapshot {
        run_id: snapshot.run_id.get(),
        task_id: snapshot.task_id.get(),
        state,
        exit_code,
        error,
        stdout: snapshot.stdout,
        stderr: snapshot.stderr,
        output_truncated: snapshot.output_truncated,
    }
}
fn root_string(workspace: &WorkspaceManager) -> Result<String, ProtocolError> {
    Ok(workspace
        .root()
        .map_err(workspace_error)?
        .display()
        .to_string())
}
fn invalid(message: impl Into<String>) -> ProtocolError {
    ProtocolError {
        code: ErrorCode::InvalidRequest,
        message: message.into(),
    }
}
fn workspace_error(error: koren_workspace::WorkspaceError) -> ProtocolError {
    ProtocolError {
        code: ErrorCode::InvalidPath,
        message: error.to_string(),
    }
}
fn git_error(error: koren_git::GitError) -> ProtocolError {
    let code = match error {
        koren_git::GitError::InvalidPath => ErrorCode::InvalidPath,
        koren_git::GitError::NotFound => ErrorCode::NotFound,
        koren_git::GitError::Conflict(_) => ErrorCode::Conflict,
        koren_git::GitError::Git(_)
        | koren_git::GitError::DiffTooLarge
        | koren_git::GitError::EmptyCommitMessage
        | koren_git::GitError::NothingToCommit
        | koren_git::GitError::RepositoryLimit => ErrorCode::InvalidRequest,
    };
    ProtocolError {
        code,
        message: error.to_string(),
    }
}
fn search_error(error: koren_search::SearchError) -> ProtocolError {
    ProtocolError {
        code: ErrorCode::InvalidRequest,
        message: error.to_string(),
    }
}
fn document_error(error: koren_documents::DocumentError) -> ProtocolError {
    let code = match error {
        koren_documents::DocumentError::Conflict(_)
        | koren_documents::DocumentError::ConflictChangedAgain(_)
        | koren_documents::DocumentError::StaleRevision { .. }
        | koren_documents::DocumentError::UnsavedChanges(_)
        | koren_documents::DocumentError::PathAlreadyOpen(_) => ErrorCode::Conflict,
        koren_documents::DocumentError::NotOpen(_) => ErrorCode::NotOpen,
        koren_documents::DocumentError::Workspace(_) => ErrorCode::InvalidPath,
        _ => ErrorCode::InvalidRequest,
    };
    ProtocolError {
        code,
        message: error.to_string(),
    }
}
fn project_error(error: koren_projects::ProjectError) -> ProtocolError {
    ProtocolError {
        code: ErrorCode::InvalidRequest,
        message: error.to_string(),
    }
}
fn task_error(error: koren_tasks::TaskError) -> ProtocolError {
    let code = match error {
        koren_tasks::TaskError::NotFound | koren_tasks::TaskError::ApprovalNotFound => {
            ErrorCode::NotFound
        }
        koren_tasks::TaskError::ApprovalExpired | koren_tasks::TaskError::ApprovalInvalidated => {
            ErrorCode::NotOpen
        }
        koren_tasks::TaskError::Unauthorized(_) => ErrorCode::Unauthorized,
        _ => ErrorCode::InvalidRequest,
    };
    ProtocolError {
        code,
        message: error.to_string(),
    }
}

fn extension_error(error: koren_extensions::ExtensionError) -> ProtocolError {
    let code = match &error {
        koren_extensions::ExtensionError::NotFound(_) => ErrorCode::NotFound,
        _ => ErrorCode::InvalidRequest,
    };
    ProtocolError {
        code,
        message: error.to_string(),
    }
}

fn lua_error(error: koren_lua::LuaRuntimeError) -> ProtocolError {
    ProtocolError {
        code: ErrorCode::InvalidRequest,
        message: error.to_string(),
    }
}

fn lsp_error(error: koren_lsp::LspError) -> ProtocolError {
    let code = if matches!(&error, koren_lsp::LspError::AuthorizationDenied(_)) {
        ErrorCode::Unauthorized
    } else if matches!(&error, koren_lsp::LspError::ServerNotFound(_)) {
        ErrorCode::NotFound
    } else {
        ErrorCode::InvalidRequest
    };
    ProtocolError {
        code,
        message: error.to_string(),
    }
}

fn inspection_error(error: koren_inspections::InspectionError) -> ProtocolError {
    ProtocolError {
        code: match error {
            koren_inspections::InspectionError::ConcurrencyLimit
            | koren_inspections::InspectionError::Cancelled
            | koren_inspections::InspectionError::RequestTimeout => ErrorCode::Conflict,
            _ => ErrorCode::InvalidRequest,
        },
        message: error.to_string(),
    }
}

fn inspection_finding(
    finding: koren_inspections::Finding,
    active_path: Option<&Path>,
    expected_revision: Option<u64>,
) -> InspectionFinding {
    let active_path = active_path
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let actions = finding
        .actions
        .into_iter()
        .map(|action| {
            let supported = action.command.is_none()
                && action.edits.len() == 1
                && action.edits.first().is_some_and(|edit| {
                    edit.path == active_path
                        && edit.expected_revision == expected_revision
                });
            let edits = if supported {
                action
                    .edits
                    .into_iter()
                    .map(|edit| InspectionProposedTextEdit {
                        path: edit.path,
                        expected_revision: edit.expected_revision,
                        range: edit.range,
                        new_text: edit.new_text,
                    })
                    .collect()
            } else {
                Vec::new()
            };
            InspectionCodeAction {
                id: action.id,
                title: action.title,
                provider_id: action.provider_id,
                kind: action.kind,
                diagnostic_ids: action.diagnostic_ids,
                edits,
                disabled_reason: action.disabled_reason.or_else(|| {
                    (!supported).then(|| {
                        "This action is stale, affects another file, or uses an unsupported operation.".into()
                    })
                }),
            }
        })
        .collect();
    let (origin, source_id) = match finding.origin {
        FindingOrigin::Document => ("document".into(), None),
        FindingOrigin::ProjectSnapshot { snapshot_id } => {
            ("project_snapshot".into(), Some(snapshot_id))
        }
        FindingOrigin::Imported { report_id, .. } => ("imported".into(), Some(report_id)),
    };
    InspectionFinding {
        id: finding.id,
        provider_id: finding.provider_id,
        rule_id: finding.rule_id,
        path: finding.path,
        document_revision: finding.document_revision,
        range: finding.range,
        severity: match finding.severity {
            koren_inspections::Severity::Error => "error",
            koren_inspections::Severity::Warning => "warning",
            koren_inspections::Severity::Information => "information",
            koren_inspections::Severity::Hint => "hint",
        }
        .into(),
        message: finding.message,
        explanation: finding.explanation,
        category: finding.category,
        related: finding
            .related
            .into_iter()
            .map(|location| InspectionRelatedLocation {
                path: location.path,
                range: location.range,
                message: location.message,
            })
            .collect(),
        actions,
        origin,
        source_id,
        source_revision: finding.source_revision,
    }
}

fn not_found(message: String) -> ProtocolError {
    ProtocolError {
        code: ErrorCode::NotFound,
        message,
    }
}

fn protocol_extension(extension: koren_extensions::ExtensionSummary) -> ProtocolExtensionSummary {
    ProtocolExtensionSummary {
        id: extension.id,
        name: extension.name,
        publisher: extension.publisher,
        version: extension.version,
        description: extension.description,
        enabled: extension.enabled,
        state: match extension.state {
            ExtensionState::Disabled => "disabled",
            ExtensionState::Installed => "installed",
            ExtensionState::Activating => "activating",
            ExtensionState::Active => "active",
            ExtensionState::Failed => "failed",
        }
        .into(),
        error: extension.error,
    }
}

fn protocol_ui_document(view: &UiDocument) -> ExtensionUiDocument {
    ExtensionUiDocument {
        id: view.id.clone(),
        elements: view
            .elements
            .iter()
            .map(|element| match element {
                UiElement::Text { id, text } => ExtensionUiElement::Text {
                    id: id.clone(),
                    text: text.clone(),
                },
                UiElement::Heading { id, text } => ExtensionUiElement::Heading {
                    id: id.clone(),
                    text: text.clone(),
                },
                UiElement::Button {
                    id,
                    label,
                    command_id,
                } => ExtensionUiElement::Button {
                    id: id.clone(),
                    label: label.clone(),
                    command_id: command_id.clone(),
                },
                UiElement::List { id, items } => ExtensionUiElement::List {
                    id: id.clone(),
                    items: items.clone(),
                },
                UiElement::Table { id, rows } => ExtensionUiElement::Table {
                    id: id.clone(),
                    rows: rows.clone(),
                },
                UiElement::Input {
                    id,
                    label,
                    value,
                    required,
                    max_length,
                } => ExtensionUiElement::Input {
                    id: id.clone(),
                    label: label.clone(),
                    value: value.clone(),
                    required: *required,
                    max_length: *max_length,
                },
                UiElement::Select {
                    id,
                    label,
                    options,
                    value,
                } => ExtensionUiElement::Select {
                    id: id.clone(),
                    label: label.clone(),
                    options: options.clone(),
                    value: value.clone(),
                },
                UiElement::Checkbox { id, label, checked } => ExtensionUiElement::Checkbox {
                    id: id.clone(),
                    label: label.clone(),
                    checked: *checked,
                },
                UiElement::Status { id, label, tone } => ExtensionUiElement::Status {
                    id: id.clone(),
                    label: label.clone(),
                    tone: format!("{tone:?}").to_lowercase(),
                },
            })
            .collect(),
    }
}

fn protocol_setting_kind(kind: &SettingKind) -> (ExtensionSettingKind, ExtensionSettingValue) {
    match kind {
        SettingKind::Boolean { default } => (
            ExtensionSettingKind::Boolean { default: *default },
            ExtensionSettingValue::Boolean(*default),
        ),
        SettingKind::Integer { default, min, max } => (
            ExtensionSettingKind::Integer {
                default: *default,
                min: *min,
                max: *max,
            },
            ExtensionSettingValue::Integer(*default),
        ),
        SettingKind::Text {
            default,
            max_length,
        } => (
            ExtensionSettingKind::Text {
                default: default.clone(),
                max_length: *max_length,
            },
            ExtensionSettingValue::Text(default.clone()),
        ),
        SettingKind::Choice { default, options } => (
            ExtensionSettingKind::Choice {
                default: default.clone(),
                options: options.clone(),
            },
            ExtensionSettingValue::Text(default.clone()),
        ),
    }
}

fn valid_setting_value(kind: &SettingKind, value: &ExtensionSettingValue) -> bool {
    match (kind, value) {
        (SettingKind::Boolean { .. }, ExtensionSettingValue::Boolean(_)) => true,
        (SettingKind::Integer { min, max, .. }, ExtensionSettingValue::Integer(value)) => {
            value >= min && value <= max
        }
        (SettingKind::Text { max_length, .. }, ExtensionSettingValue::Text(value)) => {
            value.len() <= *max_length
        }
        (SettingKind::Choice { options, .. }, ExtensionSettingValue::Text(value)) => {
            options.contains(value)
        }
        _ => false,
    }
}

fn extension_settings_path() -> Option<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"))
    } else if let Some(home) = std::env::var_os("XDG_CONFIG_HOME") {
        Some(PathBuf::from(home))
    } else {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
    }?;
    base.is_absolute()
        .then(|| base.join("koren/extension-settings.json"))
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct PersistedDesktopSettings {
    #[serde(default)]
    extension_settings: BTreeMap<String, ExtensionSettingValue>,
    #[serde(default)]
    inspection_profiles: InspectionProfiles,
}

fn load_desktop_settings(path: Option<&Path>) -> PersistedDesktopSettings {
    let Some(bytes) = path.and_then(|path| std::fs::read(path).ok()) else {
        return PersistedDesktopSettings::default();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return PersistedDesktopSettings::default();
    };
    parse_desktop_settings(value)
}

fn parse_desktop_settings(value: serde_json::Value) -> PersistedDesktopSettings {
    if value.get("extension_settings").is_some() || value.get("inspection_profiles").is_some() {
        serde_json::from_value(value).unwrap_or_default()
    } else {
        PersistedDesktopSettings {
            extension_settings: serde_json::from_value(value).unwrap_or_default(),
            inspection_profiles: InspectionProfiles::default(),
        }
    }
}

fn persist_desktop_settings(
    path: Option<&Path>,
    extension_settings: &BTreeMap<String, ExtensionSettingValue>,
    inspection_profiles: &InspectionProfiles,
) -> std::io::Result<()> {
    let Some(path) = path else {
        return Err(std::io::Error::other(
            "no user configuration directory is available",
        ));
    };
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("settings path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let temp_path = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(&PersistedDesktopSettings {
        extension_settings: extension_settings.clone(),
        inspection_profiles: inspection_profiles.clone(),
    })
    .map_err(std::io::Error::other)?;
    std::fs::write(&temp_path, bytes)?;
    std::fs::rename(temp_path, path)
}

fn settings_error(error: std::io::Error) -> ProtocolError {
    ProtocolError {
        code: ErrorCode::Internal,
        message: format!("could not save editor settings: {error}"),
    }
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Information => "information",
        Severity::Hint => "hint",
    }
}

fn parse_severity_name(value: &str) -> Result<Severity, ()> {
    match value {
        "error" => Ok(Severity::Error),
        "warning" => Ok(Severity::Warning),
        "information" => Ok(Severity::Information),
        "hint" => Ok(Severity::Hint),
        _ => Err(()),
    }
}

fn extension_install_root() -> Option<PathBuf> {
    let path = if let Some(path) = std::env::var_os("KOREN_EXTENSION_DIR") {
        Some(PathBuf::from(path))
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(|value| PathBuf::from(value).join("Koren/extensions"))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(|value| PathBuf::from(value).join("Library/Application Support/Koren/extensions"))
    } else if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        Some(PathBuf::from(data_home).join("koren/extensions"))
    } else {
        std::env::var_os("HOME")
            .map(|value| PathBuf::from(value).join(".local/share/koren/extensions"))
    };
    path.filter(|path| path.is_absolute())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct Temp(PathBuf);

    struct TestInspectionProvider;

    impl InspectionProvider for TestInspectionProvider {
        fn id(&self) -> &str {
            "org.example.rules"
        }

        fn rules(&self) -> Vec<koren_inspections::RuleMetadata> {
            vec![koren_inspections::RuleMetadata {
                id: "org.example.rules.todo".into(),
                provider_id: self.id().into(),
                title: "TODO marker".into(),
                description: "Find TODO markers".into(),
                category: "maintenance".into(),
                languages: ["plaintext".into()].into_iter().collect(),
                default_severity: Severity::Warning,
                supports_fixes: false,
                options: BTreeMap::new(),
            }]
        }

        fn inspect(
            &self,
            document: &koren_inspections::InspectionDocument,
            _profile: &InspectionProfile,
            _workspace: Option<&str>,
            _cancellation: &CancellationToken,
        ) -> Result<Vec<koren_inspections::Finding>, String> {
            Ok(vec![koren_inspections::Finding {
                id: format!("{}:fixture", document.path),
                provider_id: self.id().into(),
                rule_id: "org.example.rules.todo".into(),
                path: document.path.clone(),
                document_id: document.document_id,
                document_revision: document.revision,
                source_revision: None,
                range: koren_protocol::TextRange {
                    start: koren_protocol::TextPosition {
                        line: 0,
                        character: 0,
                    },
                    end: koren_protocol::TextPosition {
                        line: 0,
                        character: 0,
                    },
                },
                severity: Severity::Warning,
                message: "fixture finding".into(),
                explanation: None,
                category: Some("maintenance".into()),
                related: Vec::new(),
                actions: Vec::new(),
                documentation: None,
                origin: FindingOrigin::Document,
            }])
        }
    }

    struct SlowInspectionProvider;

    impl InspectionProvider for SlowInspectionProvider {
        fn id(&self) -> &str {
            "org.example.slow"
        }
        fn rules(&self) -> Vec<koren_inspections::RuleMetadata> {
            vec![koren_inspections::RuleMetadata {
                id: "org.example.slow.wait".into(),
                provider_id: self.id().into(),
                title: "Slow rule".into(),
                description: "Cancellation fixture".into(),
                category: "test".into(),
                languages: BTreeSet::new(),
                default_severity: Severity::Information,
                supports_fixes: false,
                options: BTreeMap::new(),
            }]
        }
        fn inspect(
            &self,
            _document: &koren_inspections::InspectionDocument,
            _profile: &InspectionProfile,
            _workspace: Option<&str>,
            cancellation: &CancellationToken,
        ) -> Result<Vec<koren_inspections::Finding>, String> {
            for _ in 0..100 {
                cancellation
                    .checkpoint()
                    .map_err(|error| error.to_string())?;
                thread::sleep(Duration::from_millis(5));
            }
            Ok(Vec::new())
        }
    }

    #[test]
    fn desktop_settings_migrate_legacy_values_and_round_trip_profiles() {
        let legacy = serde_json::json!({
            "org.example.sample.enabled": true,
            "org.example.sample.count": 3
        });
        let migrated = parse_desktop_settings(legacy);
        assert_eq!(migrated.extension_settings.len(), 2);
        assert!(migrated.inspection_profiles.profiles.is_empty());

        let mut profiles = InspectionProfiles::default();
        profiles.insert(InspectionProfile::new("Default"));
        profiles.insert(InspectionProfile::new("Strict"));
        profiles.select(None, "Strict").unwrap();
        let stored = PersistedDesktopSettings {
            extension_settings: migrated.extension_settings,
            inspection_profiles: profiles,
        };
        let reloaded = parse_desktop_settings(serde_json::to_value(&stored).unwrap());
        assert_eq!(reloaded.extension_settings, stored.extension_settings);
        assert_eq!(reloaded.inspection_profiles, stored.inspection_profiles);
    }

    #[test]
    fn terminal_request_requires_approval_and_denial_never_spawns_shell() {
        let workspace = Temp::new();
        let mut backend = DesktopBackend::new().unwrap();
        backend
            .method(RequestMethod::OpenWorkspace {
                path: workspace.0.to_string_lossy().into_owned(),
            })
            .unwrap();
        let requested = backend.method(RequestMethod::RequestTerminalStart).unwrap();
        let approval = match requested {
            ResponseValue::TerminalApprovalRequired(approval) => approval,
            other => panic!("unexpected response {other:?}"),
        };
        assert_eq!(approval.actor, "interactive_user");
        assert_eq!(
            approval.workspace.as_deref(),
            Some(workspace.0.to_string_lossy().as_ref())
        );
        assert!(!approval.executable.is_empty());
        assert!(
            matches!(backend.method(RequestMethod::ListTerminalSessions).unwrap(), ResponseValue::TerminalSessions(sessions) if sessions.len() == 1 && sessions[0].state == TerminalLifecycle::Created)
        );
        assert!(matches!(
            backend
                .method(RequestMethod::DenyTerminalStart {
                    request_id: approval.request_id
                })
                .unwrap(),
            ResponseValue::TerminalDenied { .. }
        ));
        assert!(
            matches!(backend.method(RequestMethod::ListTerminalSessions).unwrap(), ResponseValue::TerminalSessions(sessions) if sessions.is_empty())
        );
        backend.shutdown();
    }

    #[test]
    fn debug_configuration_is_lazy_and_denial_never_starts_an_adapter() {
        let workspace = Temp::new();
        let mut backend = DesktopBackend::new().unwrap();
        backend
            .method(RequestMethod::OpenWorkspace {
                path: workspace.0.to_string_lossy().into_owned(),
            })
            .unwrap();
        let adapter_script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/koren-debug/tests/fake_adapter.py");
        let mut environment = BTreeMap::new();
        environment.insert("DEBUG_SECRET".into(), "never-show-this".into());
        backend
            .method(RequestMethod::RegisterDebugConfiguration {
                adapter: DebugAdapterDefinition {
                    id: "fake-debug".into(),
                    name: "Fake adapter".into(),
                    command: if cfg!(windows) { "python" } else { "python3" }.into(),
                    arguments: vec![adapter_script.to_string_lossy().into_owned()],
                    working_directory: workspace.0.clone(),
                    environment: environment.clone(),
                },
                configuration: DebugConfiguration {
                    id: "fake-debug.launch".into(),
                    name: "Launch fixture".into(),
                    adapter_id: "fake-debug".into(),
                    request: DebugRequestType::Launch,
                    program: Some(workspace.0.join("fixture")),
                    process_id: None,
                    arguments: Vec::new(),
                    working_directory: workspace.0.clone(),
                    environment,
                    adapter_settings: serde_json::Map::new(),
                    stop_on_entry: true,
                },
            })
            .unwrap();
        assert!(matches!(
            backend.method(RequestMethod::ListDebugSessions).unwrap(),
            ResponseValue::DebugSessions(sessions) if sessions.is_empty()
        ));
        let approval = match backend
            .method(RequestMethod::RequestDebugStart {
                configuration_id: "fake-debug.launch".into(),
            })
            .unwrap()
        {
            ResponseValue::DebugApprovalRequired(approval) => approval,
            other => panic!("unexpected response {other:?}"),
        };
        let rendered = serde_json::to_string(&approval).unwrap();
        assert!(rendered.contains("DEBUG_SECRET"));
        assert!(!rendered.contains("never-show-this"));
        backend
            .method(RequestMethod::DenyDebugStart {
                approval_id: approval.approval_id,
            })
            .unwrap();
        assert!(matches!(
            backend.method(RequestMethod::ListDebugSessions).unwrap(),
            ResponseValue::DebugSessions(sessions) if sessions.is_empty()
        ));
        backend.shutdown();
    }

    #[test]
    fn extension_profile_approval_is_invalidated_when_owner_is_disabled() {
        let temp = Temp::new();
        let workspace = temp.0.join("workspace");
        let extension_root = temp.0.join("extensions");
        let extension = extension_root.join("org.example.debug");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&extension).unwrap();
        let adapter_script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/koren-debug/tests/fake_adapter.py");
        fs::write(extension.join("main.lua"), "return {}\n").unwrap();
        fs::write(
            extension.join("koren-extension.toml"),
            format!(
                "schema_version=1\nid='org.example.debug'\nname='Debug fixture'\npublisher='Koren tests'\nversion='0.1.0'\ndescription='Debug lifecycle test'\nentry_point='main.lua'\napi_version='1.0.0'\ncapabilities=['debug_adapters']\n\n[[debug_adapters]]\nid='fake'\nname='Fake DAP'\ncommand='python3'\narguments=['{}']\nworking_directory='${{workspaceFolder}}'\n\n[[debug_configurations]]\nid='launch'\nname='Launch fixture'\nadapter_id='fake'\nrequest='launch'\nprogram='${{workspaceFolder}}/fixture'\nworking_directory='${{workspaceFolder}}'\n",
                adapter_script.display()
            ),
        )
        .unwrap();
        let mut backend = DesktopBackend::new().unwrap();
        backend.extension_root = Some(extension_root);
        backend
            .method(RequestMethod::OpenWorkspace {
                path: workspace.to_string_lossy().into_owned(),
            })
            .unwrap();
        backend.method(RequestMethod::ListExtensions).unwrap();
        backend
            .method(RequestMethod::SetExtensionEnabled {
                extension_id: "org.example.debug".into(),
                enabled: true,
            })
            .unwrap();
        backend
            .method(RequestMethod::ActivateExtension {
                extension_id: "org.example.debug".into(),
            })
            .unwrap();
        let approval = match backend
            .method(RequestMethod::RequestDebugStart {
                configuration_id: "org.example.debug.launch".into(),
            })
            .unwrap()
        {
            ResponseValue::DebugApprovalRequired(approval) => approval,
            other => panic!("unexpected response {other:?}"),
        };
        backend
            .method(RequestMethod::SetExtensionEnabled {
                extension_id: "org.example.debug".into(),
                enabled: false,
            })
            .unwrap();
        let result = backend.method(RequestMethod::ApproveDebugStart {
            approval_id: approval.approval_id,
        });
        assert!(matches!(
            result,
            Err(ProtocolError {
                code: ErrorCode::Conflict,
                ..
            })
        ));
        assert!(matches!(
            backend.method(RequestMethod::ListDebugSessions).unwrap(),
            ResponseValue::DebugSessions(sessions) if sessions.is_empty()
        ));
        assert!(
            backend
                .debugger
                .configurations()
                .iter()
                .all(|profile| { profile.id != "org.example.debug.launch" })
        );
        backend.shutdown();
    }

    #[test]
    fn inspection_severity_setting_rejects_unknown_names() {
        assert_eq!(parse_severity_name("error"), Ok(Severity::Error));
        assert_eq!(parse_severity_name("hint"), Ok(Severity::Hint));
        assert_eq!(parse_severity_name("critical"), Err(()));
    }

    #[test]
    fn desktop_profile_api_applies_workspace_overrides_and_persists_them() {
        let temp = Temp::new();
        let config_path = temp.0.join("settings.json");
        let workspace = temp.0.join("workspace");
        fs::create_dir(&workspace).unwrap();
        let mut backend = DesktopBackend::new().unwrap();
        backend.settings_path = Some(config_path.clone());
        backend
            .inspection_profiles
            .insert(InspectionProfile::new("Strict"));
        backend
            .inspections
            .register(Arc::new(TestInspectionProvider))
            .unwrap();
        backend
            .method(RequestMethod::OpenWorkspace {
                path: workspace.to_string_lossy().into_owned(),
            })
            .unwrap();
        assert!(matches!(
            backend
                .method(RequestMethod::SelectInspectionProfile {
                    profile_name: "Strict".into(),
                    workspace_specific: true,
                })
                .unwrap(),
            ResponseValue::InspectionConfiguration(config)
                if config.selected_profile == "Strict" && config.workspace_specific
        ));
        let configuration = backend
            .method(RequestMethod::SetInspectionRule {
                rule_id: "org.example.rules.todo".into(),
                enabled: false,
                severity: Some("error".into()),
                workspace_specific: true,
                reset: false,
            })
            .unwrap();
        assert!(matches!(configuration,
            ResponseValue::InspectionConfiguration(config)
                if config.rules.len() == 1 && !config.rules[0].enabled &&
                   config.rules[0].severity == "error" && config.rules[0].workspace_override
        ));
        let persisted = load_desktop_settings(Some(&config_path));
        assert_eq!(
            persisted.inspection_profiles.selected,
            Some("Default".into())
        );
        assert_eq!(
            persisted
                .inspection_profiles
                .workspace_selection
                .get(&workspace.to_string_lossy().to_string()),
            Some(&"Strict".into())
        );
        let profile = persisted
            .inspection_profiles
            .profiles
            .get("Strict")
            .unwrap();
        let rule = profile
            .workspace_overrides
            .get(&workspace.to_string_lossy().to_string())
            .unwrap()
            .get("org.example.rules.todo")
            .unwrap();
        assert!(!rule.enabled);
        assert_eq!(rule.severity, Some(Severity::Error));
        backend.shutdown();
    }

    #[test]
    fn desktop_profile_api_creates_and_selects_workspace_profile() {
        let temp = Temp::new();
        let config_path = temp.0.join("settings.json");
        let workspace = temp.0.join("workspace");
        fs::create_dir(&workspace).unwrap();
        let mut backend = DesktopBackend::new().unwrap();
        backend.settings_path = Some(config_path.clone());
        backend
            .method(RequestMethod::OpenWorkspace {
                path: workspace.to_string_lossy().into_owned(),
            })
            .unwrap();
        assert!(matches!(
            backend
                .method(RequestMethod::CreateInspectionProfile {
                    profile_name: "Team Strict".into(),
                    workspace_specific: true,
                })
                .unwrap(),
            ResponseValue::InspectionConfiguration(config)
                if config.selected_profile == "Team Strict" && config.workspace_specific
        ));
        let persisted = load_desktop_settings(Some(&config_path));
        assert!(
            persisted
                .inspection_profiles
                .profiles
                .contains_key("Team Strict")
        );
        assert!(
            persisted
                .inspection_profiles
                .workspace_selection
                .contains_key(
                    &workspace
                        .canonicalize()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                )
        );
    }

    #[test]
    fn sarif_desktop_commands_import_validated_reports_and_export_fresh_results() {
        let temp = Temp::new();
        let report_path = temp.0.join("input.sarif");
        fs::write(
            &report_path,
            r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"CI"}},"results":[{"ruleId":"SAMPLE1","level":"warning","message":{"text":"Review this line"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"src/main.rs"},"region":{"startLine":2,"startColumn":1,"endLine":2,"endColumn":3}}}]}]}]}"#,
        )
        .unwrap();
        let mut backend = DesktopBackend::new().unwrap();
        let imported = backend
            .method(RequestMethod::ImportSarif {
                path: report_path.to_string_lossy().into_owned(),
            })
            .unwrap();
        assert!(matches!(imported,
            ResponseValue::ImportedSarif { findings, .. }
                if findings.len() == 1 && findings[0].origin == "imported" &&
                   findings[0].source_id.is_some() && findings[0].source_revision.is_none() &&
                   findings[0].path == "src/main.rs"
        ));
        assert!(
            backend
                .method(RequestMethod::ExportSarif)
                .is_ok_and(|response| matches!(response, ResponseValue::SarifReport { .. }))
        );

        fs::write(&report_path, r#"{"version":"1.0.0","runs":[]}"#).unwrap();
        assert!(
            backend
                .method(RequestMethod::ImportSarif {
                    path: report_path.to_string_lossy().into_owned(),
                })
                .is_err()
        );
        fs::write(&report_path, vec![b' '; MAX_SARIF_BYTES + 1]).unwrap();
        assert!(
            backend
                .method(RequestMethod::ImportSarif {
                    path: report_path.to_string_lossy().into_owned(),
                })
                .is_err()
        );
        backend.shutdown();
    }

    #[test]
    fn project_inspection_exposes_progress_results_and_cancellation() {
        let temp = Temp::new();
        fs::write(temp.0.join("notes.txt"), "TODO").unwrap();
        let mut backend = DesktopBackend::new().unwrap();
        backend
            .inspections
            .register(Arc::new(TestInspectionProvider))
            .unwrap();
        backend
            .inspections
            .register(Arc::new(SlowInspectionProvider))
            .unwrap();
        backend
            .method(RequestMethod::OpenWorkspace {
                path: temp.0.to_string_lossy().into_owned(),
            })
            .unwrap();
        backend
            .method(RequestMethod::OpenDocument {
                path: "notes.txt".into(),
            })
            .unwrap();
        let job_id = match backend
            .method(RequestMethod::StartProjectInspection)
            .unwrap()
        {
            ResponseValue::ProjectInspectionStarted { job_id } => job_id,
            other => panic!("unexpected response {other:?}"),
        };
        let mut completed = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if matches!(backend.method(RequestMethod::ProjectInspectionStatus { job_id }).unwrap(),
                ResponseValue::ProjectInspectionStatus(summary) if summary.state == "completed" && summary.completed_documents == 1 && summary.result_count == 1)
            {
                completed = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(completed, "project inspection did not complete");
        assert!(
            matches!(backend.method(RequestMethod::ProjectInspectionFindings { job_id, offset: 0, limit: 10 }).unwrap(),
            ResponseValue::ProjectInspectionFindings(page) if page.findings.len() == 1 && page.next_offset.is_none() &&
                page.findings[0].origin == "project_snapshot" && page.findings[0].source_id.is_some())
        );

        let cancelled_job = match backend
            .method(RequestMethod::StartProjectInspection)
            .unwrap()
        {
            ResponseValue::ProjectInspectionStarted { job_id } => job_id,
            other => panic!("unexpected response {other:?}"),
        };
        backend
            .method(RequestMethod::CancelProjectInspection {
                job_id: cancelled_job,
            })
            .unwrap();
        let mut cancelled = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if matches!(backend.method(RequestMethod::ProjectInspectionStatus { job_id: cancelled_job }).unwrap(),
                ResponseValue::ProjectInspectionStatus(summary) if summary.state == "cancelled")
            {
                cancelled = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            cancelled,
            "project inspection cancellation was not observed"
        );
        backend.shutdown();
    }

    #[test]
    fn extension_setting_values_must_match_declared_type_and_range() {
        assert!(valid_setting_value(
            &SettingKind::Integer {
                default: 3,
                min: 1,
                max: 5
            },
            &ExtensionSettingValue::Integer(5)
        ));
        assert!(!valid_setting_value(
            &SettingKind::Integer {
                default: 3,
                min: 1,
                max: 5
            },
            &ExtensionSettingValue::Integer(6)
        ));
        assert!(valid_setting_value(
            &SettingKind::Choice {
                default: "fast".into(),
                options: vec!["fast".into(), "safe".into()]
            },
            &ExtensionSettingValue::Text("safe".into())
        ));
        assert!(!valid_setting_value(
            &SettingKind::Boolean { default: true },
            &ExtensionSettingValue::Text("true".into())
        ));
    }

    #[test]
    fn desktop_bridge_publishes_and_cleans_up_lua_ui_contributions() {
        let temp = Temp::new();
        let extension_root = temp.0.join("extensions");
        let example = extension_root.join("org.example.hello");
        fs::create_dir_all(&example).unwrap();
        fs::write(
            example.join("koren-extension.toml"),
            include_str!("../../../examples/extensions/org.example.hello/koren-extension.toml"),
        )
        .unwrap();
        fs::write(
            example.join("main.lua"),
            include_str!("../../../examples/extensions/org.example.hello/main.lua"),
        )
        .unwrap();
        let mut backend = DesktopBackend::new().unwrap();
        backend.extension_root = Some(extension_root);
        let listed = backend.method(RequestMethod::ListExtensions).unwrap();
        assert!(
            matches!(listed, ResponseValue::Extensions(ref catalog) if catalog.extensions.len() == 1)
        );
        backend
            .method(RequestMethod::SetExtensionEnabled {
                extension_id: "org.example.hello".into(),
                enabled: true,
            })
            .unwrap();
        backend
            .method(RequestMethod::ActivateExtension {
                extension_id: "org.example.hello".into(),
            })
            .unwrap();
        let catalog = match backend.method(RequestMethod::ListExtensions).unwrap() {
            ResponseValue::Extensions(catalog) => catalog,
            other => panic!("unexpected response {other:?}"),
        };
        assert_eq!(catalog.commands.len(), 1);
        assert_eq!(catalog.commands[0].shortcut.as_deref(), Some("Ctrl+Alt+H"));
        assert_eq!(catalog.tool_windows.len(), 1);
        assert_eq!(catalog.settings.len(), 1);
        assert_eq!(catalog.themes.len(), 1);
        assert_eq!(catalog.diagnostic_details.len(), 1);
        backend
            .method(RequestMethod::ExecuteCommand {
                command_id: "org.example.hello.hello".into(),
            })
            .unwrap();
        let refreshed = match backend.method(RequestMethod::ListExtensions).unwrap() {
            ResponseValue::Extensions(catalog) => catalog,
            other => panic!("unexpected response {other:?}"),
        };
        assert!(
            matches!(&refreshed.tool_windows[0].initial_view.elements[0], ExtensionUiElement::Text { text, .. } if text.contains("an empty workspace"))
        );
        backend
            .method(RequestMethod::SetExtensionEnabled {
                extension_id: "org.example.hello".into(),
                enabled: false,
            })
            .unwrap();
        let disabled = match backend.method(RequestMethod::ListExtensions).unwrap() {
            ResponseValue::Extensions(catalog) => catalog,
            other => panic!("unexpected response {other:?}"),
        };
        assert!(
            disabled.tool_windows.is_empty()
                && disabled.settings.is_empty()
                && disabled.themes.is_empty()
        );
        backend.shutdown();
    }

    #[test]
    fn lua_inspection_reaches_desktop_ipc_and_stale_edits_are_rejected() {
        let temp = Temp::new();
        let workspace = temp.0.join("workspace");
        fs::create_dir(&workspace).unwrap();
        fs::write(workspace.join("notes.txt"), "TODO").unwrap();
        let install_root = temp.0.join("extensions");
        let extension = install_root.join("org.example.text-inspection");
        fs::create_dir_all(&extension).unwrap();
        fs::write(
            extension.join("koren-extension.toml"),
            "schema_version=1\nid='org.example.text-inspection'\nname='Text Inspection'\npublisher='Tests'\nversion='0.1.0'\ndescription='fixture'\nentry_point='main.lua'\napi_version='1.0.0'\ncapabilities=['diagnostics','document_read']\n",
        )
        .unwrap();
        fs::write(
            extension.join("main.lua"),
            r#"
              local extension = {}
              function extension.activate()
                koren.inspections.register_rule({
                  id='todo', title='TODO marker', description='Find TODO',
                  category='maintenance', languages={'plaintext'}, severity='warning'
                }, function(document, settings)
                  if not settings.enabled then return {} end
                  local first, last = document.text:find('TODO', 1, true)
                  if not first then return {} end
                  return {{start_line=0, start_column=first - 1, end_line=0,
                    end_column=last, message='TODO found', explanation='Replace this marker.',
                    replacement='DONE'}}
                end)
              end
              return extension
            "#,
        )
        .unwrap();
        let mut state = DesktopBackend::new().unwrap();
        state.extension_root = Some(install_root);
        let backend = spawn_backend_with_state(state);
        let mut request_id = 0;
        call_backend(
            &backend,
            &mut request_id,
            RequestMethod::OpenWorkspace {
                path: workspace.to_string_lossy().into_owned(),
            },
        )
        .unwrap();
        call_backend(
            &backend,
            &mut request_id,
            RequestMethod::SetExtensionEnabled {
                extension_id: "org.example.text-inspection".into(),
                enabled: true,
            },
        )
        .unwrap();
        call_backend(
            &backend,
            &mut request_id,
            RequestMethod::ActivateExtension {
                extension_id: "org.example.text-inspection".into(),
            },
        )
        .unwrap();
        let document_id = match call_backend(
            &backend,
            &mut request_id,
            RequestMethod::OpenDocument {
                path: "notes.txt".into(),
            },
        )
        .unwrap()
        {
            ResponseValue::DocumentOpened { document_id, .. } => document_id,
            other => panic!("unexpected response {other:?}"),
        };
        let findings = match call_backend(
            &backend,
            &mut request_id,
            RequestMethod::InspectDocument {
                document_id,
                expected_revision: 0,
                language: "plaintext".into(),
            },
        )
        .unwrap()
        {
            ResponseValue::InspectionFindings {
                document_revision,
                findings,
            } => {
                assert_eq!(document_revision, 0);
                findings
            }
            other => panic!("unexpected response {other:?}"),
        };
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].provider_id, "org.example.text-inspection");
        assert_eq!(findings[0].rule_id, "org.example.text-inspection.todo");
        assert_eq!(findings[0].actions[0].edits[0].new_text, "DONE");
        let sarif =
            match call_backend(&backend, &mut request_id, RequestMethod::ExportSarif).unwrap() {
                ResponseValue::SarifReport { document, .. } => document,
                other => panic!("unexpected response {other:?}"),
            };
        let sarif: serde_json::Value = serde_json::from_str(&sarif).unwrap();
        assert_eq!(
            sarif
                .pointer("/runs/0/results")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );

        call_backend(
            &backend,
            &mut request_id,
            RequestMethod::ApplyDocumentEdits {
                document_id,
                expected_revision: 0,
                edits: vec![TextEdit {
                    range: koren_protocol::TextRange {
                        start: koren_protocol::TextPosition {
                            line: 0,
                            character: 0,
                        },
                        end: koren_protocol::TextPosition {
                            line: 0,
                            character: 4,
                        },
                    },
                    new_text: "LATER".into(),
                }],
            },
        )
        .unwrap();
        let stale_edit = call_backend(
            &backend,
            &mut request_id,
            RequestMethod::ApplyDocumentEdits {
                document_id,
                expected_revision: 0,
                edits: vec![TextEdit {
                    range: koren_protocol::TextRange {
                        start: koren_protocol::TextPosition {
                            line: 0,
                            character: 0,
                        },
                        end: koren_protocol::TextPosition {
                            line: 0,
                            character: 4,
                        },
                    },
                    new_text: "DONE".into(),
                }],
            },
        );
        assert!(
            matches!(
                stale_edit,
                Err(ProtocolError {
                    code: ErrorCode::Conflict,
                    ..
                })
            ),
            "stale edit response: {stale_edit:?}"
        );
        assert!(matches!(
            call_backend(
                &backend,
                &mut request_id,
                RequestMethod::ReadDocument { document_id }
            )
            .unwrap(),
            ResponseValue::DocumentContents { contents, revision: 1, .. } if contents == "LATER"
        ));
        let after_edit =
            match call_backend(&backend, &mut request_id, RequestMethod::ExportSarif).unwrap() {
                ResponseValue::SarifReport { document, .. } => document,
                other => panic!("unexpected response {other:?}"),
            };
        let after_edit: serde_json::Value = serde_json::from_str(&after_edit).unwrap();
        assert_eq!(
            after_edit
                .pointer("/runs/0/results")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            0
        );
        call_backend(
            &backend,
            &mut request_id,
            RequestMethod::SetExtensionEnabled {
                extension_id: "org.example.text-inspection".into(),
                enabled: false,
            },
        )
        .unwrap();
        assert!(matches!(
            call_backend(
                &backend,
                &mut request_id,
                RequestMethod::ListInspectionRules
            )
            .unwrap(),
            ResponseValue::InspectionConfiguration(config) if config.rules.is_empty()
        ));
        backend.send(BackendCommand::Shutdown).unwrap();
    }
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "koren-code-bridge-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn wait_for_event(backend: &mut DesktopBackend, matches: impl Fn(&DesktopEvent) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if backend.drain_events().iter().any(&matches) {
                return;
            }
            assert!(Instant::now() < deadline, "desktop event timed out");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn run_deferred_language_request(
        backend: &DesktopBackend,
        pool: &LanguageFeaturePool,
        request_id: u64,
        method: RequestMethod,
    ) -> Result<ResponseValue, ProtocolError> {
        let (sender, receiver) = mpsc::channel();
        let request = Request::new(request_id, method);
        assert!(backend.defer_language_feature(&request, sender, pool));
        receiver
            .recv_timeout(LANGUAGE_FEATURE_TIMEOUT + Duration::from_secs(1))
            .expect("language request response")
            .result
    }

    fn wait_for_file(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.exists() {
            assert!(
                Instant::now() < deadline,
                "language-server fixture gate timed out"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn call_backend(
        backend: &BackendSender,
        request_id: &mut u64,
        method: RequestMethod,
    ) -> Result<ResponseValue, ProtocolError> {
        *request_id += 1;
        send_request(backend, Request::new(*request_id, method))
            .expect("platform request transport")
            .result
    }

    #[test]
    fn dispatch_validates_protocol_and_performs_workspace_document_operations() {
        let temp = Temp::new();
        fs::write(temp.0.join("main.rs"), "fn main() {}\n").unwrap();
        let mut backend = DesktopBackend::new().unwrap();
        let opened = backend.dispatch(Request::new(
            9,
            RequestMethod::OpenWorkspace {
                path: temp.0.to_string_lossy().into_owned(),
            },
        ));
        assert_eq!(opened.request_id, 9);
        assert!(matches!(
            opened.result,
            Ok(ResponseValue::WorkspaceOpened { .. })
        ));
        let doc = backend.dispatch(Request::new(
            10,
            RequestMethod::OpenDocument {
                path: "main.rs".into(),
            },
        ));
        assert!(matches!(
            doc.result,
            Ok(ResponseValue::DocumentOpened { document_id: 1, .. })
        ));
        let invalid_version = Request {
            version: 99,
            request_id: 11,
            method: RequestMethod::Ping,
        };
        assert!(matches!(
            backend.dispatch(invalid_version).result,
            Err(ProtocolError {
                code: ErrorCode::UnsupportedVersion,
                ..
            })
        ));
        backend.shutdown();
    }

    #[test]
    fn public_workspace_mutations_protect_dirty_buffers_and_search_is_incremental() {
        let temp = Temp::new();
        let mut backend = DesktopBackend::new().unwrap();
        backend
            .method(RequestMethod::OpenWorkspace {
                path: temp.0.to_string_lossy().into_owned(),
            })
            .unwrap();
        assert!(matches!(
            backend.method(RequestMethod::CreateWorkspaceDirectory { path: "src".into() }),
            Ok(ResponseValue::WorkspaceEntry(_))
        ));
        assert!(matches!(
            backend.method(RequestMethod::CreateWorkspaceFile {
                path: "src/main.rs".into()
            }),
            Ok(ResponseValue::WorkspaceEntry(_))
        ));
        let document_id = match backend
            .method(RequestMethod::OpenDocument {
                path: "src/main.rs".into(),
            })
            .unwrap()
        {
            ResponseValue::DocumentOpened { document_id, .. } => document_id,
            other => panic!("unexpected response: {other:?}"),
        };
        backend
            .method(RequestMethod::ApplyDocumentEdits {
                document_id,
                expected_revision: 0,
                edits: vec![TextEdit {
                    range: koren_protocol::TextRange {
                        start: koren_protocol::TextPosition {
                            line: 0,
                            character: 0,
                        },
                        end: koren_protocol::TextPosition {
                            line: 0,
                            character: 0,
                        },
                    },
                    new_text: "unsaved".into(),
                }],
            })
            .unwrap();
        assert!(matches!(
            backend.method(RequestMethod::RenameWorkspaceEntry {
                source: "src".into(),
                destination: "code".into()
            }),
            Err(ProtocolError {
                code: ErrorCode::Conflict,
                ..
            })
        ));
        backend
            .method(RequestMethod::CloseDocument {
                document_id,
                discard_unsaved_changes: true,
            })
            .unwrap();
        backend
            .method(RequestMethod::RenameWorkspaceEntry {
                source: "src".into(),
                destination: "code".into(),
            })
            .unwrap();
        fs::write(temp.0.join("code/search.txt"), "needle 🦀\n").unwrap();
        let job_id = match backend
            .method(RequestMethod::StartWorkspaceSearch {
                query: "needle".into(),
                case_sensitive: true,
                regular_expression: false,
                file_filter: Some("*.txt".into()),
            })
            .unwrap()
        {
            ResponseValue::SearchStarted { job_id } => job_id,
            other => panic!("unexpected response: {other:?}"),
        };
        let mut found = Vec::new();
        for _ in 0..500 {
            match backend
                .method(RequestMethod::PollWorkspaceSearch { job_id })
                .unwrap()
            {
                ResponseValue::SearchBatch {
                    matches, complete, ..
                } => {
                    found.extend(matches);
                    if complete {
                        break;
                    }
                }
                other => panic!("unexpected response: {other:?}"),
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, "code/search.txt");
        backend.shutdown();
    }

    #[test]
    fn local_git_operations_are_exposed_through_typed_protocol_and_publish_events() {
        let temp = Temp::new();
        let repository = git2::Repository::init(&temp.0).unwrap();
        repository
            .config()
            .unwrap()
            .set_str("user.name", "Koren Test")
            .unwrap();
        repository
            .config()
            .unwrap()
            .set_str("user.email", "koren@example.invalid")
            .unwrap();
        fs::write(temp.0.join("README.md"), "local change\n").unwrap();
        let mut backend = DesktopBackend::new().unwrap();
        backend
            .method(RequestMethod::OpenWorkspace {
                path: temp.0.to_string_lossy().into_owned(),
            })
            .unwrap();
        assert!(
            matches!(backend.method(RequestMethod::DiscoverGitRepositories).unwrap(), ResponseValue::GitRepositories(repositories) if repositories.len() == 1)
        );
        assert!(
            matches!(backend.method(RequestMethod::GitStatus { path: String::new() }).unwrap(), ResponseValue::GitStatus(status) if status.files.iter().any(|file| file.path == "README.md" && file.untracked))
        );
        backend
            .method(RequestMethod::GitStage {
                repository: String::new(),
                path: "README.md".into(),
            })
            .unwrap();
        assert!(
            matches!(backend.method(RequestMethod::GitStatus { path: String::new() }).unwrap(), ResponseValue::GitStatus(status) if status.files.iter().any(|file| file.path == "README.md" && file.staged))
        );
        assert!(
            matches!(backend.method(RequestMethod::GitCommit { repository: String::new(), message: "initial".into() }).unwrap(), ResponseValue::GitOperation { operation, commit: Some(_) } if operation == "committed")
        );
        assert!(
            matches!(backend.method(RequestMethod::GitStatus { path: String::new() }).unwrap(), ResponseValue::GitStatus(status) if status.files.is_empty())
        );
        let events = backend.drain_events();
        assert!(
            events
                .iter()
                .any(|event| event.kind == "git_status_changed")
        );
        assert!(
            events
                .iter()
                .any(|event| event.kind == "git_operation_completed" && event.commit.is_some())
        );
        backend.shutdown();
    }

    #[test]
    fn platform_commands_are_listed_and_invoked_through_the_existing_registry() {
        let mut backend = DesktopBackend::new().unwrap();
        assert!(matches!(
            backend.method(RequestMethod::ListCommands).unwrap(),
            ResponseValue::Commands(commands) if commands == ["core.ping", "editor.new_file", "editor.search", "editor.source_control"]
        ));
        assert!(matches!(
            backend.method(RequestMethod::ExecuteCommand { command_id: "core.ping".into() }).unwrap(),
            ResponseValue::CommandOutput(output) if output == "pong"
        ));
        assert!(matches!(
            backend.method(RequestMethod::ExecuteCommand {
                command_id: "missing.command".into()
            }),
            Err(ProtocolError {
                code: ErrorCode::NotFound,
                ..
            })
        ));
        backend.shutdown();
    }

    #[test]
    fn cargo_tasks_require_one_time_approval_and_event_dispatch_preserves_fifo() {
        let temp = Temp::new();
        fs::write(
            temp.0.join("Cargo.toml"),
            "[package]\nname='demo'\nversion='0.1.0'\nedition='2024'\n",
        )
        .unwrap();
        fs::create_dir_all(temp.0.join("src")).unwrap();
        fs::write(temp.0.join("src/main.rs"), "fn main() {}\n").unwrap();
        let mut backend = DesktopBackend::new().unwrap();
        assert!(matches!(
            backend.method(RequestMethod::OpenWorkspace {
                path: temp.0.to_string_lossy().into_owned()
            }),
            Ok(ResponseValue::WorkspaceOpened { .. })
        ));
        let response = backend
            .method(RequestMethod::DiscoverCargoProjects {
                max_directories: 100,
                max_projects: 20,
                max_depth: 8,
            })
            .unwrap();
        assert!(matches!(response, ResponseValue::CargoProjects(_)));
        let tasks = backend.method(RequestMethod::ListTasks).unwrap();
        let ResponseValue::Tasks(tasks) = tasks else {
            panic!("expected tasks")
        };
        assert!(!tasks.is_empty());
        let request = backend
            .method(RequestMethod::RequestTaskExecution {
                task_id: tasks[0].task_id,
            })
            .unwrap();
        assert!(matches!(request, ResponseValue::ApprovalRequired(_)));
        assert!(!backend.drain_events().is_empty());
        backend.shutdown();
    }

    #[test]
    fn fake_language_server_is_approved_synchronized_and_reaches_typed_editor_features() {
        let temp = Temp::new();
        fs::write(temp.0.join("main.demo"), "hello\n").unwrap();
        let install_root = temp.0.join("extension-install");
        let extension_dir = install_root.join("org.example.demo-server");
        fs::create_dir_all(&extension_dir).unwrap();
        fs::write(extension_dir.join("main.lua"), "return {}\n").unwrap();
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/koren-lsp/tests/fake_lsp_server.py");
        let command = if cfg!(windows) { "python" } else { "python3" };
        let argument = serde_json::to_string(&fixture.to_string_lossy()).unwrap();
        let manifest = format!(
            "schema_version=1\nid='org.example.demo-server'\nname='Demo server'\npublisher='Koren tests'\nversion='0.1.0'\ndescription='Fake language-server integration test'\nentry_point='main.lua'\napi_version='1.0.0'\ncapabilities=['language_definitions','language_server']\n[[languages]]\nid='demo'\nname='Demo'\nextensions=['demo']\n[[language_servers]]\nid='fake'\ncommand='{command}'\nlanguages=['demo']\nfile_extensions=['demo']\narguments=[{argument}]\nenvironment={{FAKE_SECRET='hidden-value',FAKE_LSP_REQUIRE_CODE_ACTION_CONTEXT='1'}}\n"
        );
        fs::write(extension_dir.join("koren-extension.toml"), manifest).unwrap();

        let mut backend = DesktopBackend::new().unwrap();
        backend.extension_root = Some(install_root);
        assert!(matches!(
            backend.method(RequestMethod::OpenWorkspace {
                path: temp.0.to_string_lossy().into_owned()
            }),
            Ok(ResponseValue::WorkspaceOpened { .. })
        ));
        assert!(
            matches!(backend.method(RequestMethod::ListExtensions).unwrap(), ResponseValue::Extensions(catalog) if catalog.extensions.len() == 1 && catalog.language_servers.len() == 1)
        );
        backend
            .method(RequestMethod::SetExtensionEnabled {
                extension_id: "org.example.demo-server".into(),
                enabled: true,
            })
            .unwrap();
        let approval = backend
            .method(RequestMethod::StartLanguageServer {
                extension_id: "org.example.demo-server".into(),
                configuration_id: "fake".into(),
            })
            .unwrap();
        let ResponseValue::LanguageServerApprovalRequired(approval) = approval else {
            panic!("expected explicit server approval")
        };
        assert_eq!(
            approval.environment_keys,
            ["FAKE_LSP_REQUIRE_CODE_ACTION_CONTEXT", "FAKE_SECRET"]
        );
        assert!(
            !serde_json::to_string(&approval)
                .unwrap()
                .contains("hidden-value")
        );
        let ResponseValue::LanguageServerStarted { server_id, .. } = backend
            .method(RequestMethod::ApproveLanguageServer {
                approval_id: approval.approval_id,
            })
            .unwrap()
        else {
            panic!("expected server startup")
        };
        let ResponseValue::DocumentOpened { document_id, .. } = backend
            .method(RequestMethod::OpenDocument {
                path: "main.demo".into(),
            })
            .unwrap()
        else {
            panic!("expected document open")
        };
        wait_for_event(&mut backend, |event| {
            event.kind == "language_diagnostics" && event.document_id == Some(document_id)
        });
        assert!(
            matches!(backend.method(RequestMethod::LanguageHover { document_id, expected_revision: 0, position: koren_protocol::TextPosition { line: 0, character: 1 } }).unwrap(), ResponseValue::LanguageHover { contents: Some(contents), .. } if contents == "hover info")
        );
        assert!(
            matches!(backend.method(RequestMethod::LanguageCompletion { document_id, expected_revision: 0, position: koren_protocol::TextPosition { line: 0, character: 1 } }).unwrap(), ResponseValue::LanguageCompletions { items, .. } if items[0].label == "complete")
        );
        assert!(
            matches!(backend.method(RequestMethod::LanguageDefinition { document_id, expected_revision: 0, position: koren_protocol::TextPosition { line: 0, character: 1 } }).unwrap(), ResponseValue::LanguageDefinitions { locations, .. } if locations.is_empty())
        );
        let feature_pool = LanguageFeaturePool::new();
        let range = koren_protocol::TextRange {
            start: koren_protocol::TextPosition {
                line: 0,
                character: 0,
            },
            end: koren_protocol::TextPosition {
                line: 0,
                character: 5,
            },
        };
        assert!(matches!(
            run_deferred_language_request(&backend, &feature_pool, 21, RequestMethod::LanguageCodeActions {
                document_id, expected_revision: 0, range,
                diagnostics: Some(vec![koren_protocol::LanguageDiagnosticContext {
                    range,
                    severity: Some(1),
                    code: Some("missing-name".into()),
                    source: Some("rust-analyzer".into()),
                    message: "cannot find value in this scope".into(),
                }]),
            }).unwrap(),
            ResponseValue::LanguageCodeActions { actions, .. }
                if actions.len() == 1 && actions[0].edits.as_ref().is_some_and(|edits| edits[0].new_text == "demo")
        ));
        assert!(matches!(
            run_deferred_language_request(&backend, &feature_pool, 22, RequestMethod::LanguageReferences {
                document_id, expected_revision: 0,
                position: koren_protocol::TextPosition { line: 0, character: 1 },
                include_declaration: true,
            }).unwrap(),
            ResponseValue::LanguageReferences { locations, .. } if locations.len() == 1
        ));
        assert!(matches!(
            run_deferred_language_request(&backend, &feature_pool, 23, RequestMethod::LanguageFormat {
                document_id, expected_revision: 0,
            }).unwrap(),
            ResponseValue::LanguageFormatting { edits, .. } if edits.len() == 1 && edits[0].new_text == "x"
        ));
        assert!(matches!(
            run_deferred_language_request(&backend, &feature_pool, 24, RequestMethod::LanguageRename {
                document_id, expected_revision: 0,
                position: koren_protocol::TextPosition { line: 0, character: 1 },
                new_name: "world".into(),
            }).unwrap(),
            ResponseValue::LanguageRename { edits, .. } if edits.len() == 1 && edits[0].new_text == "world"
        ));
        assert!(matches!(
            backend
                .method(RequestMethod::ApplyDocumentEdits {
                    document_id,
                    expected_revision: 0,
                    edits: vec![TextEdit {
                        range: koren_protocol::TextRange {
                            start: koren_protocol::TextPosition {
                                line: 0,
                                character: 0
                            },
                            end: koren_protocol::TextPosition {
                                line: 0,
                                character: 5
                            }
                        },
                        new_text: "world".into()
                    }]
                })
                .unwrap(),
            ResponseValue::DocumentChanged { revision: 1, .. }
        ));
        assert!(matches!(
            backend
                .method(RequestMethod::LanguageHover {
                    document_id,
                    expected_revision: 1,
                    position: koren_protocol::TextPosition {
                        line: 0,
                        character: 1
                    }
                })
                .unwrap(),
            ResponseValue::LanguageHover {
                document_revision: 1,
                ..
            }
        ));
        backend
            .method(RequestMethod::CloseDocument {
                document_id,
                discard_unsaved_changes: true,
            })
            .unwrap();
        backend
            .method(RequestMethod::StopLanguageServer { server_id })
            .unwrap();
        backend.method(RequestMethod::CloseWorkspace).unwrap();
        backend.shutdown();
    }

    #[test]
    fn stalled_lsp_initialization_and_hover_do_not_block_worker_save_or_close() {
        let temp = Temp::new();
        fs::write(temp.0.join("main.demo"), "hello 🦀\n").unwrap();
        let install_root = temp.0.join("extension-install");
        let extension_dir = install_root.join("org.example.stalled-server");
        fs::create_dir_all(&extension_dir).unwrap();
        fs::write(extension_dir.join("main.lua"), "return {}\n").unwrap();
        let init_gate = temp.0.join("release-init");
        let init_reached = temp.0.join("init-reached");
        let hover_gate = temp.0.join("release-hover");
        let hover_reached = temp.0.join("hover-reached");
        let trace = temp.0.join("lsp-trace.jsonl");
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/koren-lsp/tests/fake_lsp_server.py");
        let command = if cfg!(windows) { "python" } else { "python3" };
        let args = serde_json::to_string(&fixture.to_string_lossy()).unwrap();
        let toml_string = |path: &Path| serde_json::to_string(&path.to_string_lossy()).unwrap();
        let manifest = format!(
            "schema_version=1\nid='org.example.stalled-server'\nname='Stalled test server'\npublisher='Koren tests'\nversion='0.1.0'\ndescription='Concurrency regression fixture'\nentry_point='main.lua'\napi_version='1.0.0'\ncapabilities=['language_definitions','language_server']\n[[languages]]\nid='demo'\nname='Demo'\nextensions=['demo']\n[[language_servers]]\nid='fake'\ncommand='{command}'\nlanguages=['demo']\nfile_extensions=['demo']\narguments=[{args}]\nenvironment={{FAKE_LSP_INIT_GATE={},FAKE_LSP_INIT_REACHED={},FAKE_LSP_HOVER_GATE={},FAKE_LSP_HOVER_REACHED={},FAKE_LSP_TRACE={}}}\n",
            toml_string(&init_gate),
            toml_string(&init_reached),
            toml_string(&hover_gate),
            toml_string(&hover_reached),
            toml_string(&trace),
        );
        fs::write(extension_dir.join("koren-extension.toml"), manifest).unwrap();

        let mut state = DesktopBackend::new().unwrap();
        state.extension_root = Some(install_root);
        let backend = spawn_backend_with_state(state);
        let (event_tx, event_rx) = mpsc::sync_channel(2);
        subscribe(&backend, event_tx).unwrap();
        let mut request_id = 0;
        call_backend(
            &backend,
            &mut request_id,
            RequestMethod::OpenWorkspace {
                path: temp.0.to_string_lossy().into_owned(),
            },
        )
        .unwrap();
        call_backend(&backend, &mut request_id, RequestMethod::ListExtensions).unwrap();
        call_backend(
            &backend,
            &mut request_id,
            RequestMethod::SetExtensionEnabled {
                extension_id: "org.example.stalled-server".into(),
                enabled: true,
            },
        )
        .unwrap();
        let approval = call_backend(
            &backend,
            &mut request_id,
            RequestMethod::StartLanguageServer {
                extension_id: "org.example.stalled-server".into(),
                configuration_id: "fake".into(),
            },
        )
        .unwrap();
        let ResponseValue::LanguageServerApprovalRequired(approval) = approval else {
            panic!("expected explicit server approval");
        };
        let initialization_dispatch_started = Instant::now();
        call_backend(
            &backend,
            &mut request_id,
            RequestMethod::ApproveLanguageServer {
                approval_id: approval.approval_id,
            },
        )
        .unwrap();
        println!(
            "phase65_bridge_initialize_dispatch_ms={:.3}",
            initialization_dispatch_started.elapsed().as_secs_f64() * 1_000.0
        );
        wait_for_file(&init_reached);

        let ResponseValue::DocumentOpened { document_id, .. } = call_backend(
            &backend,
            &mut request_id,
            RequestMethod::OpenDocument {
                path: "main.demo".into(),
            },
        )
        .unwrap() else {
            panic!("expected document open");
        };
        let ResponseValue::DocumentChanged { revision, .. } = call_backend(
            &backend,
            &mut request_id,
            RequestMethod::ApplyDocumentEdits {
                document_id,
                expected_revision: 0,
                edits: vec![TextEdit {
                    range: koren_protocol::TextRange {
                        start: koren_protocol::TextPosition {
                            line: 0,
                            character: 0,
                        },
                        end: koren_protocol::TextPosition {
                            line: 0,
                            character: 5,
                        },
                    },
                    new_text: "world".into(),
                }],
            },
        )
        .unwrap() else {
            panic!("expected document edit");
        };
        let started = Instant::now();
        assert!(matches!(
            call_backend(
                &backend,
                &mut request_id,
                RequestMethod::SaveDocument {
                    document_id,
                    expected_revision: revision,
                },
            ),
            Ok(ResponseValue::DocumentSaved { dirty: false, .. })
        ));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "save waited for stalled initialize"
        );
        println!(
            "phase65_bridge_save_during_stalled_initialize_ms={:.3}",
            started.elapsed().as_secs_f64() * 1_000.0
        );
        assert_eq!(
            fs::read_to_string(temp.0.join("main.demo")).unwrap(),
            "world 🦀\n"
        );

        fs::write(&init_gate, "continue").unwrap();
        let mut platform_events = Vec::new();
        wait_for_platform_event(&event_rx, &mut platform_events, |event| {
            event.kind == "language_server_initialized"
        });
        wait_for_platform_event(&event_rx, &mut platform_events, |event| {
            event.kind == "language_diagnostics" && event.document_id == Some(document_id)
        });
        wait_for_trace(&trace, "textDocument/didChange", 1);

        let hover_backend = backend.clone();
        let hover_id = request_id + 1;
        request_id = hover_id;
        let (hover_tx, hover_rx) = mpsc::channel();
        thread::spawn(move || {
            let result = send_request(
                &hover_backend,
                Request::new(
                    hover_id,
                    RequestMethod::LanguageHover {
                        document_id,
                        expected_revision: revision,
                        position: koren_protocol::TextPosition {
                            line: 0,
                            character: 1,
                        },
                    },
                ),
            );
            let _ = hover_tx.send(result);
        });
        let feature_started = Instant::now();
        wait_for_file(&hover_reached);
        println!(
            "phase65_hover_dispatch_to_fixture_receipt_ms={:.3}",
            feature_started.elapsed().as_secs_f64() * 1_000.0
        );

        let ResponseValue::DocumentChanged { revision, .. } = call_backend(
            &backend,
            &mut request_id,
            RequestMethod::ApplyDocumentEdits {
                document_id,
                expected_revision: revision,
                edits: vec![TextEdit {
                    range: koren_protocol::TextRange {
                        start: koren_protocol::TextPosition {
                            line: 0,
                            character: 0,
                        },
                        end: koren_protocol::TextPosition {
                            line: 0,
                            character: 5,
                        },
                    },
                    new_text: "again".into(),
                }],
            },
        )
        .unwrap() else {
            panic!("expected second edit");
        };
        let started = Instant::now();
        call_backend(
            &backend,
            &mut request_id,
            RequestMethod::SaveDocument {
                document_id,
                expected_revision: revision,
            },
        )
        .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "save waited for stalled hover"
        );
        println!(
            "phase65_bridge_save_during_stalled_hover_ms={:.3}",
            started.elapsed().as_secs_f64() * 1_000.0
        );
        let ResponseValue::DocumentChanged { revision, .. } = call_backend(
            &backend,
            &mut request_id,
            RequestMethod::ApplyDocumentEdits {
                document_id,
                expected_revision: revision,
                edits: vec![TextEdit {
                    range: koren_protocol::TextRange {
                        start: koren_protocol::TextPosition {
                            line: 0,
                            character: 0,
                        },
                        end: koren_protocol::TextPosition {
                            line: 0,
                            character: 5,
                        },
                    },
                    new_text: "final".into(),
                }],
            },
        )
        .unwrap() else {
            panic!("expected third document edit");
        };
        call_backend(
            &backend,
            &mut request_id,
            RequestMethod::SaveDocument {
                document_id,
                expected_revision: revision,
            },
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(temp.0.join("main.demo")).unwrap(),
            "final 🦀\n"
        );
        call_backend(
            &backend,
            &mut request_id,
            RequestMethod::CloseDocument {
                document_id,
                discard_unsaved_changes: false,
            },
        )
        .unwrap();
        let hover_result = hover_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("closed document should cancel its pending feature response")
            .expect("hover bridge response");
        assert!(matches!(
            hover_result.result,
            Err(ProtocolError {
                code: ErrorCode::NotFound,
                ..
            })
        ));
        fs::write(&hover_gate, "continue").unwrap();
        wait_for_trace(&trace, "textDocument/didChange", 3);
        wait_for_trace(&trace, "textDocument/didClose", 1);
        call_backend(&backend, &mut request_id, RequestMethod::CloseWorkspace).unwrap();
        shutdown(&backend);
    }

    fn wait_for_platform_event(
        receiver: &Receiver<Vec<DesktopEvent>>,
        pending: &mut Vec<DesktopEvent>,
        matches: impl Fn(&DesktopEvent) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(index) = pending.iter().position(&matches) {
                pending.remove(index);
                return;
            }
            if let Ok(mut events) = receiver.recv_timeout(Duration::from_millis(100)) {
                pending.append(&mut events);
            }
            assert!(
                Instant::now() < deadline,
                "platform event timed out; received={pending:?}"
            );
        }
    }

    fn wait_for_trace(path: &Path, method: &str, required_count: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let trace = fs::read_to_string(path).unwrap_or_default();
            let messages = trace
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .filter(|message| {
                    message.get("method").and_then(serde_json::Value::as_str) == Some(method)
                })
                .collect::<Vec<_>>();
            if messages.len() >= required_count {
                if method == "textDocument/didChange" {
                    let versions = messages
                        .iter()
                        .filter_map(|message| {
                            message.pointer("/params/textDocument/version")?.as_i64()
                        })
                        .collect::<Vec<_>>();
                    assert!(versions.windows(2).all(|pair| pair[0] < pair[1]));
                }
                return;
            }
            assert!(
                Instant::now() < deadline,
                "document synchronization trace timed out"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}
