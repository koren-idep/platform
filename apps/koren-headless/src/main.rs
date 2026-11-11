//! Headless demonstration application for the Koren Platform backend.

use koren_cargo::{CargoProjectMetadata, CargoProvider, CargoTargetKind};
use koren_commands::{CommandError, CommandRegistry};
use koren_core::Platform;
use koren_documents::{DocumentError, DocumentId, DocumentManager};
use koren_projects::{
    DiscoveryLimits, MarkerFileProvider, ProjectId, ProjectManager, ProjectSpec, ProviderId,
};
use koren_protocol::{
    CargoPackageSummary, CargoProjectSummary, CargoTargetSummary, DirectoryEntry,
    DirectoryEntryKind, ErrorCode, ExecutionApprovalSummary, ProjectLifecycle, ProjectSummary,
    ProtocolError, Request, RequestMethod, Response, ResponseValue, TaskLifecycle, TaskSummary,
    validate_version,
};
use koren_tasks::{
    ExecutionRequestId, TaskApprovalManager, TaskAuthorizationDecision, TaskAuthorizer, TaskCaller,
    TaskCategory, TaskDefinition, TaskError, TaskExecutionPolicy, TaskExecutionRequest,
    TaskExecutionRequestOutcome, TaskId, TaskManager, TaskOptions, TaskRunId,
};
use koren_workspace::{EntryKind, WorkspaceError, WorkspaceManager};
use std::io::{BufRead, Write};

fn ping(args: &[String]) -> Result<String, CommandError> {
    if !args.is_empty() {
        return Err(CommandError::Execution(
            "core.ping takes no arguments".into(),
        ));
    }
    Ok("pong".into())
}

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if let Err(message) = run(&args) {
        eprintln!("error: {message}");
        std::process::exit(2);
    }
}

fn run(args: &[String]) -> Result<(), String> {
    if args == ["--version"] || args == ["-V"] {
        println!("koren-headless {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "koren-headless {}\n\nUsage:\n  koren-headless --version\n  koren-headless commands list [--json]\n  koren-headless command execute <id> [--json]\n  koren-headless workspace info <path> [--json]\n  koren-headless session --stdio [--allow-task-execution]",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    if args.len() >= 2 && args[0] == "session" && args[1] == "--stdio" && args.len() <= 3 {
        let allow_tasks = args
            .get(2)
            .is_some_and(|arg| arg == "--allow-task-execution");
        if args.len() == 3 && !allow_tasks {
            return Err("unknown session option".into());
        }
        return run_session(
            std::io::stdin().lock(),
            std::io::stdout().lock(),
            allow_tasks,
        );
    }
    let json = args.last().is_some_and(|arg| arg == "--json");
    let args = if json { &args[..args.len() - 1] } else { args };
    let mut platform = Platform::new();
    platform.initialize().map_err(|error| error.to_string())?;
    let result = match args {
        [group, action] if group == "commands" && action == "list" => {
            let registry = registry()?;
            let ids = registry
                .ids()
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if json {
                print_response(1, Ok(ResponseValue::Commands(ids)));
            } else {
                for id in ids {
                    println!("{id}\t{}", registry.description(&id).unwrap_or(""));
                }
            }
            Ok(())
        }
        [group, action, id] if group == "command" && action == "execute" => {
            let registry = registry()?;
            match registry.invoke(id, &[]) {
                Ok(output) => {
                    if json {
                        print_response(2, Ok(ResponseValue::CommandOutput(output)));
                    } else {
                        println!("{output}");
                    }
                    Ok(())
                }
                Err(error) => {
                    if json {
                        print_response(2, Err(command_protocol_error(&error)));
                    }
                    Err(error.to_string())
                }
            }
        }
        [group, action, path] if group == "workspace" && action == "info" => {
            let mut workspace = WorkspaceManager::new();
            workspace.open(path).map_err(|error| error.to_string())?;
            let root = workspace
                .root()
                .map_err(|error| error.to_string())?
                .display()
                .to_string();
            let entries = workspace.entries().map_err(|error| error.to_string())?;
            if json {
                print_response(
                    3,
                    Ok(ResponseValue::Workspace {
                        root,
                        entries: entries.len(),
                    }),
                );
            } else {
                println!("root: {root}");
                println!("entries: {}", entries.len());
                for entry in entries {
                    println!("{}\t{:?}", entry.name.to_string_lossy(), entry.kind);
                }
            }
            Ok(())
        }
        _ => Err("invalid arguments; run 'koren-headless --help'".into()),
    };
    let shutdown_result = platform.shutdown().map_err(|error| error.to_string());
    result.and(shutdown_result)
}

fn registry() -> Result<CommandRegistry, String> {
    let mut registry = CommandRegistry::new();
    registry
        .register("core.ping", "Check that the platform is running", ping)
        .map_err(|error| error.to_string())?;
    Ok(registry)
}

fn command_protocol_error(error: &CommandError) -> ProtocolError {
    let code = match error {
        CommandError::UnknownCommand(_) => ErrorCode::NotFound,
        _ => ErrorCode::InvalidRequest,
    };
    ProtocolError {
        code,
        message: error.to_string(),
    }
}

fn run_session<R: BufRead, W: Write>(
    mut input: R,
    mut output: W,
    allow_tasks: bool,
) -> Result<(), String> {
    let mut platform = Platform::new();
    platform.initialize().map_err(|error| error.to_string())?;
    let mut workspace = WorkspaceManager::new();
    let mut documents = DocumentManager::new();
    let mut projects = ProjectManager::new();
    let cargo_provider = CargoProvider::new();
    cargo_provider
        .register(&mut projects)
        .map_err(|error| error.to_string())?;
    let mut tasks = TaskManager::new(TaskOptions::default()).map_err(|e| e.to_string())?;
    let mut approvals = TaskApprovalManager::new();
    let authorizer = SessionTaskAuthorizer {
        enabled: allow_tasks,
    };
    let mut line = String::new();
    loop {
        line.clear();
        let count = input
            .read_line(&mut line)
            .map_err(|error| format!("failed to read session input: {error}"))?;
        if count == 0 {
            break;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => {
                let mut session = SessionContext {
                    workspace: &mut workspace,
                    documents: &mut documents,
                    projects: &mut projects,
                    tasks: &mut tasks,
                    approvals: &mut approvals,
                    authorizer: &authorizer,
                    cargo_provider: &cargo_provider,
                };
                handle_request(request, &mut session)
            }
            Err(error) => Response::new(
                0,
                Err(ProtocolError {
                    code: ErrorCode::InvalidRequest,
                    message: format!("invalid request JSON: {error}"),
                }),
            ),
        };
        serde_json::to_writer(&mut output, &response)
            .map_err(|error| format!("failed to encode session response: {error}"))?;
        output
            .write_all(b"\n")
            .map_err(|error| format!("failed to write session response: {error}"))?;
        output
            .flush()
            .map_err(|error| format!("failed to flush session response: {error}"))?;
    }
    approvals.shutdown();
    tasks.shutdown();
    if let Ok(root) = workspace.root() {
        if documents.has_documents_for_workspace(root) {
            return Err(
                "session ended with open documents; close them before ending the session".into(),
            );
        }
        projects.close_workspace(root);
        workspace.close().map_err(|error| error.to_string())?;
    }
    platform.shutdown().map_err(|error| error.to_string())
}

struct SessionContext<'a> {
    workspace: &'a mut WorkspaceManager,
    documents: &'a mut DocumentManager,
    projects: &'a mut ProjectManager,
    tasks: &'a mut TaskManager,
    approvals: &'a mut TaskApprovalManager,
    authorizer: &'a SessionTaskAuthorizer,
    cargo_provider: &'a CargoProvider,
}

fn handle_request(request: Request, session: &mut SessionContext<'_>) -> Response {
    let request_id = request.request_id;
    if let Err(error) = validate_version(request.version) {
        return Response::new(request_id, Err(error));
    }
    let result = handle_method(request.method, session);
    Response::new(request_id, result)
}

fn handle_method(
    method: RequestMethod,
    session: &mut SessionContext<'_>,
) -> Result<ResponseValue, ProtocolError> {
    match method {
        RequestMethod::Ping => Ok(ResponseValue::Pong),
        RequestMethod::ListCommands => registry()
            .map(|registry| {
                ResponseValue::Commands(registry.ids().into_iter().map(str::to_owned).collect())
            })
            .map_err(internal_error),
        RequestMethod::ExecuteCommand { command_id } => registry()
            .map_err(internal_error)
            .and_then(|registry| {
                registry
                    .invoke(&command_id, &[])
                    .map_err(|error| command_protocol_error(&error))
            })
            .map(ResponseValue::CommandOutput),
        RequestMethod::WorkspaceInfo { path } => {
            let mut temporary = WorkspaceManager::new();
            temporary
                .open(path)
                .map_err(|error| workspace_error(&error))
                .and_then(|()| {
                    let root = temporary
                        .root()
                        .map_err(|error| workspace_error(&error))?
                        .display()
                        .to_string();
                    let entries = temporary
                        .entries()
                        .map_err(|error| workspace_error(&error))?
                        .len();
                    Ok(ResponseValue::Workspace { root, entries })
                })
        }
        RequestMethod::OpenWorkspace { path } => open_workspace(session.workspace, path),
        RequestMethod::CloseWorkspace => close_workspace(
            session.workspace,
            session.documents,
            session.projects,
            session.tasks,
            session.approvals,
        ),
        RequestMethod::ListDirectory { path } => list_directory(session.workspace, path),
        method @ (RequestMethod::OpenDocument { .. }
        | RequestMethod::ReadDocument { .. }
        | RequestMethod::ApplyDocumentEdits { .. }
        | RequestMethod::SaveDocument { .. }
        | RequestMethod::ResolveDocumentConflict { .. }
        | RequestMethod::CloseDocument { .. }) => {
            handle_document_method(method, session.workspace, session.documents)
        }
        method => handle_project_task_method(
            method,
            session.workspace,
            session.projects,
            session.tasks,
            session.approvals,
            session.authorizer,
            session.cargo_provider,
        ),
    }
}

fn open_workspace(
    workspace: &mut WorkspaceManager,
    path: String,
) -> Result<ResponseValue, ProtocolError> {
    workspace
        .open(path)
        .map_err(|error| workspace_error(&error))?;
    let root = workspace
        .root()
        .map_err(|error| workspace_error(&error))?
        .display()
        .to_string();
    Ok(ResponseValue::WorkspaceOpened { root })
}

fn close_workspace(
    workspace: &mut WorkspaceManager,
    documents: &DocumentManager,
    projects: &mut ProjectManager,
    tasks: &TaskManager,
    approvals: &mut TaskApprovalManager,
) -> Result<ResponseValue, ProtocolError> {
    let root = workspace.root().map_err(|error| workspace_error(&error))?;
    if documents.has_documents_for_workspace(root) {
        return Err(ProtocolError {
            code: ErrorCode::InvalidRequest,
            message: "close workspace documents before closing the workspace".into(),
        });
    }
    let root = root.display().to_string();
    let root_path = workspace
        .root()
        .map_err(|error| workspace_error(&error))?
        .to_path_buf();
    approvals.invalidate_workspace(&root_path);
    projects.close_workspace(&root_path);
    tasks.shutdown();
    workspace.close().map_err(|error| workspace_error(&error))?;
    Ok(ResponseValue::WorkspaceClosed { root })
}

struct SessionTaskAuthorizer {
    enabled: bool,
}
impl TaskAuthorizer for SessionTaskAuthorizer {
    fn authorize(&self, caller: &TaskCaller, _task: &TaskDefinition) -> Result<(), TaskError> {
        if self.enabled && matches!(caller, TaskCaller::InteractiveUser) {
            Ok(())
        } else {
            Err(TaskError::Unauthorized("start the session with --allow-task-execution after reviewing its task definitions".into()))
        }
    }
}
impl TaskExecutionPolicy for SessionTaskAuthorizer {
    fn decide(&self, _request: &koren_tasks::TaskExecutionSnapshot) -> TaskAuthorizationDecision {
        if self.enabled {
            TaskAuthorizationDecision::Allow
        } else {
            TaskAuthorizationDecision::RequireApproval
        }
    }
}

fn handle_project_task_method(
    method: RequestMethod,
    workspace: &WorkspaceManager,
    projects: &mut ProjectManager,
    tasks: &mut TaskManager,
    approvals: &mut TaskApprovalManager,
    authorizer: &SessionTaskAuthorizer,
    cargo_provider: &CargoProvider,
) -> Result<ResponseValue, ProtocolError> {
    match method {
        request @ (RequestMethod::RegisterProject { .. }
        | RequestMethod::ListProjects
        | RequestMethod::DiscoverProjects { .. }
        | RequestMethod::ProjectInfo { .. }
        | RequestMethod::UnregisterProject { .. }) => {
            handle_project_method(request, workspace, projects, approvals)
        }
        request @ (RequestMethod::DiscoverCargoProjects { .. }
        | RequestMethod::CargoProjectInfo { .. }) => {
            handle_cargo_project_method(&request, workspace, projects, tasks, cargo_provider)
        }
        request => handle_task_method(request, workspace, projects, tasks, approvals, authorizer),
    }
}

fn handle_project_method(
    method: RequestMethod,
    workspace: &WorkspaceManager,
    projects: &mut ProjectManager,
    approvals: &mut TaskApprovalManager,
) -> Result<ResponseValue, ProtocolError> {
    match method {
        RequestMethod::RegisterProject {
            display_name,
            root,
            provider_id,
            configuration_path,
        } => {
            let provider_id =
                ProviderId::new(provider_id).map_err(|e| project_protocol_error(e.to_string()))?;
            let id = projects
                .register(
                    workspace,
                    ProjectSpec {
                        display_name,
                        root: root.into(),
                        provider_id,
                        configuration_path: configuration_path.map(Into::into),
                        provider_data: std::collections::BTreeMap::new(),
                    },
                )
                .map_err(|e| project_protocol_error(e.to_string()))?;
            project_summary(
                projects
                    .info(id)
                    .map_err(|e| project_protocol_error(e.to_string()))?,
            )
            .map(ResponseValue::Project)
        }
        RequestMethod::ListProjects => projects
            .list()
            .into_iter()
            .map(project_summary)
            .collect::<Result<Vec<_>, _>>()
            .map(ResponseValue::Projects),
        RequestMethod::DiscoverProjects {
            provider_id,
            marker_filename,
            max_directories,
            max_projects,
            max_depth,
        } => {
            let provider_id =
                ProviderId::new(provider_id).map_err(|e| project_protocol_error(e.to_string()))?;
            projects
                .register_provider(Box::new(MarkerFileProvider::new(
                    provider_id.clone(),
                    marker_filename,
                )))
                .map_err(|e| project_protocol_error(e.to_string()))?;
            projects
                .discover_registered(
                    workspace,
                    &provider_id,
                    DiscoveryLimits {
                        max_directories,
                        max_projects,
                        max_depth,
                    },
                )
                .map_err(|e| project_protocol_error(e.to_string()))?;
            projects
                .list()
                .into_iter()
                .filter(|p| p.provider_id == provider_id)
                .map(project_summary)
                .collect::<Result<Vec<_>, _>>()
                .map(ResponseValue::Projects)
        }
        RequestMethod::ProjectInfo { project_id } => project_summary(
            projects
                .info(ProjectId::from_raw(project_id))
                .map_err(|e| project_protocol_error(e.to_string()))?,
        )
        .map(ResponseValue::Project),
        RequestMethod::UnregisterProject { project_id } => {
            projects
                .info(ProjectId::from_raw(project_id))
                .map_err(|e| project_protocol_error(e.to_string()))?;
            approvals.invalidate_project(project_id);
            projects
                .unregister(ProjectId::from_raw(project_id))
                .map_err(|e| project_protocol_error(e.to_string()))?;
            Ok(ResponseValue::ProjectClosed { project_id })
        }
        _ => Err(ProtocolError {
            code: ErrorCode::InvalidRequest,
            message: "request is not a project operation".into(),
        }),
    }
}

fn handle_cargo_project_method(
    method: &RequestMethod,
    workspace: &WorkspaceManager,
    projects: &mut ProjectManager,
    tasks: &mut TaskManager,
    cargo_provider: &CargoProvider,
) -> Result<ResponseValue, ProtocolError> {
    match method {
        RequestMethod::DiscoverCargoProjects {
            max_directories,
            max_projects,
            max_depth,
        } => {
            let found = cargo_provider
                .discover(
                    workspace,
                    projects,
                    DiscoveryLimits {
                        max_directories: *max_directories,
                        max_projects: *max_projects,
                        max_depth: *max_depth,
                    },
                )
                .map_err(|error| cargo_protocol_error(error.to_string()))?;
            for record in &found {
                let info = projects
                    .info(record.project_id)
                    .map_err(|error| project_protocol_error(error.to_string()))?;
                for definition in cargo_provider.tasks_for_project(info) {
                    tasks.register(definition).map_err(task_protocol_error)?;
                }
            }
            found
                .iter()
                .map(|record| {
                    let project = projects
                        .info(record.project_id)
                        .map_err(|error| project_protocol_error(error.to_string()))?;
                    cargo_project_summary(project, &record.metadata)
                })
                .collect::<Result<Vec<_>, _>>()
                .map(ResponseValue::CargoProjects)
        }
        RequestMethod::CargoProjectInfo { project_id } => {
            let project = projects
                .info(ProjectId::from_raw(*project_id))
                .map_err(|error| project_protocol_error(error.to_string()))?;
            let metadata = cargo_provider
                .project_metadata(project)
                .map_err(|error| cargo_protocol_error(error.to_string()))?;
            cargo_project_summary(project, &metadata).map(ResponseValue::CargoProject)
        }
        _ => Err(ProtocolError {
            code: ErrorCode::InvalidRequest,
            message: "request is not a Cargo operation".into(),
        }),
    }
}

fn handle_task_method(
    method: RequestMethod,
    workspace: &WorkspaceManager,
    projects: &ProjectManager,
    tasks: &mut TaskManager,
    approvals: &mut TaskApprovalManager,
    authorizer: &SessionTaskAuthorizer,
) -> Result<ResponseValue, ProtocolError> {
    match method {
        request @ RequestMethod::RegisterTask { .. } => {
            register_task_method(request, workspace, projects, tasks)
        }
        RequestMethod::ListTasks => Ok(ResponseValue::Tasks(
            tasks
                .list()
                .into_iter()
                .map(|(id, t)| TaskSummary {
                    task_id: id.get(),
                    key: t.key.clone(),
                    display_name: t.display_name.clone(),
                    category: format!("{:?}", t.category).to_lowercase(),
                    executable: t.executable.to_string_lossy().into_owned(),
                    arguments: t.arguments.clone(),
                    working_directory: t.working_directory.to_string_lossy().into_owned(),
                    timeout_ms: t
                        .timeout
                        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
                    project_id: t.project_id,
                })
                .collect(),
        )),
        RequestMethod::StartTask { task_id } => {
            ensure_task_scope(workspace, projects, tasks, TaskId::from_raw(task_id))?;
            tasks
                .start(
                    TaskId::from_raw(task_id),
                    &TaskCaller::InteractiveUser,
                    authorizer,
                )
                .map(|id| ResponseValue::TaskStarted { run_id: id.get() })
                .map_err(task_protocol_error)
        }
        request @ (RequestMethod::RequestTaskExecution { .. }
        | RequestMethod::ListPendingApprovals
        | RequestMethod::ApproveTaskExecution { .. }
        | RequestMethod::DenyTaskExecution { .. }) => {
            handle_approval_method(&request, workspace, projects, tasks, approvals, authorizer)
        }
        RequestMethod::TaskStatus { run_id } | RequestMethod::TaskOutput { run_id } => tasks
            .output(TaskRunId::from_raw(run_id))
            .map(task_snapshot_response)
            .map_err(task_protocol_error),
        RequestMethod::CancelTask { run_id } => tasks
            .cancel(TaskRunId::from_raw(run_id))
            .map(|()| ResponseValue::TaskCancelled { run_id })
            .map_err(task_protocol_error),
        RequestMethod::CleanupTask { run_id } => tasks
            .cleanup(TaskRunId::from_raw(run_id))
            .map(|()| ResponseValue::TaskCleaned { run_id })
            .map_err(task_protocol_error),
        _ => Err(ProtocolError {
            code: ErrorCode::InvalidRequest,
            message: "request is not a task operation".into(),
        }),
    }
}

fn handle_approval_method(
    method: &RequestMethod,
    workspace: &WorkspaceManager,
    projects: &ProjectManager,
    tasks: &mut TaskManager,
    approvals: &mut TaskApprovalManager,
    authorizer: &SessionTaskAuthorizer,
) -> Result<ResponseValue, ProtocolError> {
    match method {
        RequestMethod::RequestTaskExecution { task_id } => {
            let task_id = TaskId::from_raw(*task_id);
            ensure_task_scope(workspace, projects, tasks, task_id)?;
            let workspace_root = workspace
                .root()
                .map_err(|error| workspace_error(&error))?
                .to_path_buf();
            match approvals
                .request_execution(
                    tasks,
                    task_id,
                    TaskCaller::InteractiveUser,
                    workspace_root,
                    authorizer,
                    std::time::Duration::from_secs(120),
                )
                .map_err(task_protocol_error)?
            {
                TaskExecutionRequestOutcome::Started(run_id) => Ok(ResponseValue::TaskStarted {
                    run_id: run_id.get(),
                }),
                TaskExecutionRequestOutcome::ApprovalRequired(request) => {
                    approval_summary(&request, approvals).map(ResponseValue::ApprovalRequired)
                }
            }
        }
        RequestMethod::ListPendingApprovals => {
            let pending = approvals
                .pending_requests()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            pending
                .into_iter()
                .map(|request| approval_summary(&request, approvals))
                .collect::<Result<Vec<_>, _>>()
                .map(ResponseValue::PendingApprovals)
        }
        RequestMethod::ApproveTaskExecution { request_id } => {
            let id = ExecutionRequestId::from_raw(*request_id);
            let scope = approvals
                .pending_requests()
                .into_iter()
                .find(|request| request.id == id)
                .map(|request| {
                    (
                        request.snapshot.workspace_root.clone(),
                        request.snapshot.definition.project_id,
                    )
                })
                .ok_or_else(|| task_protocol_error(TaskError::ApprovalNotFound))?;
            if scope.0 != *workspace.root().map_err(|error| workspace_error(&error))? {
                approvals.invalidate_workspace(&scope.0);
                return Err(task_protocol_error(TaskError::ApprovalInvalidated));
            }
            if let Some(project_id) = scope.1 {
                if projects.info(ProjectId::from_raw(project_id)).is_err() {
                    approvals.invalidate_project(project_id);
                    return Err(task_protocol_error(TaskError::ApprovalInvalidated));
                }
            }
            approvals
                .approve(tasks, id)
                .map(|run_id| ResponseValue::TaskApproved {
                    request_id: *request_id,
                    run_id: run_id.get(),
                })
                .map_err(task_protocol_error)
        }
        RequestMethod::DenyTaskExecution { request_id } => approvals
            .deny(ExecutionRequestId::from_raw(*request_id))
            .map(|()| ResponseValue::TaskDenied {
                request_id: *request_id,
            })
            .map_err(task_protocol_error),
        _ => Err(ProtocolError {
            code: ErrorCode::InvalidRequest,
            message: "request is not an approval operation".into(),
        }),
    }
}

fn register_task_method(
    method: RequestMethod,
    workspace: &WorkspaceManager,
    projects: &ProjectManager,
    tasks: &mut TaskManager,
) -> Result<ResponseValue, ProtocolError> {
    let RequestMethod::RegisterTask {
        key,
        display_name,
        category,
        executable,
        arguments,
        working_directory,
        environment,
        timeout_ms,
        project_id,
    } = method
    else {
        return Err(ProtocolError {
            code: ErrorCode::InvalidRequest,
            message: "request is not task registration".into(),
        });
    };
    let category = parse_task_category(&category)?;
    if !working_directory.is_empty() {
        workspace
            .list_dir(&working_directory)
            .map_err(|e| workspace_error(&e))?;
    }
    if let Some(id) = project_id {
        projects
            .info(ProjectId::from_raw(id))
            .map_err(|e| project_protocol_error(e.to_string()))?;
    }
    let workspace_root = workspace.root().map_err(|e| workspace_error(&e))?;
    let task = TaskDefinition {
        key,
        display_name,
        category,
        executable: executable.into(),
        arguments,
        working_directory: if working_directory.is_empty() {
            workspace_root.to_path_buf()
        } else {
            workspace_root.join(working_directory)
        },
        environment,
        timeout: timeout_ms.map(std::time::Duration::from_millis),
        project_id,
    };
    tasks
        .register(task)
        .map(|id| ResponseValue::TaskRegistered { task_id: id.get() })
        .map_err(task_protocol_error)
}

fn task_snapshot_response(snapshot: koren_tasks::TaskSnapshot) -> ResponseValue {
    let (state, exit_code, error) = match snapshot.state {
        koren_tasks::TaskState::Pending => (TaskLifecycle::Pending, None, None),
        koren_tasks::TaskState::Running => (TaskLifecycle::Running, None, None),
        koren_tasks::TaskState::Succeeded { exit_code } => {
            (TaskLifecycle::Succeeded, Some(exit_code), None)
        }
        koren_tasks::TaskState::Failed { exit_code } => (TaskLifecycle::Failed, exit_code, None),
        koren_tasks::TaskState::Cancelled => (TaskLifecycle::Cancelled, None, None),
        koren_tasks::TaskState::TimedOut => (TaskLifecycle::TimedOut, None, None),
        koren_tasks::TaskState::SpawnFailed { message } => {
            (TaskLifecycle::SpawnFailed, None, Some(message))
        }
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

fn ensure_task_scope(
    workspace: &WorkspaceManager,
    projects: &ProjectManager,
    tasks: &TaskManager,
    task_id: TaskId,
) -> Result<(), ProtocolError> {
    let root = workspace.root().map_err(|error| workspace_error(&error))?;
    let (_, definition) = tasks
        .list()
        .into_iter()
        .find(|(id, _)| *id == task_id)
        .ok_or_else(|| task_protocol_error(TaskError::NotFound))?;
    if !definition.working_directory.starts_with(root) {
        return Err(ProtocolError {
            code: ErrorCode::InvalidRequest,
            message: "task is outside the active workspace".into(),
        });
    }
    if let Some(project_id) = definition.project_id {
        let project = projects
            .info(ProjectId::from_raw(project_id))
            .map_err(|error| project_protocol_error(error.to_string()))?;
        if project.workspace_root != root
            || project.state != koren_projects::ProjectState::Registered
        {
            return Err(ProtocolError {
                code: ErrorCode::InvalidRequest,
                message: "task's project is not active in this workspace".into(),
            });
        }
    }
    Ok(())
}

fn approval_summary(
    request: &TaskExecutionRequest,
    approvals: &TaskApprovalManager,
) -> Result<ExecutionApprovalSummary, ProtocolError> {
    let definition = &request.snapshot.definition;
    let remaining = approvals
        .remaining_lifetime(request.id)
        .map_err(task_protocol_error)?;
    let requesting_actor = match &request.snapshot.caller {
        TaskCaller::InteractiveUser => "interactive_user".into(),
        TaskCaller::Extension(id) => format!("extension:{id}"),
        TaskCaller::CodingAgent(id) => format!("coding_agent:{id}"),
    };
    Ok(ExecutionApprovalSummary {
        request_id: request.id.get(),
        task_id: request.snapshot.task_id.get(),
        task_key: definition.key.clone(),
        display_name: definition.display_name.clone(),
        executable: definition.executable.to_string_lossy().into_owned(),
        arguments: definition.arguments.clone(),
        working_directory: definition.working_directory.to_string_lossy().into_owned(),
        environment_keys: definition.environment.keys().cloned().collect(),
        project_id: definition.project_id,
        workspace_root: request
            .snapshot
            .workspace_root
            .to_string_lossy()
            .into_owned(),
        requesting_actor,
        expires_in_ms: u64::try_from(remaining.as_millis())
            .unwrap_or(u64::MAX)
            .max(1),
    })
}

fn cargo_project_summary(
    project: &koren_projects::ProjectInfo,
    metadata: &CargoProjectMetadata,
) -> Result<CargoProjectSummary, ProtocolError> {
    Ok(CargoProjectSummary {
        project: project_summary(project)?,
        is_workspace: metadata.is_workspace,
        package: metadata
            .package
            .as_ref()
            .map(cargo_package_summary)
            .transpose()?,
        members: metadata
            .members
            .iter()
            .map(cargo_package_summary)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn cargo_package_summary(
    package: &koren_cargo::CargoPackageMetadata,
) -> Result<CargoPackageSummary, ProtocolError> {
    let manifest_path = package
        .manifest_path
        .to_str()
        .ok_or_else(|| ProtocolError {
            code: ErrorCode::InvalidPath,
            message: "Cargo manifest path is not valid UTF-8".into(),
        })?
        .to_owned();
    let targets = package
        .targets
        .iter()
        .map(|target| {
            Ok(CargoTargetSummary {
                name: target.name.clone(),
                kind: match target.kind {
                    CargoTargetKind::Library => "library",
                    CargoTargetKind::Binary => "binary",
                    CargoTargetKind::Example => "example",
                    CargoTargetKind::Test => "test",
                    CargoTargetKind::Benchmark => "benchmark",
                    CargoTargetKind::Other => "other",
                }
                .into(),
                source_path: target
                    .source_path
                    .as_ref()
                    .map(|path| {
                        path.to_str()
                            .map(str::to_owned)
                            .ok_or_else(|| ProtocolError {
                                code: ErrorCode::InvalidPath,
                                message: "Cargo target source path is not valid UTF-8".into(),
                            })
                    })
                    .transpose()?,
            })
        })
        .collect::<Result<Vec<_>, ProtocolError>>()?;
    Ok(CargoPackageSummary {
        name: package.name.clone(),
        version: package.version.clone(),
        manifest_path,
        targets,
    })
}

fn project_summary(info: &koren_projects::ProjectInfo) -> Result<ProjectSummary, ProtocolError> {
    let root = info
        .root
        .to_str()
        .ok_or_else(|| ProtocolError {
            code: ErrorCode::InvalidPath,
            message: "project path is not valid UTF-8".into(),
        })?
        .to_owned();
    let configuration_path = info
        .configuration_path
        .as_ref()
        .map(|p| {
            p.to_str().map(str::to_owned).ok_or_else(|| ProtocolError {
                code: ErrorCode::InvalidPath,
                message: "configuration path is not valid UTF-8".into(),
            })
        })
        .transpose()?;
    Ok(ProjectSummary {
        project_id: info.id.get(),
        display_name: info.display_name.clone(),
        root,
        provider_id: info.provider_id.as_str().to_owned(),
        configuration_path,
        state: match info.state {
            koren_projects::ProjectState::Registered => ProjectLifecycle::Registered,
            koren_projects::ProjectState::Closing => ProjectLifecycle::Closing,
        },
    })
}
fn parse_task_category(value: &str) -> Result<TaskCategory, ProtocolError> {
    match value {
        "build" => Ok(TaskCategory::Build),
        "check" => Ok(TaskCategory::Check),
        "test" => Ok(TaskCategory::Test),
        "clean" => Ok(TaskCategory::Clean),
        "run" => Ok(TaskCategory::Run),
        "custom" => Ok(TaskCategory::Custom),
        _ => Err(ProtocolError {
            code: ErrorCode::InvalidRequest,
            message: "unknown task category".into(),
        }),
    }
}
fn project_protocol_error(message: String) -> ProtocolError {
    ProtocolError {
        code: if message.contains("not found") {
            ErrorCode::NotFound
        } else {
            ErrorCode::InvalidRequest
        },
        message,
    }
}
fn task_protocol_error(error: TaskError) -> ProtocolError {
    let code = match &error {
        TaskError::NotFound | TaskError::ApprovalNotFound => ErrorCode::NotFound,
        TaskError::Unauthorized(_) => ErrorCode::Unauthorized,
        TaskError::Invalid(_) | TaskError::ApprovalExpired | TaskError::ApprovalInvalidated => {
            ErrorCode::InvalidRequest
        }
        TaskError::Io(_) => ErrorCode::Internal,
    };
    let message = error.to_string();
    drop(error);
    ProtocolError { code, message }
}

fn cargo_protocol_error(message: String) -> ProtocolError {
    ProtocolError {
        code: if message.contains("not found") {
            ErrorCode::NotFound
        } else {
            ErrorCode::InvalidRequest
        },
        message,
    }
}

fn list_directory(
    workspace: &WorkspaceManager,
    path: String,
) -> Result<ResponseValue, ProtocolError> {
    let entries = workspace
        .list_dir(path)
        .map_err(|error| workspace_error(&error))?;
    let entries = entries
        .into_iter()
        .map(|entry| {
            let name = entry.name.into_string().map_err(|_| ProtocolError {
                code: ErrorCode::InvalidPath,
                message: "directory contains a filename that is not valid UTF-8".into(),
            })?;
            let path = entry
                .path
                .into_os_string()
                .into_string()
                .map_err(|_| ProtocolError {
                    code: ErrorCode::InvalidPath,
                    message: "directory contains a path that is not valid UTF-8".into(),
                })?;
            let kind = match entry.kind {
                EntryKind::File => DirectoryEntryKind::File,
                EntryKind::Directory => DirectoryEntryKind::Directory,
                EntryKind::Symlink => DirectoryEntryKind::Symlink,
                EntryKind::Other => DirectoryEntryKind::Other,
            };
            Ok(DirectoryEntry {
                name,
                path,
                kind,
                size_bytes: entry.size_bytes,
            })
        })
        .collect::<Result<Vec<_>, ProtocolError>>()?;
    Ok(ResponseValue::DirectoryEntries(entries))
}

fn handle_document_method(
    method: RequestMethod,
    workspace: &WorkspaceManager,
    documents: &mut DocumentManager,
) -> Result<ResponseValue, ProtocolError> {
    match method {
        RequestMethod::OpenDocument { path } => {
            let id = documents
                .open_file(workspace, path)
                .map_err(|error| document_error(&error))?;
            document_opened(documents.info(id).map_err(|error| document_error(&error))?)
        }
        RequestMethod::ReadDocument { document_id } => {
            let snapshot = documents
                .snapshot(DocumentId(document_id))
                .map_err(|error| document_error(&error))?;
            Ok(ResponseValue::DocumentContents {
                document_id,
                contents: snapshot.text,
                revision: snapshot.info.revision,
                dirty: snapshot.info.dirty,
            })
        }
        RequestMethod::ApplyDocumentEdits {
            document_id,
            expected_revision,
            edits,
        } => {
            let info = documents
                .apply_edits(DocumentId(document_id), expected_revision, &edits)
                .map_err(|error| document_error(&error))?;
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
            let info = documents
                .save(DocumentId(document_id), expected_revision)
                .map_err(|error| document_error(&error))?;
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
            let info = documents
                .resolve_conflict(DocumentId(document_id), expected_revision, decision)
                .map_err(|error| document_error(&error))?;
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
            documents
                .close(DocumentId(document_id), discard_unsaved_changes)
                .map_err(|error| document_error(&error))?;
            Ok(ResponseValue::DocumentClosed { document_id })
        }
        _ => Err(ProtocolError {
            code: ErrorCode::InvalidRequest,
            message: "request is not a document operation".into(),
        }),
    }
}

fn document_opened(info: koren_documents::DocumentInfo) -> Result<ResponseValue, ProtocolError> {
    let path = info
        .path
        .map(|path| {
            path.into_os_string()
                .into_string()
                .map_err(|_| ProtocolError {
                    code: ErrorCode::InvalidPath,
                    message: "document path is not valid UTF-8".into(),
                })
        })
        .transpose()?;
    Ok(ResponseValue::DocumentOpened {
        document_id: info.id.0,
        path,
        revision: info.revision,
        dirty: info.dirty,
    })
}

fn workspace_error(error: &WorkspaceError) -> ProtocolError {
    let code = match error {
        WorkspaceError::NotOpen => ErrorCode::NotOpen,
        WorkspaceError::InvalidRelativePath(_)
        | WorkspaceError::RootIsSymlink(_)
        | WorkspaceError::SymlinkNotAllowed(_)
        | WorkspaceError::NotDirectory(_)
        | WorkspaceError::NotFile(_)
        | WorkspaceError::InvalidLimit => ErrorCode::InvalidPath,
        WorkspaceError::NotFound(_) => ErrorCode::NotFound,
        WorkspaceError::AlreadyOpen | WorkspaceError::DirectoryLimitExceeded { .. } => {
            ErrorCode::InvalidRequest
        }
        WorkspaceError::Io { .. } => ErrorCode::Internal,
    };
    ProtocolError {
        code,
        message: error.to_string(),
    }
}

fn document_error(error: &DocumentError) -> ProtocolError {
    let code = match &error {
        DocumentError::Conflict(_) | DocumentError::ConflictChangedAgain(_) => ErrorCode::Conflict,
        DocumentError::Workspace(error) => return workspace_error(error),
        DocumentError::NotOpen(_) => ErrorCode::NotOpen,
        DocumentError::ExternalFileMissing(_) => ErrorCode::NotFound,
        DocumentError::NoBackingFile(_)
        | DocumentError::NoPendingConflict(_)
        | DocumentError::UnsavedChanges(_)
        | DocumentError::InvalidUtf8(_)
        | DocumentError::BinaryFile(_)
        | DocumentError::TooLarge { .. }
        | DocumentError::InvalidPosition(_)
        | DocumentError::StaleRevision { .. }
        | DocumentError::OverlappingEdits => ErrorCode::InvalidRequest,
        DocumentError::RevisionExhausted
        | DocumentError::ChangedDuringRead(_)
        | DocumentError::Io { .. } => ErrorCode::Internal,
    };
    ProtocolError {
        code,
        message: error.to_string(),
    }
}

fn internal_error(message: String) -> ProtocolError {
    ProtocolError {
        code: ErrorCode::Internal,
        message,
    }
}

fn print_response(request_id: u64, result: Result<ResponseValue, ProtocolError>) {
    let response = Response::new(request_id, result);
    println!(
        "{}",
        serde_json::to_string(&response).expect("protocol response serializes")
    );
}
