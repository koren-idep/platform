//! Reproducible, non-gating project and process-task measurements.

use koren_projects::{ProjectManager, ProjectSpec, ProviderId};
use koren_tasks::{
    TaskAuthorizer, TaskCaller, TaskCategory, TaskDefinition, TaskError, TaskManager, TaskOptions,
    TaskRunId,
};
use koren_workspace::WorkspaceManager;
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Allow;
impl TaskAuthorizer for Allow {
    fn authorize(&self, _: &TaskCaller, _: &TaskDefinition) -> Result<(), TaskError> {
        Ok(())
    }
}
fn child_mode() -> bool {
    if env::args().nth(1).as_deref() == Some("--koren-bench-child") {
        let n = env::var("KOREN_BENCH_BYTES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        if n > 0 {
            println!("{}", "x".repeat(n));
            eprintln!("{}", "y".repeat(n));
        }
        return true;
    }
    false
}
fn wait(manager: &TaskManager, id: TaskRunId) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if manager.status(id).expect("run exists").state.is_finished() {
            return;
        }
        assert!(Instant::now() < deadline, "benchmark child timed out");
        thread::yield_now();
    }
}
fn task(key: String, exe: PathBuf, cwd: PathBuf, bytes: usize) -> TaskDefinition {
    TaskDefinition {
        key,
        display_name: "fixture".into(),
        category: TaskCategory::Custom,
        executable: exe,
        arguments: vec!["--koren-bench-child".into()],
        working_directory: cwd,
        environment: BTreeMap::from([("KOREN_BENCH_BYTES".into(), bytes.to_string())]),
        timeout: Some(Duration::from_secs(30)),
        project_id: None,
    }
}
fn report(name: &str, values: &[Duration]) {
    let total = values.iter().copied().sum::<Duration>().as_secs_f64() * 1e6;
    let count = u32::try_from(values.len()).unwrap_or(u32::MAX);
    let minimum = values.iter().copied().min().unwrap().as_secs_f64() * 1e6;
    println!(
        "{name}: mean={:.1} us min={:.1} us n={}",
        total / f64::from(count),
        minimum,
        values.len()
    );
}
struct TaskMeasurements {
    registration: Vec<Duration>,
    start: Vec<Duration>,
    completion: Vec<Duration>,
    output: Vec<Duration>,
    concurrent: Duration,
    process_only: Vec<Duration>,
}
fn bench_projects(root: &Path) -> (Vec<Duration>, Vec<Duration>) {
    let mut workspace = WorkspaceManager::new();
    workspace.open(root).unwrap();
    let mut projects = ProjectManager::new();
    let provider = ProviderId::new("benchmark").unwrap();
    let mut registration = Vec::new();
    for i in 0..128 {
        let now = Instant::now();
        projects
            .register(
                &workspace,
                ProjectSpec {
                    display_name: format!("p{i}"),
                    root: PathBuf::from(format!("p{i}")),
                    provider_id: provider.clone(),
                    configuration_path: None,
                    provider_data: BTreeMap::new(),
                },
            )
            .unwrap();
        registration.push(now.elapsed());
    }
    let mut enumeration = Vec::new();
    for _ in 0..1000 {
        let now = Instant::now();
        std::hint::black_box(projects.list());
        enumeration.push(now.elapsed());
    }
    (registration, enumeration)
}
fn bench_tasks(root: &Path) -> TaskMeasurements {
    let executable = env::current_exe().unwrap();
    let mut tasks = TaskManager::new(TaskOptions {
        max_concurrent: 8,
        ..TaskOptions::default()
    })
    .unwrap();
    let mut registration = Vec::new();
    let mut ids = Vec::new();
    for i in 0..100 {
        let now = Instant::now();
        ids.push(
            tasks
                .register(task(
                    format!("task-{i}"),
                    executable.clone(),
                    root.to_path_buf(),
                    0,
                ))
                .unwrap(),
        );
        registration.push(now.elapsed());
    }
    let authorization = Allow;
    let mut start = Vec::new();
    let mut completion = Vec::new();
    for id in ids.iter().take(20) {
        let now = Instant::now();
        let run = tasks
            .start(*id, &TaskCaller::InteractiveUser, &authorization)
            .unwrap();
        start.push(now.elapsed());
        let total = Instant::now();
        wait(&tasks, run);
        completion.push(total.elapsed());
    }
    let mut output_times = Vec::new();
    for i in 0..5 {
        let output_id = tasks
            .register(task(
                format!("output-{i}"),
                executable.clone(),
                root.to_path_buf(),
                32 * 1024,
            ))
            .unwrap();
        let now = Instant::now();
        let output_run = tasks
            .start(output_id, &TaskCaller::InteractiveUser, &authorization)
            .unwrap();
        wait(&tasks, output_run);
        output_times.push(now.elapsed());
        assert!(tasks.output(output_run).unwrap().stdout.len() >= 32 * 1024);
    }
    let concurrent_ids = (0..4)
        .map(|i| {
            tasks
                .register(task(
                    format!("concurrent-{i}"),
                    executable.clone(),
                    root.to_path_buf(),
                    0,
                ))
                .unwrap()
        })
        .collect::<Vec<_>>();
    let now = Instant::now();
    let runs = concurrent_ids
        .into_iter()
        .map(|id| {
            tasks
                .start(id, &TaskCaller::InteractiveUser, &authorization)
                .unwrap()
        })
        .collect::<Vec<_>>();
    for id in runs {
        wait(&tasks, id);
    }
    let concurrent = now.elapsed();
    let mut process_times = Vec::new();
    for _ in 0..5 {
        let now = Instant::now();
        Command::new(&executable)
            .arg("--koren-bench-child")
            .current_dir(root)
            .status()
            .unwrap();
        process_times.push(now.elapsed());
    }
    TaskMeasurements {
        registration,
        start,
        completion,
        output: output_times,
        concurrent,
        process_only: process_times,
    }
}
fn main() {
    if child_mode() {
        return;
    }
    let root = env::temp_dir().join(format!(
        "koren-phase3-bench-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    for i in 0..128 {
        fs::create_dir(root.join(format!("p{i}"))).unwrap();
    }
    let (projects, enumeration) = bench_projects(&root);
    let tasks = bench_tasks(&root);
    println!(
        "Koren Phase 3 backend benchmark; fixture root on system temp storage; current executable is a controlled Rust child."
    );
    report("project registration", &projects);
    report("project list", &enumeration);
    report("task registration", &tasks.registration);
    report("task start dispatch (includes OS spawn)", &tasks.start);
    report(
        "task completion (includes process startup and exit)",
        &tasks.completion,
    );
    report(
        "direct process startup and exit baseline",
        &tasks.process_only,
    );
    report("output processing (32 KiB stdout + stderr)", &tasks.output);
    println!(
        "four concurrent task completion: {:.1} us",
        tasks.concurrent.as_secs_f64() * 1e6
    );
    let _ = fs::remove_dir_all(root);
}
