//! Phase 9A local Git discovery and operation benchmarks.
use git2::{Repository, Signature};
use koren_git::GitService;
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
fn root() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "koren-git-bench-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&p).unwrap();
    p
}
fn report(name: &str, samples: &[Duration]) {
    println!(
        "{name}: {} iterations, mean {:.2} ms, min {:.2} ms",
        samples.len(),
        samples.iter().copied().sum::<Duration>().as_secs_f64() * 1e3
            / f64::from(u32::try_from(samples.len()).unwrap()),
        samples.iter().copied().min().unwrap().as_secs_f64() * 1e3
    );
}
fn main() {
    let temp = root();
    let repo = Repository::init(&temp).unwrap();
    repo.config()
        .unwrap()
        .set_str("user.name", "Benchmark")
        .unwrap();
    repo.config()
        .unwrap()
        .set_str("user.email", "bench@example.invalid")
        .unwrap();
    for i in 0..500 {
        fs::write(temp.join(format!("f{i}.txt")), "base\n").unwrap();
    }
    let mut index = repo.index().unwrap();
    for i in 0..500 {
        index
            .add_path(std::path::Path::new(&format!("f{i}.txt")))
            .unwrap();
    }
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = Signature::now("Benchmark", "bench@example.invalid").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "fixture", &tree, &[])
        .unwrap();
    drop(tree);
    fs::write(temp.join("f0.txt"), "changed\n").unwrap();
    let service = GitService::new(&temp).unwrap();
    let git = service.open(".").unwrap();
    let mut detect = Vec::new();
    for _ in 0..30 {
        let start = Instant::now();
        let _ = service.discover().unwrap();
        detect.push(start.elapsed());
    }
    report("repository detection (500 files)", &detect);
    let mut status = Vec::new();
    for _ in 0..50 {
        let start = Instant::now();
        let _ = git.status().unwrap();
        status.push(start.elapsed());
    }
    report("status (500 tracked files)", &status);
    let mut diff = Vec::new();
    for _ in 0..50 {
        let start = Instant::now();
        let _ = git.diff("f0.txt", false).unwrap();
        diff.push(start.elapsed());
    }
    report("one-file diff", &diff);
    let mut staging = Vec::new();
    for _ in 0..50 {
        let start = Instant::now();
        git.stage("f0.txt").unwrap();
        git.unstage("f0.txt").unwrap();
        staging.push(start.elapsed());
    }
    report("stage plus unstage", &staging);
    drop(git);
    drop(service);
    let _ = fs::remove_dir_all(temp);
}
