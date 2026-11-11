use koren_cargo::CargoProvider;
use koren_commands::{CommandError, CommandRegistry};
use koren_core::{Platform, PlatformEvent};
use koren_documents::{DocumentId, DocumentManager};
use koren_projects::{DiscoveryLimits, ProjectId, ProjectInfo, ProjectManager};
use koren_protocol::{
    CargoPackageSummary, CargoProjectSummary, CargoTargetSummary, DirectoryEntry,
    DirectoryEntryKind, ErrorCode, ExecutionApprovalSummary, ProjectLifecycle, ProjectSummary,
    ProtocolError, Request, RequestMethod, Response, ResponseValue, TaskLifecycle, TaskSummary,
    validate_version,
};
use koren_tasks::{
    ExecutionRequestId, TaskApprovalManager, TaskAuthorizationDecision, TaskCaller, TaskDefinition,
    TaskExecutionPolicy, TaskExecutionRequest, TaskExecutionRequestOutcome, TaskId, TaskManager,
    TaskOptions, TaskRunId, TaskSnapshot, TaskState,
};
use koren_workspace::{EntryKind, WorkspaceManager};
use serde::Serialize;
use std::{
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
    pub revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
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
        })
    }

    fn shutdown(&mut self) {
        self.approvals.shutdown();
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
            RequestMethod::ListCommands => Ok(ResponseValue::Commands(
                self.commands.ids().into_iter().map(str::to_owned).collect(),
            )),
            RequestMethod::ExecuteCommand { command_id } => self
                .commands
                .invoke(&command_id, &[])
                .map(ResponseValue::CommandOutput)
                .map_err(command_error),
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
                let info = self.documents.info(id).map_err(document_error)?;
                Ok(ResponseValue::DocumentOpened {
                    document_id: id.0,
                    path: info.path.map(|path| path.to_string_lossy().into_owned()),
                    revision: info.revision,
                    dirty: info.dirty,
                })
            }
            RequestMethod::ReadDocument { document_id } => {
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
        events
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
        revision: None,
        root: None,
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
}
