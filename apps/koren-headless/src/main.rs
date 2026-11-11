//! Headless demonstration application for the Koren Platform backend.

use koren_commands::{CommandError, CommandRegistry};
use koren_core::Platform;
use koren_documents::{DocumentError, DocumentId, DocumentManager};
use koren_protocol::{
    DirectoryEntry, DirectoryEntryKind, ErrorCode, ProtocolError, Request, RequestMethod, Response,
    ResponseValue, validate_version,
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
            "koren-headless {}\n\nUsage:\n  koren-headless --version\n  koren-headless commands list [--json]\n  koren-headless command execute <id> [--json]\n  koren-headless workspace info <path> [--json]\n  koren-headless session --stdio",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    if args == ["session", "--stdio"] {
        return run_session(std::io::stdin().lock(), std::io::stdout().lock());
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

fn run_session<R: BufRead, W: Write>(mut input: R, mut output: W) -> Result<(), String> {
    let mut platform = Platform::new();
    platform.initialize().map_err(|error| error.to_string())?;
    let mut workspace = WorkspaceManager::new();
    let mut documents = DocumentManager::new();
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
            Ok(request) => handle_request(request, &mut workspace, &mut documents),
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
    if let Ok(root) = workspace.root() {
        if documents.has_documents_for_workspace(root) {
            return Err(
                "session ended with open documents; close them before ending the session".into(),
            );
        }
        workspace.close().map_err(|error| error.to_string())?;
    }
    platform.shutdown().map_err(|error| error.to_string())
}

fn handle_request(
    request: Request,
    workspace: &mut WorkspaceManager,
    documents: &mut DocumentManager,
) -> Response {
    let request_id = request.request_id;
    if let Err(error) = validate_version(request.version) {
        return Response::new(request_id, Err(error));
    }
    let result = handle_method(request.method, workspace, documents);
    Response::new(request_id, result)
}

fn handle_method(
    method: RequestMethod,
    workspace: &mut WorkspaceManager,
    documents: &mut DocumentManager,
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
        RequestMethod::OpenWorkspace { path } => open_workspace(workspace, path),
        RequestMethod::CloseWorkspace => close_workspace(workspace, documents),
        RequestMethod::ListDirectory { path } => list_directory(workspace, path),
        method @ (RequestMethod::OpenDocument { .. }
        | RequestMethod::ReadDocument { .. }
        | RequestMethod::ApplyDocumentEdits { .. }
        | RequestMethod::SaveDocument { .. }
        | RequestMethod::ResolveDocumentConflict { .. }
        | RequestMethod::CloseDocument { .. }) => {
            handle_document_method(method, workspace, documents)
        }
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
) -> Result<ResponseValue, ProtocolError> {
    let root = workspace.root().map_err(|error| workspace_error(&error))?;
    if documents.has_documents_for_workspace(root) {
        return Err(ProtocolError {
            code: ErrorCode::InvalidRequest,
            message: "close workspace documents before closing the workspace".into(),
        });
    }
    let root = root.display().to_string();
    workspace.close().map_err(|error| workspace_error(&error))?;
    Ok(ResponseValue::WorkspaceClosed { root })
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
