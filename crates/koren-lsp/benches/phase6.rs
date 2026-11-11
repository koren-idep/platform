//! Fake-server measurements for process startup, typed request latency, and diagnostics.

use koren_documents::{DocumentId, DocumentInfo, DocumentSnapshot};
use koren_lsp::{
    LspDocumentSync, LspServer, LspServerManager, ServerAuthorizer, ServerConfig, ServerEvent,
};
use koren_protocol::TextPosition;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Allow;
impl ServerAuthorizer for Allow {
    fn authorize(&mut self, _configuration: &ServerConfig) -> bool {
        true
    }
}

fn measure(label: &str, iterations: u32, mut operation: impl FnMut()) {
    let mut samples = Vec::with_capacity(iterations as usize);
    for _ in 0..iterations {
        let start = Instant::now();
        operation();
        samples.push(start.elapsed());
    }
    let mean = samples.iter().map(Duration::as_secs_f64).sum::<f64>() * 1e6 / f64::from(iterations);
    let min = samples
        .iter()
        .copied()
        .min()
        .unwrap_or_default()
        .as_secs_f64()
        * 1e6;
    println!("{label},iterations={iterations},mean_us={mean:.3},min_us={min:.3}");
}

fn config(root: &std::path::Path) -> ServerConfig {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fake_lsp_server.py");
    ServerConfig {
        id: "phase6-fake".into(),
        command: if cfg!(windows) { "python" } else { "python3" }.into(),
        arguments: vec![fixture.to_string_lossy().into_owned()],
        workspace_root: root.to_path_buf(),
        environment: BTreeMap::new(),
    }
}

fn measure_diagnostics(server: &mut LspServer, snapshot: &DocumentSnapshot) -> Vec<Duration> {
    let mut diagnostics = Vec::new();
    for index in 0..20_u64 {
        let uri = format!("file:///bench-{index}.demo");
        let start = Instant::now();
        server
            .did_open(&uri, "demo", 1, &snapshot.text)
            .expect("notify open");
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let found = server
                .drain_events()
                .into_iter()
                .any(|event| matches!(event, ServerEvent::Diagnostics(_)));
            if found {
                diagnostics.push(start.elapsed());
                break;
            }
            assert!(
                Instant::now() < deadline,
                "fake server diagnostics timed out"
            );
            std::thread::yield_now();
        }
    }
    diagnostics
}

fn main() {
    let root = std::env::temp_dir().join(format!(
        "koren-phase6-bench-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("create LSP benchmark workspace");

    measure("python_fixture_process_start_exit", 20, || {
        let mut child =
            std::process::Command::new(if cfg!(windows) { "python" } else { "python3" })
                .args(["-c", "pass"])
                .spawn()
                .expect("spawn Python baseline");
        child.wait().expect("wait Python baseline");
    });

    measure("fake_lsp_start_and_shutdown", 20, || {
        let mut manager = LspServerManager::new();
        let server = manager
            .start_server(config(&root), &mut Allow)
            .expect("start fake server");
        manager.stop_server(server).expect("stop fake server");
    });

    let mut manager = LspServerManager::new();
    let server_id = manager
        .start_server(config(&root), &mut Allow)
        .expect("start fake server");
    let server = manager.server_mut(server_id).expect("get fake server");
    measure("fake_lsp_hover_request", 100, || {
        std::hint::black_box(
            server
                .hover(
                    "file:///bench.demo",
                    1,
                    TextPosition {
                        line: 0,
                        character: 0,
                    },
                )
                .expect("fake hover"),
        );
    });

    let snapshot = DocumentSnapshot {
        info: DocumentInfo {
            id: DocumentId(1),
            path: Some("bench.demo".into()),
            revision: 1,
            dirty: false,
        },
        text: "benchmark".into(),
    };
    let mut sync = LspDocumentSync::default();
    sync.open(DocumentId(1), "file:///bench.demo", "demo", &snapshot)
        .expect("open benchmark sync");
    measure("document_sync_change_mapping", 10_000, || {
        let revision = sync
            .version(DocumentId(1))
            .map(|(_, revision)| revision)
            .unwrap_or_default()
            + 1;
        let next = DocumentSnapshot {
            info: DocumentInfo {
                revision,
                ..snapshot.info.clone()
            },
            text: "benchmark changed".into(),
        };
        std::hint::black_box(
            sync.change(DocumentId(1), &next)
                .expect("advance sync revision"),
        );
    });
    let diagnostics = measure_diagnostics(server, &snapshot);
    let iterations = u32::try_from(diagnostics.len()).expect("diagnostic sample count fits u32");
    let average =
        diagnostics.iter().map(Duration::as_secs_f64).sum::<f64>() * 1e6 / f64::from(iterations);
    println!(
        "fake_lsp_diagnostic_publication,iterations={},mean_us={average:.1}",
        diagnostics.len()
    );
    manager.shutdown();
    let _ = fs::remove_dir_all(root);
}
