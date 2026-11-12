//! Bounded, cancellable text search over local workspace trees.

use crossbeam_channel::{Receiver, RecvTimeoutError, SendTimeoutError, Sender, bounded};
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

/// Maximum query length in bytes.
pub const MAX_QUERY_BYTES: usize = 4096;
/// Maximum file glob length.
pub const MAX_FILE_FILTER_BYTES: usize = 512;
/// Maximum number of emitted matches for one search.
pub const MAX_SEARCH_RESULTS: usize = 20_000;
/// Maximum source bytes read from any one file.
pub const MAX_SEARCH_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Maximum number of files considered per search.
pub const MAX_SEARCH_FILES: usize = 100_000;
/// Maximum directory and entry visits per search.
pub const MAX_SEARCH_ENTRIES: usize = 200_000;
/// Maximum search recursion depth.
pub const MAX_SEARCH_DEPTH: usize = 128;
const RESULT_CHANNEL_CAPACITY: usize = 128;
const MAX_PREVIEW_CHARS: usize = 240;

/// Search options for one explicit workspace query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchOptions {
    /// Match uppercase and lowercase characters distinctly.
    pub case_sensitive: bool,
    /// Parse the query as a regular expression instead of literal text.
    pub regular_expression: bool,
    /// Optional glob matched against workspace-relative paths.
    pub file_filter: Option<String>,
    /// Maximum results emitted; clamped to [`MAX_SEARCH_RESULTS`].
    pub result_limit: usize,
    /// Maximum file size read; clamped to [`MAX_SEARCH_FILE_BYTES`].
    pub file_size_limit: u64,
    /// Maximum number of files traversed; clamped to [`MAX_SEARCH_FILES`].
    pub file_limit: usize,
    /// Maximum directory depth; clamped to [`MAX_SEARCH_DEPTH`].
    pub depth_limit: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            regular_expression: false,
            file_filter: None,
            result_limit: 5_000,
            file_size_limit: 2 * 1024 * 1024,
            file_limit: 25_000,
            depth_limit: 64,
        }
    }
}

/// One literal or regular-expression match.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchResult {
    /// Workspace-relative UTF-8 path.
    pub path: String,
    /// One-based source line number.
    pub line: usize,
    /// Zero-based UTF-16 code-unit column.
    pub column_utf16: usize,
    /// Match length in UTF-16 code units.
    pub match_length_utf16: usize,
    /// Bounded source line preview.
    pub preview: String,
}

/// Search progress snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchProgress {
    /// Files fully considered so far.
    pub files_scanned: usize,
    /// Matches emitted so far.
    pub matches_found: usize,
    /// Most recently scanned relative path, if available.
    pub current_path: Option<String>,
}

/// Final search summary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchSummary {
    /// Files considered.
    pub files_scanned: usize,
    /// Matches emitted.
    pub matches_found: usize,
    /// Files skipped because they were binary or invalid UTF-8.
    pub skipped_binary_files: usize,
    /// Files skipped because they exceeded the configured size limit.
    pub skipped_large_files: usize,
    /// Files skipped because their path could not be represented as UTF-8.
    pub skipped_non_utf8_paths: usize,
    /// Traversal or read errors encountered.
    pub errors: usize,
    /// Whether the result limit stopped the scan early.
    pub truncated: bool,
    /// Whether cancellation stopped the scan.
    pub cancelled: bool,
}

/// Incremental message delivered by a search job.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SearchEvent {
    /// A matching source range.
    Match(SearchResult),
    /// A coalescible progress update.
    Progress(SearchProgress),
    /// Terminal summary.
    Completed(SearchSummary),
    /// Query or filesystem error.
    Error(String),
}

/// Errors that prevent a search job from starting.
#[derive(Debug)]
pub enum SearchError {
    /// Search text is empty or exceeds the query limit.
    InvalidQuery,
    /// A regular-expression pattern or file glob did not compile.
    InvalidPattern(String),
    /// No valid result capacity was provided.
    InvalidLimit,
    /// A search is already running or the worker could not start.
    Worker(String),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidQuery => {
                f.write_str("search query must be non-empty and within the query size limit")
            }
            Self::InvalidPattern(message) => write!(f, "invalid search pattern: {message}"),
            Self::InvalidLimit => f.write_str("search result limit must be greater than zero"),
            Self::Worker(message) => write!(f, "search worker failed: {message}"),
        }
    }
}

impl std::error::Error for SearchError {}

/// Explicit search service rooted at one canonical workspace directory.
#[derive(Clone, Debug)]
pub struct SearchService {
    root: Arc<PathBuf>,
    active_jobs: Arc<AtomicUsize>,
    max_concurrent_jobs: usize,
}

impl SearchService {
    /// Creates a service for an existing local directory.
    ///
    /// # Errors
    /// Returns an error if the root cannot be canonicalized or is not a directory.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, SearchError> {
        let root = std::fs::canonicalize(root.as_ref())
            .map_err(|error| SearchError::Worker(error.to_string()))?;
        if !root.is_dir() {
            return Err(SearchError::Worker("search root is not a directory".into()));
        }
        Ok(Self {
            root: Arc::new(root),
            active_jobs: Arc::new(AtomicUsize::new(0)),
            max_concurrent_jobs: 2,
        })
    }

    /// Starts a bounded worker for one explicit query.
    ///
    /// Traversal honors `.gitignore`, `.ignore`, Git exclude files and global Git ignores by
    /// default. It never follows symlinks and always prunes `.git` metadata directories.
    /// Binary, invalid UTF-8 and oversized files are counted in the final summary and skipped.
    ///
    /// # Errors
    /// Returns an error for invalid query/options, malformed patterns, concurrency limits or
    /// worker startup failure.
    pub fn search(&self, query: &str, options: SearchOptions) -> Result<SearchJob, SearchError> {
        if query.trim().is_empty() || query.len() > MAX_QUERY_BYTES {
            return Err(SearchError::InvalidQuery);
        }
        if options.result_limit == 0 || options.file_limit == 0 {
            return Err(SearchError::InvalidLimit);
        }
        if options
            .file_filter
            .as_ref()
            .is_some_and(|filter| filter.len() > MAX_FILE_FILTER_BYTES)
        {
            return Err(SearchError::InvalidPattern(
                "file filter exceeds the 512-byte limit".into(),
            ));
        }
        let matcher = if options.regular_expression {
            query.to_owned()
        } else {
            regex::escape(query)
        };
        let regex = RegexBuilder::new(&matcher)
            .case_insensitive(!options.case_sensitive)
            .build()
            .map_err(|error| SearchError::InvalidPattern(error.to_string()))?;
        let file_glob = options
            .file_filter
            .as_deref()
            .map(glob::Pattern::new)
            .transpose()
            .map_err(|error| SearchError::InvalidPattern(error.to_string()))?;
        let previous = self.active_jobs.fetch_add(1, Ordering::AcqRel);
        if previous >= self.max_concurrent_jobs {
            self.active_jobs.fetch_sub(1, Ordering::AcqRel);
            return Err(SearchError::Worker(
                "the workspace search concurrency limit is full".into(),
            ));
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let root = self.root.clone();
        let active_jobs = self.active_jobs.clone();
        let (sender, receiver) = bounded(RESULT_CHANNEL_CAPACITY);
        let options = BoundedOptions::from(options);
        let spawn = thread::Builder::new()
            .name("koren-workspace-search".into())
            .spawn(move || {
                let _guard = ActiveJob(active_jobs);
                run_search(
                    &root,
                    &regex,
                    file_glob.as_ref(),
                    options,
                    &worker_cancelled,
                    &sender,
                );
            });
        if let Err(error) = spawn {
            self.active_jobs.fetch_sub(1, Ordering::AcqRel);
            return Err(SearchError::Worker(error.to_string()));
        }
        Ok(SearchJob {
            receiver,
            cancelled,
        })
    }
}

/// Cancellable handle for one bounded search.
pub struct SearchJob {
    receiver: Receiver<SearchEvent>,
    cancelled: Arc<AtomicBool>,
}

impl SearchJob {
    /// Requests cooperative cancellation.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Receives the next incremental event without blocking.
    #[must_use]
    pub fn try_recv(&self) -> Option<SearchEvent> {
        self.receiver.try_recv().ok()
    }

    /// Receives the next event with a caller-selected timeout.
    ///
    /// # Errors
    /// Returns [`SearchReceiveError::Timeout`] if no event arrives before the timeout, or
    /// [`SearchReceiveError::Completed`] after the worker has disconnected.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<SearchEvent, SearchReceiveError> {
        self.receiver
            .recv_timeout(timeout)
            .map_err(|error| match error {
                RecvTimeoutError::Timeout => SearchReceiveError::Timeout,
                RecvTimeoutError::Disconnected => SearchReceiveError::Completed,
            })
    }
}

impl Drop for SearchJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Search result receive state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchReceiveError {
    /// No event arrived in the requested interval.
    Timeout,
    /// The worker has stopped and its message channel is closed.
    Completed,
}

#[derive(Clone, Copy)]
struct BoundedOptions {
    results: usize,
    file_bytes: u64,
    files: usize,
    depth: usize,
}

impl From<SearchOptions> for BoundedOptions {
    fn from(options: SearchOptions) -> Self {
        Self {
            results: options.result_limit.min(MAX_SEARCH_RESULTS),
            file_bytes: options.file_size_limit.min(MAX_SEARCH_FILE_BYTES),
            files: options.file_limit.min(MAX_SEARCH_FILES),
            depth: options.depth_limit.min(MAX_SEARCH_DEPTH),
        }
    }
}

struct ActiveJob(Arc<AtomicUsize>);
impl Drop for ActiveJob {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct SearchRun<'a> {
    root: &'a Path,
    regex: &'a Regex,
    file_glob: Option<&'a glob::Pattern>,
    options: BoundedOptions,
    cancelled: &'a AtomicBool,
    sender: &'a Sender<SearchEvent>,
    summary: SearchSummary,
}

fn run_search(
    root: &Path,
    regex: &Regex,
    file_glob: Option<&glob::Pattern>,
    options: BoundedOptions,
    cancelled: &AtomicBool,
    sender: &Sender<SearchEvent>,
) {
    let mut walker = WalkBuilder::new(root);
    walker
        .hidden(false)
        .follow_links(false)
        .require_git(false)
        .max_depth(Some(options.depth));
    walker.filter_entry(|entry| {
        !(entry.file_type().is_some_and(|kind| kind.is_dir())
            && matches!(
                entry.file_name().to_str(),
                Some(".git" | "target" | "node_modules" | "dist" | "build" | ".cache")
            ))
    });
    let mut run = SearchRun {
        root,
        regex,
        file_glob,
        options,
        cancelled,
        sender,
        summary: SearchSummary {
            files_scanned: 0,
            matches_found: 0,
            skipped_binary_files: 0,
            skipped_large_files: 0,
            skipped_non_utf8_paths: 0,
            errors: 0,
            truncated: false,
            cancelled: false,
        },
    };
    let mut visited_entries = 0usize;
    for entry in walker.build() {
        if run.cancelled.load(Ordering::Acquire) || run.summary.truncated {
            break;
        }
        visited_entries = visited_entries.saturating_add(1);
        if visited_entries > MAX_SEARCH_ENTRIES {
            run.summary.truncated = true;
            break;
        }
        let Ok(entry) = entry else {
            run.summary.errors += 1;
            continue;
        };
        if !process_entry(&entry, &mut run) {
            break;
        }
    }
    run.summary.cancelled = run.cancelled.load(Ordering::Acquire);
    let _ = send_terminal(run.sender, SearchEvent::Completed(run.summary));
}

fn process_entry(entry: &ignore::DirEntry, run: &mut SearchRun<'_>) -> bool {
    let path = entry.path();
    if entry.file_type().is_some_and(|kind| kind.is_dir())
        && path.file_name().is_some_and(|name| {
            matches!(
                name.to_str(),
                Some(".git" | "target" | "node_modules" | "dist" | "build" | ".cache")
            )
        })
    {
        return true;
    }
    if !entry.file_type().is_some_and(|kind| kind.is_file()) {
        return true;
    }
    let Ok(relative) = path.strip_prefix(run.root) else {
        run.summary.errors += 1;
        return true;
    };
    let Some(relative_text) = relative.to_str() else {
        run.summary.skipped_non_utf8_paths += 1;
        return true;
    };
    if run.file_glob.is_some_and(|pattern| {
        !pattern.matches_path(relative)
            && !relative
                .file_name()
                .is_some_and(|name| pattern.matches_path(Path::new(name)))
    }) {
        return true;
    }
    if run.summary.files_scanned >= run.options.files {
        run.summary.truncated = true;
        return false;
    }
    run.summary.files_scanned += 1;
    scan_file(entry, path, relative_text, run);
    !run.summary.truncated && !run.cancelled.load(Ordering::Acquire)
}

fn scan_file(entry: &ignore::DirEntry, path: &Path, relative_text: &str, run: &mut SearchRun<'_>) {
    let SearchRun {
        regex,
        options,
        cancelled,
        sender,
        summary,
        ..
    } = run;
    if let Ok(metadata) = entry.metadata() {
        if metadata.len() > options.file_bytes {
            summary.skipped_large_files += 1;
            return;
        }
    }
    if cancelled.load(Ordering::Acquire) {
        return;
    }
    let mut bytes = Vec::new();
    let read = File::open(path).and_then(|file| {
        file.take(options.file_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
    });
    if read.is_err() {
        summary.errors += 1;
        return;
    }
    if bytes.len() as u64 > options.file_bytes {
        summary.skipped_large_files += 1;
        return;
    }
    if bytes.contains(&0) {
        summary.skipped_binary_files += 1;
        return;
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        summary.skipped_binary_files += 1;
        return;
    };
    for (line_index, line) in text.lines().enumerate() {
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        for found in regex.find_iter(line) {
            let result = SearchResult {
                path: relative_text.replace('\\', "/"),
                line: line_index + 1,
                column_utf16: line[..found.start()].encode_utf16().count(),
                match_length_utf16: line[found.start()..found.end()].encode_utf16().count(),
                preview: preview_line(line),
            };
            if !send_match(sender, result, cancelled) {
                break;
            }
            summary.matches_found += 1;
            if summary.matches_found >= options.results {
                summary.truncated = true;
                break;
            }
        }
        if cancelled.load(Ordering::Acquire) || summary.truncated {
            break;
        }
    }
    let _ = sender.try_send(SearchEvent::Progress(SearchProgress {
        files_scanned: summary.files_scanned,
        matches_found: summary.matches_found,
        current_path: Some(relative_text.replace('\\', "/")),
    }));
}

fn send_match(sender: &Sender<SearchEvent>, result: SearchResult, cancelled: &AtomicBool) -> bool {
    send_until_cancelled(sender, SearchEvent::Match(result), cancelled)
}

fn send_until_cancelled(
    sender: &Sender<SearchEvent>,
    mut event: SearchEvent,
    cancelled: &AtomicBool,
) -> bool {
    loop {
        if cancelled.load(Ordering::Acquire) {
            return false;
        }
        match sender.send_timeout(event, Duration::from_millis(50)) {
            Ok(()) => return true,
            Err(SendTimeoutError::Timeout(returned)) => event = returned,
            Err(SendTimeoutError::Disconnected(_)) => return false,
        }
    }
}

fn send_terminal(sender: &Sender<SearchEvent>, mut event: SearchEvent) -> bool {
    loop {
        match sender.send_timeout(event, Duration::from_millis(50)) {
            Ok(()) => return true,
            Err(SendTimeoutError::Timeout(returned)) => event = returned,
            Err(SendTimeoutError::Disconnected(_)) => return false,
        }
    }
}

fn preview_line(line: &str) -> String {
    let mut preview = line.chars().take(MAX_PREVIEW_CHARS).collect::<String>();
    if line.chars().count() > MAX_PREVIEW_CHARS {
        preview.push('…');
    }
    preview
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "koren-search-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn literal_regex_filters_ignore_binary_and_reports_utf16_columns() {
        let temp = Temp::new();
        fs::write(temp.0.join(".gitignore"), "ignored/\n").unwrap();
        fs::create_dir(temp.0.join("src")).unwrap();
        fs::create_dir(temp.0.join("ignored")).unwrap();
        fs::create_dir(temp.0.join("target")).unwrap();
        fs::create_dir(temp.0.join("node_modules")).unwrap();
        fs::write(temp.0.join("src/main.rs"), "😀 needle\nNeedle\n").unwrap();
        fs::write(temp.0.join("src/image.bin"), b"needle\0binary").unwrap();
        fs::write(temp.0.join("ignored/no.txt"), "needle").unwrap();
        fs::write(temp.0.join("target/generated.txt"), "needle").unwrap();
        fs::write(temp.0.join("node_modules/dependency.txt"), "needle").unwrap();
        let service = SearchService::new(&temp.0).unwrap();
        let job = service
            .search(
                "needle",
                SearchOptions {
                    file_filter: Some("*.rs".into()),
                    ..SearchOptions::default()
                },
            )
            .unwrap();
        let mut results = Vec::new();
        let mut summary = None;
        while let Ok(event) = job.recv_timeout(Duration::from_secs(2)) {
            match event {
                SearchEvent::Match(result) => results.push(result),
                SearchEvent::Completed(value) => {
                    summary = Some(value);
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(results.len(), 2);
        assert_eq!((results[0].line, results[0].column_utf16), (1, 3));
        assert_eq!(results[0].path, "src/main.rs");
        assert_eq!(
            summary.unwrap().skipped_binary_files,
            0,
            "file glob excludes the binary fixture"
        );

        let all_text = service.search("needle", SearchOptions::default()).unwrap();
        let mut paths = Vec::new();
        while let Ok(event) = all_text.recv_timeout(Duration::from_secs(2)) {
            match event {
                SearchEvent::Match(found) => paths.push(found.path),
                SearchEvent::Completed(_) => break,
                _ => {}
            }
        }
        paths.sort();
        assert_eq!(paths, ["src/main.rs", "src/main.rs"]);

        let regex = service
            .search(
                "N.*e",
                SearchOptions {
                    regular_expression: true,
                    case_sensitive: true,
                    file_filter: Some("*.rs".into()),
                    ..SearchOptions::default()
                },
            )
            .unwrap();
        let mut regex_count = 0;
        while let Ok(event) = regex.recv_timeout(Duration::from_secs(2)) {
            if matches!(event, SearchEvent::Match(_)) {
                regex_count += 1;
            }
            if matches!(event, SearchEvent::Completed(_)) {
                break;
            }
        }
        assert_eq!(regex_count, 1);
    }

    #[test]
    fn search_cancellation_and_limits_are_bounded() {
        let temp = Temp::new();
        fs::write(temp.0.join("many.txt"), "match\n".repeat(10_000)).unwrap();
        let service = SearchService::new(&temp.0).unwrap();
        let job = service
            .search(
                "match",
                SearchOptions {
                    result_limit: 2,
                    ..SearchOptions::default()
                },
            )
            .unwrap();
        let mut matches = 0;
        let mut truncated = false;
        while let Ok(event) = job.recv_timeout(Duration::from_secs(2)) {
            match event {
                SearchEvent::Match(_) => matches += 1,
                SearchEvent::Completed(summary) => {
                    truncated = summary.truncated;
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(matches, 2);
        assert!(truncated);
        assert!(
            service
                .search(
                    "[",
                    SearchOptions {
                        regular_expression: true,
                        ..SearchOptions::default()
                    }
                )
                .is_err()
        );
        assert!(service.search(" ", SearchOptions::default()).is_err());
        let cancelled = service.search("match", SearchOptions::default()).unwrap();
        cancelled.cancel();
        let mut finished_cancelled = false;
        while let Ok(event) = cancelled.recv_timeout(Duration::from_secs(1)) {
            if let SearchEvent::Completed(summary) = event {
                finished_cancelled = summary.cancelled;
                break;
            }
        }
        assert!(finished_cancelled);
    }
}
