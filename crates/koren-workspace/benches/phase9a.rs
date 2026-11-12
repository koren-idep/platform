//! Phase 9A local filesystem mutation benchmarks.
use koren_workspace::WorkspaceManager;
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn temp_root() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "koren-workspace-bench-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}
fn report(name: &str, samples: &[Duration]) {
    let total = samples.iter().copied().sum::<Duration>();
    println!(
        "{name}: {} iterations, mean {:.1} us, min {:.1} us",
        samples.len(),
        total.as_secs_f64() * 1e6 / f64::from(u32::try_from(samples.len()).unwrap()),
        samples.iter().copied().min().unwrap().as_secs_f64() * 1e6
    );
}
fn main() {
    let root = temp_root();
    let mut workspace = WorkspaceManager::new();
    workspace.open(&root).unwrap();
    let mut create_rename = Vec::new();
    for index in 0..500 {
        let source = format!("created-{index}.txt");
        let destination = format!("renamed-{index}.txt");
        let start = Instant::now();
        workspace.create_file(&source).unwrap();
        workspace.rename(&source, &destination).unwrap();
        create_rename.push(start.elapsed());
    }
    report("file create plus rename", &create_rename);
    let mut directory_create = Vec::new();
    for index in 0..250 {
        let start = Instant::now();
        workspace.create_directory(format!("dir-{index}")).unwrap();
        directory_create.push(start.elapsed());
    }
    report("directory create", &directory_create);
    workspace.close().unwrap();
    let _ = fs::remove_dir_all(root);
}
