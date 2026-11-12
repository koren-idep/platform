//! Local fake-adapter measurements. Adapter and Python startup costs are included.
use koren_debug::{
    DebugAdapterDefinition, DebugAuthorizer, DebugConfiguration, DebugManager, DebugRequestType,
    DebugSessionHandle, DebugSessionState,
};
use std::{
    collections::BTreeMap,
    env,
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

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "fake DAP workload timed out");
        thread::sleep(Duration::from_millis(1));
    }
}

fn wait_response(manager: &DebugManager, session: &DebugSessionHandle, request: u64) -> Duration {
    let start = Instant::now();
    let deadline = start + Duration::from_secs(5);
    loop {
        let events = manager.drain_events();
        if events.iter().any(|event| {
            matches!(event,
            koren_debug::DebugEvent::Response { request_id, success: true, .. }
                if *request_id == request)
        }) {
            return start.elapsed();
        }
        assert!(
            Instant::now() < deadline,
            "fake adapter response timed out: {:?}",
            session.info()
        );
        thread::yield_now();
    }
}

fn report(name: &str, samples: &[Duration]) {
    let total: Duration = samples.iter().sum();
    let count = f64::from(u32::try_from(samples.len()).unwrap());
    let mean = total.as_secs_f64() * 1e3 / count;
    let min = samples.iter().copied().min().unwrap().as_secs_f64() * 1e3;
    println!(
        "{name}: {} samples, mean {mean:.3} ms, min {min:.3} ms",
        samples.len()
    );
}

fn manager(root: &Path) -> DebugManager {
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
            working_directory: root.into(),
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
            arguments: Vec::new(),
            working_directory: root.into(),
            environment: BTreeMap::new(),
            adapter_settings: serde_json::Map::new(),
            stop_on_entry: true,
        })
        .unwrap();
    manager
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut starts = Vec::new();
    let mut threads = Vec::new();
    let mut stacks = Vec::new();
    let mut scopes = Vec::new();
    let mut variables = Vec::new();
    let mut multiple = Vec::new();
    for _ in 0..10 {
        let mut manager = manager(&root);
        let start = Instant::now();
        let started = manager.start_session("fake.launch", &mut Allow).unwrap();
        let session = manager.session(started.session_id).unwrap();
        wait_until(|| session.info().state == DebugSessionState::Paused);
        starts.push(start.elapsed());
        let request = session.threads().unwrap();
        threads.push(wait_response(&manager, &session, request));
        let request = session.stack_trace(1, 0, 50).unwrap();
        stacks.push(wait_response(&manager, &session, request));
        let request = session.scopes(7).unwrap();
        scopes.push(wait_response(&manager, &session, request));
        let reference = koren_debug::VariableReference {
            session_id: started.session_id,
            stop_generation: session.info().stop_generation,
            adapter_reference: 11,
        };
        let request = session.variables(reference, None, 0, 50).unwrap();
        variables.push(wait_response(&manager, &session, request));
        let start = Instant::now();
        let second = manager.start_session("fake.launch", &mut Allow).unwrap();
        let second_session = manager.session(second.session_id).unwrap();
        wait_until(|| second_session.info().state == DebugSessionState::Paused);
        multiple.push(start.elapsed());
        session.stop().unwrap();
        second_session.stop().unwrap();
        manager.shutdown();
    }
    report(
        "Adapter process start + DAP initialization + launch",
        &starts,
    );
    report("Request/response: threads", &threads);
    report("Request/response: stackTrace", &stacks);
    report("Request/response: scopes", &scopes);
    report("Request/response: variables", &variables);
    report(
        "Second concurrent session start while one session is active",
        &multiple,
    );
}
