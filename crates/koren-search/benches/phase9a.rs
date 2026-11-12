//! Phase 9A local project search and cancellation benchmarks.
use koren_search::{SearchEvent, SearchOptions, SearchService};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
fn root() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "koren-search-bench-{}-{}",
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
    let fixture = root();
    for dir in 0..20 {
        fs::create_dir(fixture.join(format!("d{dir}"))).unwrap();
        for file in 0..50 {
            fs::write(
                fixture.join(format!("d{dir}/f{file}.txt")),
                format!(
                    "other text\n{}\n",
                    if file % 10 == 0 {
                        "target needle"
                    } else {
                        "not a match"
                    }
                ),
            )
            .unwrap();
        }
    }
    let service = SearchService::new(&fixture).unwrap();
    let mut search = Vec::new();
    for _ in 0..12 {
        let start = Instant::now();
        let job = service.search("needle", SearchOptions::default()).unwrap();
        let mut complete = false;
        while !complete {
            if matches!(
                job.recv_timeout(Duration::from_secs(10)).unwrap(),
                SearchEvent::Completed(_)
            ) {
                complete = true;
            }
        }
        search.push(start.elapsed());
    }
    report("search 1,000 text files", &search);
    let mut cancellation = Vec::new();
    for _ in 0..12 {
        let start = Instant::now();
        let job = service.search("other", SearchOptions::default()).unwrap();
        job.cancel();
        let mut complete = false;
        while !complete {
            if let Ok(SearchEvent::Completed(_)) = job.recv_timeout(Duration::from_secs(10)) {
                complete = true;
            }
        }
        cancellation.push(start.elapsed());
    }
    report("search cancellation", &cancellation);
    let _ = fs::remove_dir_all(fixture);
}
