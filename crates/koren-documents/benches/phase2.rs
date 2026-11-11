//! Reproducible local baseline workloads for Phase 2 workspace and document operations.

use koren_documents::DocumentManager;
use koren_protocol::{TextPosition, TextRange};
use koren_workspace::WorkspaceManager;
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "koren-phase2-bench-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock must be after Unix epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("create benchmark directory");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn measure(label: &str, iterations: u32, mut operation: impl FnMut()) {
    let mut samples =
        Vec::with_capacity(usize::try_from(iterations).expect("iteration count fits usize"));
    for _ in 0..iterations {
        let start = Instant::now();
        operation();
        samples.push(start.elapsed());
    }
    let total = samples.iter().copied().sum::<Duration>();
    let mean = total.as_secs_f64() * 1_000_000.0 / f64::from(iterations);
    let min = samples
        .iter()
        .copied()
        .min()
        .unwrap_or_default()
        .as_secs_f64()
        * 1_000_000.0;
    println!("{label},iterations={iterations},mean_us={mean:.1},min_us={min:.1}");
}

fn prepare_fixtures(temp: &TempDir) {
    let small_file = temp.0.join("representative.rs");
    let large_file = temp.0.join("large-document.txt");
    let navigation = temp.0.join("navigation");
    fs::create_dir(&navigation).expect("create navigation fixture");
    let one_mib = "a".repeat(1024 * 1024);
    let eight_mib = "b".repeat(8 * 1024 * 1024);
    fs::write(small_file, one_mib).expect("write representative file");
    fs::write(large_file, &eight_mib).expect("write large document");
    for index in 0..500 {
        fs::write(navigation.join(format!("entry-{index:04}.txt")), "entry")
            .expect("write directory fixture");
    }
}

fn workspace_and_open_benchmarks(temp: &TempDir) -> WorkspaceManager {
    measure("workspace_open", 50, || {
        let mut workspace = WorkspaceManager::new();
        workspace.open(&temp.0).expect("open workspace");
        workspace.close().expect("close workspace");
    });

    let mut workspace = WorkspaceManager::new();
    workspace.open(&temp.0).expect("open benchmark workspace");
    measure("directory_list_500_entries", 50, || {
        std::hint::black_box(workspace.list_dir("navigation").expect("list directory"));
    });

    measure("open_text_file_1mib", 20, || {
        let mut documents = DocumentManager::new();
        let id = documents
            .open_file(&workspace, "representative.rs")
            .expect("open text file");
        std::hint::black_box(documents.info(id).expect("read metadata"));
    });

    workspace
}

fn edit_benchmarks(workspace: &WorkspaceManager) {
    let mut documents = DocumentManager::new();
    let large_id = documents
        .open_file(workspace, "large-document.txt")
        .expect("open large document");
    measure("small_edit_8mib", 100, || {
        let revision = documents.info(large_id).expect("read revision").revision;
        std::hint::black_box(
            documents
                .replace_range(
                    large_id,
                    revision,
                    TextRange {
                        start: TextPosition {
                            line: 0,
                            character: 4 * 1024 * 1024,
                        },
                        end: TextPosition {
                            line: 0,
                            character: 4 * 1024 * 1024 + 1,
                        },
                    },
                    "x",
                )
                .expect("small edit"),
        );
    });
    let replacement = "z".repeat(256 * 1024);
    measure("large_edit_256kib_in_8mib", 10, || {
        let revision = documents.info(large_id).expect("read revision").revision;
        std::hint::black_box(
            documents
                .replace_range(
                    large_id,
                    revision,
                    TextRange {
                        start: TextPosition {
                            line: 0,
                            character: 2 * 1024 * 1024,
                        },
                        end: TextPosition {
                            line: 0,
                            character: 2 * 1024 * 1024 + 256 * 1024,
                        },
                    },
                    &replacement,
                )
                .expect("large edit"),
        );
    });
}

fn save_benchmark(temp: &TempDir) {
    let save_file = temp.0.join("save.txt");
    fs::write(&save_file, "s".repeat(1024 * 1024)).expect("write save fixture");
    let mut save_workspace = WorkspaceManager::new();
    save_workspace.open(&temp.0).expect("open save workspace");
    let mut save_documents = DocumentManager::new();
    let save_id = save_documents
        .open_file(&save_workspace, "save.txt")
        .expect("open save fixture");
    measure("save_modified_document", 20, || {
        let revision = save_documents
            .info(save_id)
            .expect("read save revision")
            .revision;
        save_documents
            .insert_text(
                save_id,
                revision,
                TextPosition {
                    line: 0,
                    character: 1024 * 1024,
                },
                ".",
            )
            .expect("edit save fixture");
        save_documents
            .save(save_id, revision + 1)
            .expect("save fixture");
    });
}

fn main() {
    let temp = TempDir::new();
    prepare_fixtures(&temp);
    let workspace = workspace_and_open_benchmarks(&temp);
    edit_benchmarks(&workspace);
    save_benchmark(&temp);
}
