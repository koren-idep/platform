mod bridge;

use bridge::{BackendSender, DesktopEvent};
#[cfg(target_os = "linux")]
use gtk::prelude::*;
use include_dir::{Dir, include_dir};
use std::{cell::RefCell, path::Path, rc::Rc, sync::mpsc, thread, time::Duration};
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
    PickerResolved {
        id: u64,
        path: Option<String>,
        responder: RequestAsyncResponder,
    },
    CloseDecision(bool),
    SmokeExit,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Koren Code Editor could not start: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
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
            let Ok(message) = serde_json::from_str::<serde_json::Value>(request.body()) else {
                return;
            };
            if message.get("type").and_then(serde_json::Value::as_str) != Some("close_decision") {
                return;
            }
            if let Some(allow) = message.get("allow").and_then(serde_json::Value::as_bool) {
                let _ = proxy_for_ipc.send_event(UiEvent::CloseDecision(allow));
            }
        })
        .with_navigation_handler(|url| url.starts_with("koren://app/"));

    #[cfg(target_os = "linux")]
    let webview = {
        let container = window
            .default_vbox()
            .ok_or("Tao did not create its GTK content container")?;
        builder
            .with_url("koren://app/index.html")
            .build_gtk(container)?
    };
    #[cfg(not(target_os = "linux"))]
    let webview = builder
        .with_url("koren://app/index.html")
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
    if path == "/picker" && request.method() == "GET" {
        if proxy.send_event(UiEvent::PickWorkspace(responder)).is_err() {
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
            let (sender, receiver) = mpsc::channel::<Vec<DesktopEvent>>();
            if bridge::subscribe(&backend, sender).is_err() {
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

fn is_disallowed_method(path: &str, method: &Method) -> bool {
    match path {
        "/api" => method != Method::POST,
        "/events" | "/picker" => method != Method::GET,
        _ => false,
    }
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
    use super::is_disallowed_method;
    use wry::http::Method;

    #[test]
    fn bridge_routes_accept_only_their_expected_http_methods() {
        assert!(!is_disallowed_method("/api", &Method::POST));
        assert!(!is_disallowed_method("/events", &Method::GET));
        assert!(!is_disallowed_method("/picker", &Method::GET));
        assert!(is_disallowed_method("/api", &Method::GET));
        assert!(is_disallowed_method("/events", &Method::POST));
        assert!(is_disallowed_method("/picker", &Method::POST));
    }
}
