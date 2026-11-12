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
            environment: BTreeMap::new(),
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
