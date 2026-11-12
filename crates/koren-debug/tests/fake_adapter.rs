//! End-to-end tests for the public DAP manager and session API.

use koren_debug::{
    BreakpointSpec, DebugAdapterDefinition, DebugAuthorizer, DebugConfiguration, DebugManager,
    DebugRequestType, DebugSessionState,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

struct Allow;
impl DebugAuthorizer for Allow {
    fn authorize_adapter(&mut self, _: &DebugAdapterDefinition) -> bool {
        true
    }
    fn authorize_target(&mut self, _: &DebugConfiguration) -> bool {
        true
    }
}

fn wait_until(label: &str, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        predicate(),
        "timed out waiting for fake DAP adapter: {label}"
    );
}

fn manager_with_fake_adapter(root: &Path) -> DebugManager {
    manager_with_fake_adapter_mode(root, "normal")
}

fn manager_with_fake_adapter_mode(root: &Path, mode: &str) -> DebugManager {
    let mut manager = DebugManager::new();
    manager
        .register_adapter(DebugAdapterDefinition {
            id: "fake".into(),
            name: "Fake DAP".into(),
            command: if cfg!(windows) { "python" } else { "python3" }.into(),
            arguments: vec![
                root.join("tests/fake_adapter.py")
                    .to_string_lossy()
                    .into_owned(),
            ],
            working_directory: root.to_path_buf(),
            environment: [("KOREN_FAKE_DAP_MODE".into(), mode.into())]
                .into_iter()
                .collect::<BTreeMap<_, _>>(),
        })
        .unwrap();
    manager
        .register_configuration(DebugConfiguration {
            id: "fake.launch".into(),
            name: "Fake launch".into(),
            adapter_id: "fake".into(),
            request: DebugRequestType::Launch,
            program: Some(root.join("fixture")),
            process_id: None,
            arguments: vec![],
            working_directory: root.to_path_buf(),
            environment: BTreeMap::new(),
            adapter_settings: serde_json::Map::new(),
            stop_on_entry: false,
        })
        .unwrap();
    manager
}

fn wait_for_response(
    manager: &DebugManager,
    session: &koren_debug::DebugSessionHandle,
    request_id: u64,
    command: &str,
) {
    let mut observed = Vec::new();
    wait_until(command, || {
        observed.extend(manager.drain_events());
        assert_ne!(
            session.info().state,
            DebugSessionState::Failed,
            "{observed:?}"
        );
        observed.iter().any(|event| matches!(event,
            koren_debug::DebugEvent::Response { request_id: response_id, command: response_command, success: true, .. }
                if *response_id == request_id && response_command == command))
    });
}

#[test]
fn fake_adapter_initializes_stops_inspects_and_disconnects() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut manager = manager_with_fake_adapter(&root);
    let source = root.join("fixture.src");
    manager
        .set_source_breakpoints(
            &source,
            vec![BreakpointSpec {
                line: 3,
                column: None,
                condition: None,
                hit_condition: None,
                log_message: None,
                enabled: true,
            }],
        )
        .unwrap();
    let started = manager.start_session("fake.launch", &mut Allow).unwrap();
    let session = manager.session(started.session_id).unwrap();
    wait_until("initial stop", || {
        session.info().state == DebugSessionState::Paused
            || session.info().state == DebugSessionState::Failed
    });
    assert_eq!(
        session.info().state,
        DebugSessionState::Paused,
        "{:?}",
        session.info()
    );

    let thread_request = session.threads().unwrap();
    wait_for_response(&manager, &session, thread_request, "threads");
    let stack_request = session.stack_trace(1, 0, 10).unwrap();
    wait_for_response(&manager, &session, stack_request, "stackTrace");
    let scope_request = session.scopes(7).unwrap();
    wait_for_response(&manager, &session, scope_request, "scopes");
    let reference = koren_debug::VariableReference {
        session_id: started.session_id,
        stop_generation: session.info().stop_generation,
        adapter_reference: 11,
    };
    let variables_request = session.variables(reference, None, 0, 20).unwrap();
    wait_for_response(&manager, &session, variables_request, "variables");
    session.continue_execution().unwrap();
    wait_until("continue state", || {
        session.info().state == DebugSessionState::Running
    });
    session.stop().unwrap();
    wait_until("stop state", || {
        matches!(
            session.info().state,
            DebugSessionState::Terminated | DebugSessionState::Failed
        )
    });
}

#[test]
fn manager_runs_multiple_independent_sessions_and_cleans_them_up() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut manager = manager_with_fake_adapter(&root);
    let first = manager.start_session("fake.launch", &mut Allow).unwrap();
    let second = manager.start_session("fake.launch", &mut Allow).unwrap();
    assert_ne!(first.session_id, second.session_id);
    let first_session = manager.session(first.session_id).unwrap();
    let second_session = manager.session(second.session_id).unwrap();
    wait_until("both sessions paused", || {
        first_session.info().state == DebugSessionState::Paused
            && second_session.info().state == DebugSessionState::Paused
    });
    first_session.stop().unwrap();
    second_session.stop().unwrap();
    wait_until("both sessions stopped", || {
        matches!(
            first_session.info().state,
            DebugSessionState::Terminated | DebugSessionState::Failed
        ) && matches!(
            second_session.info().state,
            DebugSessionState::Terminated | DebugSessionState::Failed
        )
    });
    manager.shutdown();
    assert!(manager.sessions().iter().all(|session| matches!(
        session.state,
        DebugSessionState::Terminated | DebugSessionState::Failed
    )));
}

#[test]
fn failed_initialization_adapter_crash_and_reverse_request_fail_safely() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for mode in ["failed-initialize", "crash"] {
        let mut manager = manager_with_fake_adapter_mode(&root, mode);
        let started = manager.start_session("fake.launch", &mut Allow).unwrap();
        let session = manager.session(started.session_id).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.info().state != DebugSessionState::Failed && Instant::now() < deadline {
            thread::yield_now();
        }
        assert_eq!(
            session.info().state,
            DebugSessionState::Failed,
            "mode={mode}: {:?}",
            session.info()
        );
        assert!(session.info().error.is_some());
        manager.shutdown();
    }

    let mut manager = manager_with_fake_adapter_mode(&root, "reverse-request");
    let started = manager.start_session("fake.launch", &mut Allow).unwrap();
    let session = manager.session(started.session_id).unwrap();
    let mut observed = Vec::new();
    wait_until("reverse request rejection", || {
        observed.extend(manager.drain_events());
        session.info().state == DebugSessionState::Paused
            && observed.iter().any(|event| {
                matches!(event,
                koren_debug::DebugEvent::Output { output, .. }
                    if output.contains("reverse request rejected"))
            })
    });
    session.stop().unwrap();
    manager.shutdown();
}
