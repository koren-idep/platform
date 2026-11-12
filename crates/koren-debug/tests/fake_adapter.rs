//! End-to-end tests for the public DAP manager and session API.

use koren_debug::{
    BreakpointSpec, DebugAdapterDefinition, DebugAuthorizer, DebugConfiguration, DebugManager,
    DebugRequestType, DebugSessionState,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
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

#[test]
fn fake_adapter_initializes_stops_inspects_and_disconnects() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let adapter_script = root.join("tests/fake_adapter.py");
    let python = if cfg!(windows) { "python" } else { "python3" };
    let mut manager = DebugManager::new();
    manager
        .register_adapter(DebugAdapterDefinition {
            id: "fake".into(),
            name: "Fake DAP".into(),
            command: python.into(),
            arguments: vec![adapter_script.to_string_lossy().into_owned()],
            working_directory: root.clone(),
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
            working_directory: root.clone(),
            environment: BTreeMap::new(),
            adapter_settings: serde_json::Map::new(),
            stop_on_entry: false,
        })
        .unwrap();
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
    let mut observed = Vec::new();
    wait_until("threads response", || {
        observed.extend(manager.drain_events());
        if session.info().state == DebugSessionState::Failed {
            panic!("session failed: {:?}; events: {observed:?}", session.info());
        }
        observed.iter().any(|event| {
            matches!(event,
        koren_debug::DebugEvent::Response { request_id, command, success: true, .. }
            if *request_id == thread_request && command == "threads")
        })
    });
    observed.clear();
    let stack_request = session.stack_trace(1, 0, 10).unwrap();
    wait_until("stack response", || {
        manager.drain_events().iter().any(|event| {
            matches!(event,
        koren_debug::DebugEvent::Response { request_id, command, success: true, .. }
            if *request_id == stack_request && command == "stackTrace")
        })
    });
    let scope_request = session.scopes(7).unwrap();
    wait_until("scopes response", || {
        manager.drain_events().iter().any(|event| {
            matches!(event,
        koren_debug::DebugEvent::Response { request_id, command, success: true, .. }
            if *request_id == scope_request && command == "scopes")
        })
    });
    let reference = koren_debug::VariableReference {
        session_id: started.session_id,
        stop_generation: session.info().stop_generation,
        adapter_reference: 11,
    };
    let variables_request = session.variables(reference, None, 0, 20).unwrap();
    wait_until("variables response", || {
        manager.drain_events().iter().any(|event| {
            matches!(event,
        koren_debug::DebugEvent::Response { request_id, command, success: true, .. }
            if *request_id == variables_request && command == "variables")
        })
    });
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
