mod bridge;
mod recovery;

use bridge::{BackendSender, DesktopEvent};
#[cfg(target_os = "linux")]
use gtk::prelude::*;
use include_dir::{Dir, include_dir};
use std::{
    cell::RefCell,
    ffi::OsString,
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc,
    thread,
    time::Duration,
};
#[cfg(target_os = "linux")]
use tao::platform::unix::WindowExtUnix;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::WindowBuilder,
};
#[cfg(target_os = "linux")]
use wry::WebViewBuilderExtUnix;
use wry::{
    RequestAsyncResponder, WebViewBuilder,
    http::{Method, Request, Response, StatusCode},
};

static FRONTEND: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/frontend/dist");

enum UiEvent {
    PickWorkspace(RequestAsyncResponder),
    PickFile(RequestAsyncResponder),
    PickerResolved {
        id: u64,
        path: Option<String>,
        responder: RequestAsyncResponder,
    },
    BackendResponse(Box<koren_protocol::Response>),
    RecoveryResponse(serde_json::Value),
    CloseDecision(bool),
    SmokeExit,
}

fn main() {
    let result = workspace_argument(std::env::args_os().skip(1)).and_then(run);
    if let Err(error) = result {
        eprintln!("Koren Code Editor could not start: {error}");
        std::process::exit(1);
    }
}

fn run(workspace: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoopBuilder::<UiEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title("Koren Code Editor")
        .with_inner_size(LogicalSize::new(1260.0, 820.0))
        .with_min_inner_size(LogicalSize::new(680.0, 440.0))
        .build(&event_loop)?;
    let backend = bridge::spawn_backend();
    let proxy_for_protocol = proxy.clone();
    let backend_for_protocol = backend.clone();
    let proxy_for_ipc = proxy.clone();
    let backend_for_ipc = backend.clone();
    let builder = WebViewBuilder::new()
        .with_asynchronous_custom_protocol("koren".into(), move |_id, request, responder| {
            route_request(
                request,
                responder,
                backend_for_protocol.clone(),
                proxy_for_protocol.clone(),
            );
        })
        .with_ipc_handler(move |request| {
            if request.uri().scheme_str() != Some("koren") || request.uri().host() != Some("app") {
                return;
            }
            if request.body().len() > 8 * 1024 * 1024 {
                let request_id = serde_json::from_str::<serde_json::Value>(request.body())
                    .ok()
                    .and_then(|message| {
                        message
                            .get("request_id")
                            .and_then(serde_json::Value::as_u64)
                    });
                if let Some(request_id) = request_id {
                    let response = koren_protocol::Response::new(
                        request_id,
                        Err(koren_protocol::ProtocolError {
                            code: koren_protocol::ErrorCode::InvalidRequest,
                            message: "request body exceeds the 1 MiB bridge limit".into(),
                        }),
                    );
                    let _ = proxy_for_ipc.send_event(UiEvent::BackendResponse(Box::new(response)));
                }
                return;
            }
            let Ok(message) = serde_json::from_str::<serde_json::Value>(request.body()) else {
                return;
            };
            if message.get("type").and_then(serde_json::Value::as_str)
                == Some("recovery_request")
            {
                let request_id = message.get("request_id").and_then(serde_json::Value::as_u64);
                let operation = message.get("operation").and_then(serde_json::Value::as_str).map(str::to_owned);
                let payload = message.get("payload").cloned().unwrap_or(serde_json::Value::Null);
                if let (Some(request_id), Some(operation)) = (request_id, operation) {
                    let proxy = proxy_for_ipc.clone();
                    thread::spawn(move || {
                        let result = recovery_operation(&operation, &payload);
                        let response = match result {
                            Ok(value) => serde_json::json!({"request_id": request_id, "ok": true, "value": value}),
                            Err(error) => serde_json::json!({"request_id": request_id, "ok": false, "error": error}),
                        };
                        let _ = proxy.send_event(UiEvent::RecoveryResponse(response));
                    });
                }
                return;
            }
            if message.get("type").and_then(serde_json::Value::as_str) == Some("close_decision") {
                if let Some(allow) = message.get("allow").and_then(serde_json::Value::as_bool) {
                    let _ = proxy_for_ipc.send_event(UiEvent::CloseDecision(allow));
                }
                return;
            }
            let Ok(protocol_request) = serde_json::from_value::<koren_protocol::Request>(message)
            else {
                return;
            };
            let request_id = protocol_request.request_id;
            let backend = backend_for_ipc.clone();
            let proxy = proxy_for_ipc.clone();
            thread::spawn(move || {
                let response =
                    bridge::send_request(&backend, protocol_request).unwrap_or_else(|error| {
                        koren_protocol::Response::new(
                            request_id,
                            Err(koren_protocol::ProtocolError {
                                code: koren_protocol::ErrorCode::Internal,
                                message: error,
                            }),
                        )
                    });
                let _ = proxy.send_event(UiEvent::BackendResponse(Box::new(response)));
            });
        })
        .with_navigation_handler(|url| url.starts_with("koren://app/"));

    #[cfg(target_os = "linux")]
    let webview = {
        let container = window
            .default_vbox()
            .ok_or("Tao did not create its GTK content container")?;
        builder
            .with_url(startup_url(workspace.as_deref()))
            .build_gtk(container)?
    };
    #[cfg(not(target_os = "linux"))]
    let webview = builder
        .with_url(startup_url(workspace.as_deref()))
        .build_as_child(&window)?;

    #[cfg(target_os = "linux")]
    let dialogs = Rc::new(RefCell::new(Vec::<(u64, gtk::FileChooserNative)>::new()));
    #[cfg(target_os = "linux")]
    let mut dialog_id = 0u64;
    let mut close_prompt_open = false;
    if let Some(delay_ms) = std::env::var("KOREN_CODE_SMOKE_EXIT_AFTER_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        let timer_proxy = proxy.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(delay_ms));
            let _ = timer_proxy.send_event(UiEvent::SmokeExit);
        });
    }

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(UiEvent::PickWorkspace(responder)) => {
                #[cfg(target_os = "linux")]
                {
                    dialog_id = dialog_id.saturating_add(1);
                    let id = dialog_id;
                    let dialog = gtk::FileChooserNative::new(
                        Some("Open Workspace"),
                        Some(window.gtk_window()),
                        gtk::FileChooserAction::SelectFolder,
                        Some("Open"),
                        Some("Cancel"),
                    );
                    let event_proxy = proxy.clone();
                    let responder = Rc::new(RefCell::new(Some(responder)));
                    let response_slot = responder.clone();
                    dialog.connect_response(move |chooser, response| {
                        let path = if response == gtk::ResponseType::Accept {
                            chooser
                                .filename()
                                .map(|path| path.to_string_lossy().into_owned())
                        } else {
                            None
                        };
                        if let Some(responder) = response_slot.borrow_mut().take() {
                            let _ = event_proxy.send_event(UiEvent::PickerResolved {
                                id,
                                path,
                                responder,
                            });
                        }
                    });
                    dialogs.borrow_mut().push((id, dialog.clone()));
                    dialog.show();
                }
                #[cfg(not(target_os = "linux"))]
                responder.respond(json_response(
                    StatusCode::NOT_IMPLEMENTED,
                    br#"{"error":"native folder picker is not available on this platform"}"#
                        .to_vec(),
                ));
            }
            Event::UserEvent(UiEvent::PickFile(responder)) => {
                #[cfg(target_os = "linux")]
                {
                    dialog_id = dialog_id.saturating_add(1);
                    let id = dialog_id;
                    let dialog = gtk::FileChooserNative::new(
                        Some("Open File"),
                        Some(window.gtk_window()),
                        gtk::FileChooserAction::Open,
                        Some("Open"),
                        Some("Cancel"),
                    );
                    let event_proxy = proxy.clone();
                    let responder = Rc::new(RefCell::new(Some(responder)));
                    let response_slot = responder.clone();
                    dialog.connect_response(move |chooser, response| {
                        let path = if response == gtk::ResponseType::Accept {
                            chooser
                                .filename()
                                .map(|path| path.to_string_lossy().into_owned())
                        } else {
                            None
                        };
                        if let Some(responder) = response_slot.borrow_mut().take() {
                            let _ = event_proxy.send_event(UiEvent::PickerResolved {
                                id,
                                path,
                                responder,
                            });
                        }
                    });
                    dialogs.borrow_mut().push((id, dialog.clone()));
                    dialog.show();
                }
                #[cfg(not(target_os = "linux"))]
                responder.respond(json_response(
                    StatusCode::NOT_IMPLEMENTED,
                    br#"{"error":"native file picker is not available on this platform"}"#.to_vec(),
                ));
            }
            Event::UserEvent(UiEvent::PickerResolved {
                id,
                path,
                responder,
            }) => {
                #[cfg(target_os = "linux")]
                dialogs
                    .borrow_mut()
                    .retain(|(dialog_id, _)| *dialog_id != id);
                let body = serde_json::to_vec(&serde_json::json!({ "path": path }))
                    .unwrap_or_else(|_| b"{\"path\":null}".to_vec());
                responder.respond(json_response(StatusCode::OK, body));
            }
            Event::UserEvent(UiEvent::BackendResponse(response)) => {
                if let Ok(payload) = serde_json::to_string(&response) {
                    let script = format!(
                        "window.dispatchEvent(new CustomEvent('koren-platform-response', {{ detail: {payload} }}));"
                    );
                    let _ = webview.evaluate_script(&script);
                }
            }
            Event::UserEvent(UiEvent::RecoveryResponse(response)) => {
                if let Ok(payload) = serde_json::to_string(&response) {
                    let script = format!(
                        "window.dispatchEvent(new CustomEvent('koren-recovery-response', {{ detail: {payload} }}));"
                    );
                    let _ = webview.evaluate_script(&script);
                }
            }
            Event::UserEvent(UiEvent::CloseDecision(allow)) => {
                close_prompt_open = false;
                if allow {
                    bridge::shutdown(&backend);
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::UserEvent(UiEvent::SmokeExit) => {
                bridge::shutdown(&backend);
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                if !close_prompt_open {
                    close_prompt_open = true;
                    let _ = webview
                        .evaluate_script("window.dispatchEvent(new Event('koren-close-request'))");
                }
            }
            Event::LoopDestroyed => {
                bridge::shutdown(&backend);
                let _ = &webview;
            }
            _ => {}
        }
    });
}

fn route_request(
    request: Request<Vec<u8>>,
    responder: RequestAsyncResponder,
    backend: BackendSender,
    proxy: EventLoopProxy<UiEvent>,
) {
    if request.uri().host() != Some("app") {
        responder.respond(json_response(
            StatusCode::FORBIDDEN,
            b"invalid local application host".to_vec(),
        ));
        return;
    }
    let path = request.uri().path().to_owned();
    if path == "/recovery" && request.method() == Method::GET {
        thread::spawn(move || {
            let result =
                recovery::RecoveryStore::for_current_user().and_then(|store| store.summaries());
            let (status, body) = match result {
                Ok(snapshots) => (
                    StatusCode::OK,
                    serde_json::to_vec(&snapshots).unwrap_or_default(),
                ),
                Err(error) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": error.to_string()})
                        .to_string()
                        .into_bytes(),
                ),
            };
            responder.respond(json_response(status, body));
        });
        return;
    }
    if path.starts_with("/recovery/")
        && path != "/recovery/discard"
        && path != "/recovery/fingerprint"
        && request.method() == Method::GET
    {
        let id = path.trim_start_matches("/recovery/").to_owned();
        thread::spawn(move || {
            let result =
                recovery::RecoveryStore::for_current_user().and_then(|store| store.get(&id));
            let (status, body) = match result {
                Ok(Some(snapshot)) => (
                    StatusCode::OK,
                    serde_json::to_vec(&snapshot).unwrap_or_default(),
                ),
                Ok(None) => (
                    StatusCode::NOT_FOUND,
                    b"recovery snapshot not found".to_vec(),
                ),
                Err(error) => (
                    StatusCode::BAD_REQUEST,
                    serde_json::json!({"error": error.to_string()})
                        .to_string()
                        .into_bytes(),
                ),
            };
            responder.respond(json_response(status, body));
        });
        return;
    }
    if path == "/recovery/fingerprint" && request.method() == Method::POST {
        let bytes = request.into_body();
        if bytes.len() > 16 * 1024 {
            responder.respond(json_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                b"fingerprint request exceeds limit".to_vec(),
            ));
            return;
        }
        thread::spawn(move || {
            let result = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|value| {
                    Some((
                        value.get("workspace")?.as_str()?.to_owned(),
                        value.get("path")?.as_str()?.to_owned(),
                    ))
                })
                .ok_or_else(|| format!("invalid fingerprint request ({} bytes)", bytes.len()))
                .and_then(|(workspace, path)| {
                    recovery::fingerprint_file(&workspace, &path).map_err(|error| error.to_string())
                });
            let (status, body) = match result {
                Ok(fingerprint) => (
                    StatusCode::OK,
                    serde_json::json!({"sha256": fingerprint})
                        .to_string()
                        .into_bytes(),
                ),
                Err(error) => (
                    StatusCode::BAD_REQUEST,
                    serde_json::json!({"error": error}).to_string().into_bytes(),
                ),
            };
            responder.respond(json_response(status, body));
        });
        return;
    }
    if path == "/recovery" && request.method() == Method::POST {
        let bytes = request.into_body();
        if bytes.len() > 8 * 1024 * 1024 {
            responder.respond(json_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                b"recovery snapshot exceeds limit".to_vec(),
            ));
            return;
        }
        thread::spawn(move || {
            let result = serde_json::from_slice::<recovery::RecoverySnapshot>(&bytes)
                .map_err(|error| error.to_string())
                .and_then(|snapshot| {
                    recovery::RecoveryStore::for_current_user()
                        .and_then(|store| store.write(snapshot))
                        .map_err(|error| error.to_string())
                });
            let (status, body) = match result {
                Ok(id) => (
                    StatusCode::OK,
                    serde_json::json!({"id": id}).to_string().into_bytes(),
                ),
                Err(error) => (
                    StatusCode::BAD_REQUEST,
                    serde_json::json!({"error": error}).to_string().into_bytes(),
                ),
            };
            responder.respond(json_response(status, body));
        });
        return;
    }
    if path == "/recovery/discard" && request.method() == Method::POST {
        let bytes = request.into_body();
        if bytes.len() > 16 * 1024 {
            responder.respond(json_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                b"recovery discard request exceeds limit".to_vec(),
            ));
            return;
        }
        thread::spawn(move || {
            let request = serde_json::from_slice::<serde_json::Value>(&bytes).ok();
            let result = request
                .and_then(|value| {
                    if let Some(id) = value.get("id").and_then(serde_json::Value::as_str) {
                        Some((Some(id.to_owned()), None, None))
                    } else {
                        Some((
                            None,
                            value.get("workspace")?.as_str().map(str::to_owned),
                            value.get("path")?.as_str().map(str::to_owned),
                        ))
                    }
                })
                .ok_or_else(|| "missing recovery identifier or document path".to_owned())
                .and_then(|(id, workspace, path)| {
                    recovery::RecoveryStore::for_current_user()
                        .and_then(|store| match (id, workspace, path) {
                            (Some(id), _, _) => store.remove(&id),
                            (None, Some(workspace), Some(path)) => {
                                store.remove_path(&workspace, &path)
                            }
                            _ => Err(recovery::RecoveryError::InvalidSnapshot(
                                "invalid discard request".into(),
                            )),
                        })
                        .map_err(|error| error.to_string())
                });
            let (status, body) = match result {
                Ok(()) => (StatusCode::OK, b"{}".to_vec()),
                Err(error) => (
                    StatusCode::BAD_REQUEST,
                    serde_json::json!({"error": error}).to_string().into_bytes(),
                ),
            };
            responder.respond(json_response(status, body));
        });
        return;
    }
    if path == "/picker" && request.method() == "GET" {
        if proxy.send_event(UiEvent::PickWorkspace(responder)).is_err() {
            return;
        }
        return;
    }
    if path == "/file-picker" && request.method() == "GET" {
        if proxy.send_event(UiEvent::PickFile(responder)).is_err() {
            return;
        }
        return;
    }
    if path == "/api" && request.method() == "POST" {
        thread::spawn(move || {
            let bytes = request.into_body();
            let (request_id, result) = if bytes.len() > 1024 * 1024 {
                (
                    0,
                    Err(koren_protocol::ProtocolError {
                        code: koren_protocol::ErrorCode::InvalidRequest,
                        message: "request body exceeds the 1 MiB bridge limit".into(),
                    }),
                )
            } else {
                match serde_json::from_slice::<koren_protocol::Request>(&bytes) {
                    Ok(message) => {
                        let id = message.request_id;
                        let result = bridge::send_request(&backend, message)
                            .map(|response| response.result)
                            .unwrap_or_else(|error| {
                                Err(koren_protocol::ProtocolError {
                                    code: koren_protocol::ErrorCode::Internal,
                                    message: error,
                                })
                            });
                        (id, result)
                    }
                    Err(error) => {
                        let id = serde_json::from_slice::<serde_json::Value>(&bytes)
                            .ok()
                            .and_then(|value| {
                                value.get("request_id").and_then(serde_json::Value::as_u64)
                            })
                            .unwrap_or(0);
                        (
                            id,
                            Err(koren_protocol::ProtocolError {
                                code: koren_protocol::ErrorCode::InvalidRequest,
                                message: format!("invalid protocol request: {error}"),
                            }),
                        )
                    }
                }
            };
            let body = serde_json::to_vec(&koren_protocol::Response::new(request_id, result))
                .unwrap_or_default();
            responder.respond(json_response(StatusCode::OK, body));
        });
        return;
    }
    if is_disallowed_method(&path, request.method()) {
        responder.respond(json_response(
            StatusCode::METHOD_NOT_ALLOWED,
            b"method not allowed".to_vec(),
        ));
        return;
    }
    if request.method() != "GET" {
        responder.respond(json_response(
            StatusCode::METHOD_NOT_ALLOWED,
            b"method not allowed".to_vec(),
        ));
        return;
    }
    if path == "/events" && request.method() == "GET" {
        thread::spawn(move || {
            let (sender, receiver) = mpsc::sync_channel::<Vec<DesktopEvent>>(2);
            if bridge::subscribe_once(&backend, sender).is_err() {
                responder.respond(json_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    b"[]".to_vec(),
                ));
                return;
            }
            let body = receiver
                .recv_timeout(Duration::from_secs(22))
                .ok()
                .and_then(|events| serde_json::to_vec(&events).ok())
                .unwrap_or_else(|| b"[]".to_vec());
            responder.respond(json_response(StatusCode::OK, body));
        });
        return;
    }
    serve_asset(&path, responder);
}

fn recovery_operation(
    operation: &str,
    payload: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use recovery::{RecoverySnapshot, RecoveryStore};

    let store = RecoveryStore::for_current_user().map_err(|error| error.to_string())?;
    match operation {
        "list" => serde_json::to_value(store.summaries().map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string()),
        "get" => {
            let id = payload
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "missing recovery identifier".to_owned())?;
            let snapshot = store
                .get(id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "recovery snapshot not found".to_owned())?;
            serde_json::to_value(snapshot).map_err(|error| error.to_string())
        }
        "fingerprint" => {
            let workspace = payload
                .get("workspace")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "missing workspace path".to_owned())?;
            let path = payload
                .get("path")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "missing document path".to_owned())?;
            let sha256 =
                recovery::fingerprint_file(workspace, path).map_err(|error| error.to_string())?;
            Ok(serde_json::json!({"sha256": sha256}))
        }
        "write" => {
            let snapshot = serde_json::from_value::<RecoverySnapshot>(payload.clone())
                .map_err(|error| error.to_string())?;
            let id = store.write(snapshot).map_err(|error| error.to_string())?;
            Ok(serde_json::json!({"id": id}))
        }
        "discard" => {
            if let Some(id) = payload.get("id").and_then(serde_json::Value::as_str) {
                store.remove(id).map_err(|error| error.to_string())?;
            } else {
                let workspace = payload
                    .get("workspace")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "missing workspace path".to_owned())?;
                let path = payload
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "missing document path".to_owned())?;
                store
                    .remove_path(workspace, path)
                    .map_err(|error| error.to_string())?;
            }
            Ok(serde_json::json!({}))
        }
        _ => Err("unsupported recovery operation".into()),
    }
}

fn is_disallowed_method(path: &str, method: &Method) -> bool {
    match path {
        "/api" => method != Method::POST,
        "/events" | "/picker" | "/file-picker" => method != Method::GET,
        "/recovery" => method != Method::GET && method != Method::POST,
        "/recovery/discard" | "/recovery/fingerprint" => method != Method::POST,
        _ if path.starts_with("/recovery/") => method != Method::GET,
        _ => false,
    }
}

fn workspace_argument(
    args: impl IntoIterator<Item = OsString>,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let mut args = args.into_iter();
    let Some(argument) = args.next() else {
        return Ok(None);
    };
    let value = argument.to_string_lossy();
    let path = if let Some(path) = value.strip_prefix("--workspace=") {
        PathBuf::from(path)
    } else if value == "--workspace" {
        PathBuf::from(args.next().ok_or("--workspace requires a directory path")?)
    } else {
        return Err(
            format!("unknown argument {value:?}; usage: koren-code [--workspace PATH]").into(),
        );
    };
    if args.next().is_some() {
        return Err("usage: koren-code [--workspace PATH]".into());
    }
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    };
    let metadata = std::fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(
            "--workspace must name an existing directory and cannot be a symbolic link".into(),
        );
    }
    let canonical = std::fs::canonicalize(path)?;
    Ok(Some(
        canonical
            .to_str()
            .ok_or("workspace path is not valid UTF-8")?
            .to_owned(),
    ))
}

fn startup_url(workspace: Option<&str>) -> String {
    let Some(workspace) = workspace else {
        return "koren://app/index.html".into();
    };
    let encoded = workspace
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect::<String>();
    format!("koren://app/index.html?workspace={encoded}")
}

fn serve_asset(path: &str, responder: RequestAsyncResponder) {
    let requested = if path == "/" {
        "index.html"
    } else {
        path.trim_start_matches('/')
    };
    if requested
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        responder.respond(json_response(
            StatusCode::BAD_REQUEST,
            b"invalid asset path".to_vec(),
        ));
        return;
    }
    let bytes = if cfg!(debug_assertions) {
        let asset_root = std::env::var_os("KOREN_CODE_ASSET_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("frontend/dist"));
        std::fs::read(asset_root.join(requested)).ok()
    } else {
        FRONTEND
            .get_file(requested)
            .map(|file| file.contents().to_vec())
    };
    let Some(bytes) = bytes else {
        responder.respond(json_response(
            StatusCode::NOT_FOUND,
            b"asset not found".to_vec(),
        ));
        return;
    };
    let content_type = mime_guess::from_path(requested)
        .first_or_octet_stream()
        .as_ref()
        .to_owned();
    let response = Response::builder()
        .status(StatusCode::OK)
        .header("content-type", content_type)
        .header("cache-control", "no-store")
        .header("content-security-policy", "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; object-src 'none'; frame-src 'none'; base-uri 'none'; form-action 'self'")
        .body(bytes)
        .unwrap_or_else(|_| Response::new(Vec::new()));
    responder.respond(response);
}

fn json_response(status: StatusCode, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json; charset=utf-8")
        .header("cache-control", "no-store")
        .body(body)
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::{is_disallowed_method, startup_url, workspace_argument};
    use std::{ffi::OsString, fs};
    use wry::http::Method;

    #[test]
    fn bridge_routes_accept_only_their_expected_http_methods() {
        assert!(!is_disallowed_method("/api", &Method::POST));
        assert!(!is_disallowed_method("/events", &Method::GET));
        assert!(!is_disallowed_method("/picker", &Method::GET));
        assert!(!is_disallowed_method("/file-picker", &Method::GET));
        assert!(!is_disallowed_method("/recovery", &Method::GET));
        assert!(!is_disallowed_method("/recovery", &Method::POST));
        assert!(!is_disallowed_method("/recovery/discard", &Method::POST));
        assert!(!is_disallowed_method(
            "/recovery/fingerprint",
            &Method::POST
        ));
        assert!(!is_disallowed_method("/recovery/123", &Method::GET));
        assert!(is_disallowed_method("/api", &Method::GET));
        assert!(is_disallowed_method("/events", &Method::POST));
        assert!(is_disallowed_method("/picker", &Method::POST));
        assert!(is_disallowed_method("/recovery", &Method::DELETE));
    }

    #[test]
    fn workspace_argument_accepts_a_valid_directory_and_encodes_its_startup_url() {
        let directory =
            std::env::temp_dir().join(format!("koren-workspace-arg-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let result = workspace_argument([OsString::from("--workspace"), directory.clone().into()])
            .unwrap()
            .unwrap();
        assert_eq!(
            result,
            fs::canonicalize(&directory).unwrap().to_string_lossy()
        );
        assert!(startup_url(Some("/tmp/a folder")).contains("/tmp/a%20folder"));
        assert!(workspace_argument([OsString::from("--unknown")]).is_err());
        assert!(workspace_argument([OsString::from("--workspace")]).is_err());
        assert!(
            workspace_argument([
                OsString::from("--workspace"),
                directory.join("missing").into()
            ])
            .is_err()
        );
        assert!(
            workspace_argument([
                OsString::from("--workspace"),
                directory.clone().into(),
                OsString::from("extra")
            ])
            .is_err()
        );
        let _ = fs::remove_dir_all(directory);
    }
}
