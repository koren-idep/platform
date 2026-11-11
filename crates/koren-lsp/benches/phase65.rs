//! Measurements for asynchronous LSP dispatch, concurrency, and idle actor overhead.

use koren_documents::{DocumentId, DocumentInfo, DocumentSnapshot};
use koren_lsp::{
    LspActorEvent, LspDocumentSync, LspRequest, LspResponse, LspServerActors, ServerAuthorizer,
    ServerConfig,
};
use koren_protocol::TextPosition;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Allow;
impl ServerAuthorizer for Allow {
    fn authorize(&mut self, _: &ServerConfig) -> bool {
        true
    }
}

fn config(root: &Path, id: String) -> ServerConfig {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fake_lsp_server.py");
    ServerConfig {
        id,
        command: if cfg!(windows) { "python" } else { "python3" }.into(),
        arguments: vec![fixture.to_string_lossy().into_owned()],
        workspace_root: root.to_path_buf(),
        environment: BTreeMap::new(),
    }
}

fn wait_initialized(actors: &LspServerActors, required: &BTreeSet<u64>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut initialized = BTreeSet::new();
    while initialized.len() < required.len() {
        for event in actors.drain_events() {
            if let LspActorEvent::Initialized(id) = event {
                initialized.insert(id.get());
            }
        }
        assert!(
            Instant::now() < deadline,
            "fake language-server startup timed out"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn proc_thread_cpu_ticks() -> BTreeMap<u32, u64> {
    let mut values = BTreeMap::new();
    let Ok(entries) = fs::read_dir("/proc/self/task") else {
        return values;
    };
    for entry in entries.flatten() {
        let Ok(tid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((_, tail)) = stat.rsplit_once(") ") else {
            continue;
        };
        let fields = tail.split_whitespace().collect::<Vec<_>>();
        if let (Some(user), Some(system)) = (fields.get(11), fields.get(12)) {
            if let (Ok(user), Ok(system)) = (user.parse::<u64>(), system.parse::<u64>()) {
                values.insert(tid, user + system);
            }
        }
    }
    values
}

fn process_rss_kib() -> Option<u64> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find(|line| line.starts_with("VmRSS:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn start_actors(root: &Path, actors: &mut LspServerActors) -> Vec<koren_lsp::ServerId> {
    let mut ids = Vec::new();
    let mut startup_dispatch = Vec::new();
    for index in 0..4 {
        let started = Instant::now();
        let id = actors
            .start_server(config(root, format!("phase65-{index}")), &mut Allow)
            .expect("dispatch fake language server");
        startup_dispatch.push(started.elapsed());
        ids.push(id);
    }
    let required = ids.iter().map(|id| id.get()).collect::<BTreeSet<_>>();
    wait_initialized(actors, &required);
    println!(
        "lsp_initialization_dispatch_4_servers_mean_us={:.3},min_us={:.3}",
        startup_dispatch
            .iter()
            .map(Duration::as_secs_f64)
            .sum::<f64>()
            * 250_000.0,
        startup_dispatch
            .iter()
            .copied()
            .min()
            .unwrap()
            .as_secs_f64()
            * 1_000_000.0
    );
    ids
}

fn open_documents(
    actors: &LspServerActors,
    ids: &[koren_lsp::ServerId],
) -> Vec<koren_lsp::LspServerHandle> {
    let mut handles = Vec::new();
    for id in ids {
        let handle = actors.handle(*id).expect("get language server");
        handle
            .notify(
                LspDocumentSync::default()
                    .open(
                        DocumentId(id.get()),
                        format!("file:///bench-{}.demo", id.get()),
                        "demo",
                        &DocumentSnapshot {
                            info: DocumentInfo {
                                id: DocumentId(id.get()),
                                path: Some(format!("bench-{}.demo", id.get()).into()),
                                revision: 1,
                                dirty: false,
                            },
                            text: "benchmark".into(),
                        },
                    )
                    .expect("open document sync"),
            )
            .expect("enqueue didOpen");
        handles.push(handle);
    }
    handles
}

fn measure_requests(handles: &[koren_lsp::LspServerHandle], ids: &[koren_lsp::ServerId]) {
    let position = TextPosition {
        line: 0,
        character: 0,
    };
    let mut dispatch = Vec::new();
    for (index, handle) in handles.iter().enumerate() {
        let started = Instant::now();
        let ticket = handle
            .request(LspRequest::Hover {
                uri: format!("file:///bench-{}.demo", ids[index].get()),
                version: 1,
                position,
                timeout: Duration::from_secs(2),
            })
            .expect("enqueue hover request");
        dispatch.push(started.elapsed());
        assert!(matches!(
            ticket
                .recv_timeout(Duration::from_secs(2))
                .expect("hover response"),
            LspResponse::Hover(_)
        ));
    }
    println!(
        "hover_request_dispatch_mean_us={:.3},min_us={:.3}",
        dispatch.iter().map(Duration::as_secs_f64).sum::<f64>() * 250_000.0,
        dispatch.iter().copied().min().unwrap().as_secs_f64() * 1_000_000.0
    );

    let started = Instant::now();
    let tickets = handles
        .iter()
        .enumerate()
        .map(|(index, handle)| {
            handle
                .request(LspRequest::Hover {
                    uri: format!("file:///bench-{}.demo", ids[index].get()),
                    version: 1,
                    position,
                    timeout: Duration::from_secs(2),
                })
                .expect("enqueue concurrent hover")
        })
        .collect::<Vec<_>>();
    for ticket in tickets {
        assert!(matches!(
            ticket
                .recv_timeout(Duration::from_secs(2))
                .expect("concurrent hover response"),
            LspResponse::Hover(_)
        ));
    }
    println!(
        "four_concurrent_hover_round_trip_ms={:.3}",
        started.elapsed().as_secs_f64() * 1_000.0
    );
}

fn format_rss_delta(before: Option<u64>, after: Option<u64>) -> String {
    before.zip(after).map_or_else(
        || "unavailable".into(),
        |(before, after)| {
            if after >= before {
                format!("+{}", after - before)
            } else {
                format!("-{}", before - after)
            }
        },
    )
}

fn measure_idle_overhead(rss_before: Option<u64>) {
    let idle_started = Instant::now();
    let idle_cpu_before = proc_thread_cpu_ticks();
    let idle_rss_before = process_rss_kib();
    std::thread::sleep(Duration::from_secs(2));
    let idle_cpu_after = proc_thread_cpu_ticks();
    let idle_cpu_ticks = idle_cpu_after
        .iter()
        .filter(|(tid, _)| !idle_cpu_before.contains_key(tid))
        .map(|(_, ticks)| ticks)
        .sum::<u64>()
        + idle_cpu_before
            .iter()
            .filter_map(|(tid, before)| {
                idle_cpu_after
                    .get(tid)
                    .map(|after| after.saturating_sub(*before))
            })
            .sum::<u64>();
    println!(
        "four_idle_lsp_actors_cpu_ticks_over_ms={:.0},thread_cpu_ticks={idle_cpu_ticks},process_rss_delta_kib={}",
        idle_started.elapsed().as_secs_f64() * 1_000.0,
        format_rss_delta(idle_rss_before, process_rss_kib())
    );
    println!(
        "four_lsp_actor_process_rss_delta_kib={}",
        format_rss_delta(rss_before, process_rss_kib())
    );
}

fn main() {
    let root = std::env::temp_dir().join(format!(
        "koren-phase65-bench-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("create benchmark workspace");
    let rss_before = process_rss_kib();
    let mut actors = LspServerActors::new();
    let ids = start_actors(&root, &mut actors);
    let handles = open_documents(&actors, &ids);
    measure_requests(&handles, &ids);
    measure_idle_overhead(rss_before);
    actors.shutdown();
    let _ = fs::remove_dir_all(root);
}
