use koren_cargo::CargoProvider;
use koren_commands::{CommandError, CommandRegistry};
use koren_core::{Platform, PlatformEvent};
use koren_documents::{DocumentId, DocumentManager};
use koren_extensions::{CommandContext, ExtensionManager, ExtensionState, MenuLocation};
use koren_lsp::{
    DiagnosticSet, LspDocumentSync, LspServerManager, ServerAuthorizer, ServerConfig, ServerEvent,
    ServerId, path_to_uri,
};
use koren_lua::{LuaDocumentContext, LuaExtensionRuntime, LuaHostContext, LuaLimits};
use koren_projects::{DiscoveryLimits, ProjectId, ProjectInfo, ProjectManager};
use koren_protocol::{
    CargoPackageSummary, CargoProjectSummary, CargoTargetSummary, DirectoryEntry,
    DirectoryEntryKind, ErrorCode, ExecutionApprovalSummary, ExtensionCatalog,
    ExtensionCommandSummary, ExtensionMenuSummary, ExtensionSummary as ProtocolExtensionSummary,
    LanguageServerApprovalSummary, LanguageServerSummary, ProjectLifecycle, ProjectSummary,
    ProtocolError, Request, RequestMethod, Response, ResponseValue, TaskLifecycle, TaskSummary,
    TextEdit, validate_version,
};
use koren_tasks::{
    ExecutionRequestId, TaskApprovalManager, TaskAuthorizationDecision, TaskCaller, TaskDefinition,
    TaskExecutionPolicy, TaskExecutionRequest, TaskExecutionRequestOutcome, TaskId, TaskManager,
    TaskOptions, TaskRunId, TaskSnapshot, TaskState,
};
use koren_workspace::{EntryKind, WorkspaceManager};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::Duration,
};

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
    pub server_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<DiagnosticSet>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

pub(crate) enum BackendCommand {
    Request(Box<Request>, Sender<Response>),
    Subscribe(Sender<Vec<DesktopEvent>>),
    Shutdown,
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
    events: Sender<Vec<DesktopEvent>>,
) -> Result<(), String> {
    sender
        .send(BackendCommand::Subscribe(events))
        .map_err(|_| "platform worker stopped".to_owned())
}

pub(crate) fn shutdown(sender: &BackendSender) {
    let _ = sender.send(BackendCommand::Shutdown);
}

fn backend_loop(receiver: Receiver<BackendCommand>) {
    let mut state = match DesktopBackend::new() {
        Ok(state) => state,
        Err(error) => {
            eprintln!("failed to initialize Koren Code Editor platform: {error}");
            return;
        }
    };
    let mut subscribers: Vec<Sender<Vec<DesktopEvent>>> = Vec::new();
    let mut ticks_without_events = 0u16;
    loop {
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(BackendCommand::Request(request, reply)) => {
                let _ = reply.send(state.dispatch(*request));
            }
            Ok(BackendCommand::Subscribe(sender)) => {
                if subscribers.len() < 8 {
                    subscribers.push(sender);
                } else {
                    let _ = sender.send(Vec::new());
                }
            }
            Ok(BackendCommand::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        let events = state.drain_events();
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
            subscribers.retain(|subscriber| subscriber.send(events.clone()).is_ok());
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
    extensions: ExtensionManager,
    extension_root: Option<PathBuf>,
    extensions_discovered: bool,
    lua_runtimes: BTreeMap<String, LuaExtensionRuntime>,
    active_document: Option<DocumentId>,
    language_servers: LspServerManager,
    active_language_servers: BTreeMap<u64, ServerId>,
    language_server_owners: BTreeMap<u64, (String, String)>,
    language_document_sync: BTreeMap<u64, LspDocumentSync>,
    open_documents: BTreeSet<DocumentId>,
    pending_language_servers: BTreeMap<u64, PendingLanguageServer>,
    next_language_server_approval: u64,
}

struct PendingLanguageServer {
    extension_id: String,
    configuration_id: String,
    configuration: ServerConfig,
}

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
        let mut projects = ProjectManager::new();
        let cargo = CargoProvider::new();
        cargo
            .register(&mut projects)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            platform,
            commands,
            workspace: WorkspaceManager::new(),
            documents: DocumentManager::new(),
            projects,
            cargo,
            tasks: TaskManager::new(TaskOptions::default()).map_err(|error| error.to_string())?,
            approvals: TaskApprovalManager::new(),
            extensions: ExtensionManager::new(),
            extension_root: extension_install_root(),
            extensions_discovered: false,
            lua_runtimes: BTreeMap::new(),
            active_document: None,
            language_servers: LspServerManager::new(),
            active_language_servers: BTreeMap::new(),
            language_server_owners: BTreeMap::new(),
            language_document_sync: BTreeMap::new(),
            open_documents: BTreeSet::new(),
            pending_language_servers: BTreeMap::new(),
            next_language_server_approval: 0,
        })
    }

    fn shutdown(&mut self) {
        self.approvals.shutdown();
        self.language_servers.shutdown();
        self.tasks.shutdown();
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
                self.approvals.invalidate_workspace(&root);
                self.language_servers
                    .close_workspace(&root)
                    .map_err(lsp_error)?;
                self.active_language_servers.clear();
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
                    entries
                        .into_iter()
                        .map(|entry| DirectoryEntry {
                            name: entry.name.to_string_lossy().into_owned(),
                            path: entry.path.to_string_lossy().into_owned(),
                            kind: match entry.kind {
                                EntryKind::File => DirectoryEntryKind::File,
                                EntryKind::Directory => DirectoryEntryKind::Directory,
                                EntryKind::Symlink => DirectoryEntryKind::Symlink,
                                EntryKind::Other => DirectoryEntryKind::Other,
                            },
                            size_bytes: entry.size_bytes,
                        })
                        .collect(),
                ))
            }
            RequestMethod::OpenDocument { path } => {
                let id = self
                    .documents
                    .open_file(&self.workspace, path)
                    .map_err(document_error)?;
                self.open_documents.insert(id);
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
                let info = self
                    .documents
                    .apply_edits(DocumentId(document_id), expected_revision, &edits)
                    .map_err(document_error)?;
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
                self.documents
                    .close(DocumentId(document_id), discard_unsaved_changes)
                    .map_err(document_error)?;
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
            _ => Err(invalid("operation is not supported by Koren Code Editor")),
        }
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
            menu_items: self.active_extension_menus(),
            language_servers: self.declared_language_servers(),
        }))
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
        let output = self
            .extensions
            .invoke_command(&extension_id, &context, &[])
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
        }
    }

    fn active_extension_commands(&self) -> Vec<ExtensionCommandSummary> {
        let mut commands = Vec::new();
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
                    }
                }));
            }
        }
        commands.sort_by(|left, right| left.id.cmp(&right.id));
        commands
    }

    fn active_extension_menus(&self) -> Vec<ExtensionMenuSummary> {
        let mut menus = Vec::new();
        for extension in self.extensions.list() {
            if extension.state != ExtensionState::Active {
                continue;
            }
            if let Ok(contributions) = self.extensions.contributions(&extension.id) {
                menus.extend(contributions.menus.iter().map(|menu| {
                    ExtensionMenuSummary {
                        command_id: menu.command_id.clone(),
                        label: menu.label.clone(),
                        location: match menu.location {
                            MenuLocation::Tools => "tools",
                            MenuLocation::Navigate => "navigate",
                            MenuLocation::EditorContext => "editor_context",
                        }
                        .into(),
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
                if let Ok(server) = self.language_servers.server_mut(server_id) {
                    let _ = server.send_sync_notification(&notification);
                }
            }
        }
        self.active_language_servers.remove(&id);
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
    ) -> Result<Vec<(u64, ServerId, String, i32)>, ProtocolError> {
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
            let Some(sync) = self.language_document_sync.get(application_id) else {
                continue;
            };
            let (Some(uri), Some((version, revision))) =
                (sync.uri(document), sync.version(document))
            else {
                continue;
            };
            if revision == expected_revision {
                targets.push((*application_id, *server_id, uri.to_owned(), version));
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
        for (_, server_id, uri, version) in targets {
            let Ok(server) = self.language_servers.server_mut(server_id) else {
                continue;
            };
            match server.hover_with_timeout(&uri, version, position, Duration::from_millis(150)) {
                Ok(result) if result.is_current(version) => {
                    let value = result.value;
                    return Ok(ResponseValue::LanguageHover {
                        document_revision: expected_revision,
                        contents: value.as_ref().map(|hover| hover.contents.clone()),
                        range: value.and_then(|hover| hover.range),
                    });
                }
                Err(koren_lsp::LspError::UnsupportedFeature(_))
                | Err(koren_lsp::LspError::RequestTimeout(_)) => continue,
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
        for (_, server_id, uri, version) in targets {
            let Ok(server) = self.language_servers.server_mut(server_id) else {
                continue;
            };
            match server.completion_with_timeout(
                &uri,
                version,
                position,
                Duration::from_millis(150),
            ) {
                Ok(result) if result.is_current(version) => {
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
                | Err(koren_lsp::LspError::RequestTimeout(_)) => continue,
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
        for (_, server_id, uri, version) in targets {
            let Ok(server) = self.language_servers.server_mut(server_id) else {
                continue;
            };
            match server.definition_with_timeout(
                &uri,
                version,
                position,
                Duration::from_millis(150),
            ) {
                Ok(result) if result.is_current(version) => {
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
                | Err(koren_lsp::LspError::RequestTimeout(_)) => continue,
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

    fn drain_events(&mut self) -> Vec<DesktopEvent> {
        let mut events = Vec::new();
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
        let _ = self.tasks.drain_events();
        let server_ids = self
            .active_language_servers
            .iter()
            .map(|(id, server)| (*id, *server))
            .collect::<Vec<_>>();
        let mut dead_servers = Vec::new();
        for (application_id, server_id) in server_ids {
            let Ok(server) = self.language_servers.server_mut(server_id) else {
                continue;
            };
            for service_event in server.drain_events() {
                match service_event {
                    ServerEvent::Diagnostics(mut diagnostics) => {
                        let Some(sync) = self.language_document_sync.get(&application_id) else {
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
                        })
                    }
                    ServerEvent::Notification { .. } => {}
                }
            }
            if server.is_running().is_ok_and(|running| !running) {
                dead_servers.push(application_id);
            }
        }
        for application_id in dead_servers {
            if let Some(server) = self.active_language_servers.remove(&application_id) {
                let _ = self.language_servers.stop_server(server);
            }
            self.language_server_owners.remove(&application_id);
            self.language_document_sync.remove(&application_id);
            events.push(DesktopEvent {
                kind: "language_server_stopped".into(),
                server_id: Some(application_id),
                message: Some("Language server exited".into()),
                ..empty_event()
            });
        }
        for event in &events {
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
            if let Ok(server) = self.language_servers.server_mut(server_id) {
                let _ = server.send_sync_notification(&notification);
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
            if let Ok(server) = self.language_servers.server_mut(server_id) {
                let _ = server.send_sync_notification(&notification);
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
            if let Ok(server) = self.language_servers.server_mut(server_id) {
                let _ = server.send_sync_notification(&notification);
            }
        }
    }
}

fn command_ping(arguments: &[String]) -> Result<String, CommandError> {
    if !arguments.is_empty() {
        return Err(CommandError::Execution(
            "core.ping takes no arguments".into(),
        ));
    }
    Ok("pong".into())
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
        document_id: None,
        project_id: None,
        run_id: None,
        server_id: None,
        revision: None,
        root: None,
        diagnostics: None,
        message: None,
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
fn document_error(error: koren_documents::DocumentError) -> ProtocolError {
    let code = match error {
        koren_documents::DocumentError::Conflict(_)
        | koren_documents::DocumentError::ConflictChangedAgain(_) => ErrorCode::Conflict,
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
    fn platform_commands_are_listed_and_invoked_through_the_existing_registry() {
        let mut backend = DesktopBackend::new().unwrap();
        assert!(matches!(
            backend.method(RequestMethod::ListCommands).unwrap(),
            ResponseValue::Commands(commands) if commands == ["core.ping"]
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
            "schema_version=1\nid='org.example.demo-server'\nname='Demo server'\npublisher='Koren tests'\nversion='0.1.0'\ndescription='Fake language-server integration test'\nentry_point='main.lua'\napi_version='1.0.0'\ncapabilities=['language_definitions','language_server']\n[[languages]]\nid='demo'\nname='Demo'\nextensions=['demo']\n[[language_servers]]\nid='fake'\ncommand='{command}'\nlanguages=['demo']\nfile_extensions=['demo']\narguments=[{argument}]\nenvironment={{FAKE_SECRET='hidden-value'}}\n"
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
        assert_eq!(approval.environment_keys, ["FAKE_SECRET"]);
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
        std::thread::sleep(Duration::from_millis(30));
        assert!(
            backend
                .drain_events()
                .iter()
                .any(|event| event.kind == "language_diagnostics"
                    && event.document_id == Some(document_id))
        );
        assert!(
            matches!(backend.method(RequestMethod::LanguageHover { document_id, expected_revision: 0, position: koren_protocol::TextPosition { line: 0, character: 1 } }).unwrap(), ResponseValue::LanguageHover { contents: Some(contents), .. } if contents == "hover info")
        );
        assert!(
            matches!(backend.method(RequestMethod::LanguageCompletion { document_id, expected_revision: 0, position: koren_protocol::TextPosition { line: 0, character: 1 } }).unwrap(), ResponseValue::LanguageCompletions { items, .. } if items[0].label == "complete")
        );
        assert!(
            matches!(backend.method(RequestMethod::LanguageDefinition { document_id, expected_revision: 0, position: koren_protocol::TextPosition { line: 0, character: 1 } }).unwrap(), ResponseValue::LanguageDefinitions { locations, .. } if locations.is_empty())
        );
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
}
