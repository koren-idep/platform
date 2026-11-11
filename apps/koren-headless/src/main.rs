//! Headless demonstration application for the Koren Platform backend.

use koren_commands::{CommandError, CommandRegistry};
use koren_core::Platform;
use koren_protocol::{ErrorCode, ProtocolError, Response, ResponseValue};
use koren_workspace::WorkspaceManager;

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
            "koren-headless {}\n\nUsage:\n  koren-headless --version\n  koren-headless commands list [--json]\n  koren-headless command execute <id> [--json]\n  koren-headless workspace info <path> [--json]",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
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
                    println!("{}\t{:?}", entry.name, entry.kind);
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

fn print_response(request_id: u64, result: Result<ResponseValue, ProtocolError>) {
    let response = Response::new(request_id, result);
    println!(
        "{}",
        serde_json::to_string(&response).expect("protocol response serializes")
    );
}
