//! Native, bounded PTY sessions for language-neutral Koren Platform hosts.
//!
//! Hosts must authorize a shell profile and working directory before starting a session.
//! An approved shell can run arbitrary commands with the user's account permissions; terminal
//! sessions are not a workspace sandbox.

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// Maximum raw bytes accepted by one input request.
pub const MAX_TERMINAL_INPUT_BYTES: usize = 64 * 1024;
/// Maximum bytes read from the PTY before backpressure is applied.
pub const MAX_TERMINAL_OUTPUT_CHUNK_BYTES: usize = 16 * 1024;
/// Maximum bytes delivered by one event drain.
pub const MAX_TERMINAL_OUTPUT_BYTES_PER_DRAIN: usize = 128 * 1024;
/// Hard ceiling for caller-configured simultaneous sessions.
pub const MAX_TERMINAL_SESSIONS: usize = 32;
/// Hard ceiling for caller-configured buffered output chunks per session.
pub const MAX_TERMINAL_OUTPUT_QUEUE_CHUNKS: usize = 128;
/// Hard ceiling for caller-configured buffered input chunks per session.
pub const MAX_TERMINAL_INPUT_QUEUE_CHUNKS: usize = 64;
const SHUTDOWN_GRACE: Duration = Duration::from_millis(150);
const WORKER_JOIN_GRACE: Duration = Duration::from_millis(300);
const MAX_DIMENSION: u16 = 500;

/// Manager-local terminal session identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TerminalId(u64);

impl TerminalId {
    /// Numeric session identifier.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Reconstructs an identifier received through a trusted host protocol.
    #[must_use]
    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }
}

/// Initial-directory behavior for a terminal profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalWorkingDirectory {
    /// Use the active workspace root, or the user's home when no workspace is open.
    Workspace,
    /// Use the user's home directory.
    Home,
    /// Use an explicit directory.
    Fixed(PathBuf),
}

/// Configured shell profile. Arguments remain a direct argv vector and are never shell-expanded.
#[derive(Clone, Eq, PartialEq)]
pub struct TerminalProfile {
    /// Stable profile identifier.
    pub id: String,
    /// Name shown in the UI and approval prompt.
    pub name: String,
    /// Shell executable path or name resolved by the operating system.
    pub executable: PathBuf,
    /// Arguments passed directly to the executable.
    pub arguments: Vec<String>,
    /// Initial working-directory behavior.
    pub working_directory: TerminalWorkingDirectory,
    /// Explicit environment overrides; values are redacted from debug and approval summaries.
    pub environment: BTreeMap<String, String>,
    /// Optional fallback tried only by the detected default profile.
    pub fallback_executable: Option<PathBuf>,
}

impl fmt::Debug for TerminalProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TerminalProfile")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("executable", &self.executable)
            .field("arguments", &self.arguments)
            .field("working_directory", &self.working_directory)
            .field(
                "environment_keys",
                &self.environment.keys().collect::<Vec<_>>(),
            )
            .field("fallback_executable", &self.fallback_executable)
            .finish()
    }
}

impl TerminalProfile {
    /// Detects the configured shell without starting it.
    ///
    /// Unix uses SHELL and falls back to /bin/sh. Windows uses COMSPEC and falls back to cmd.exe.
    /// A startup failure retries the fallback. Shell startup files are not read or modified.
    #[must_use]
    pub fn detected_default() -> Self {
        #[cfg(windows)]
        let (executable, fallback_executable) = {
            let fallback = PathBuf::from("cmd.exe");
            match std::env::var_os("COMSPEC").map(PathBuf::from) {
                Some(path) if path != fallback => (path, Some(fallback)),
                Some(path) => (path, None),
                None => (fallback, None),
            }
        };
        #[cfg(unix)]
        let (executable, fallback_executable) = {
            let fallback = PathBuf::from("/bin/sh");
            match std::env::var_os("SHELL")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
            {
                Some(path) if path != fallback => (path, Some(fallback)),
                Some(path) => (path, None),
                None => (fallback, None),
            }
        };
        #[cfg(not(any(unix, windows)))]
        let (executable, fallback_executable) = (PathBuf::from("sh"), None);

        Self {
            id: "default".into(),
            name: "Terminal".into(),
            executable,
            arguments: Vec::new(),
            working_directory: TerminalWorkingDirectory::Workspace,
            environment: BTreeMap::new(),
            fallback_executable,
        }
    }

    /// Returns environment override names, never their values.
    #[must_use]
    pub fn environment_keys(&self) -> Vec<String> {
        self.environment.keys().cloned().collect()
    }
}

/// Terminal session lifecycle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalState {
    /// Reserved without allocating a PTY or starting a process.
    Created,
    /// PTY and shell startup are in progress.
    Starting,
    /// Shell is running.
    Running,
    /// Shell exited normally or with a nonzero status.
    Exited,
    /// Shell startup or PTY I/O failed.
    Failed,
    /// Host explicitly terminated the session.
    Terminated,
}

impl TerminalState {
    /// Whether the state is terminal.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        matches!(self, Self::Exited | Self::Failed | Self::Terminated)
    }
}

/// Current public view of a terminal session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalSessionInfo {
    /// Session identity.
    pub id: TerminalId,
    /// Configured profile identity.
    pub profile_id: String,
    /// Resolved initial working directory.
    pub working_directory: PathBuf,
    /// Current lifecycle state.
    pub state: TerminalState,
    /// Exit status when available.
    pub exit_code: Option<u32>,
    /// Safe-to-display startup or PTY error.
    pub error: Option<String>,
    /// Whether the owned PTY process group still has processes (Unix only).
    pub process_group_active: bool,
    /// Current row count.
    pub rows: u16,
    /// Current column count.
    pub columns: u16,
}

/// Terminal service resource limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalOptions {
    /// Maximum simultaneous sessions.
    pub max_sessions: usize,
    /// Bounded output chunks per session. A full queue blocks the reader and PTY writer.
    pub output_queue_chunks: usize,
    /// Bounded input chunks per session. A full queue returns Backpressure.
    pub input_queue_chunks: usize,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            max_sessions: 8,
            output_queue_chunks: 64,
            input_queue_chunks: 32,
        }
    }
}

/// Terminal service failure.
#[derive(Debug, Eq, PartialEq)]
pub enum TerminalError {
    /// Invalid profile, request, or service configuration.
    Invalid(String),
    /// Session or profile does not exist.
    NotFound,
    /// Operation is invalid for this state.
    InvalidState(TerminalState),
    /// Caller must confirm termination of a live process.
    ActiveSession,
    /// Bounded input queue is full; retry the same bytes.
    Backpressure,
    /// PTY, process, or filesystem operation failed.
    Io(String),
}

impl fmt::Display for TerminalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) | Self::Io(message) => f.write_str(message),
            Self::NotFound => f.write_str("terminal session or profile was not found"),
            Self::InvalidState(state) => write!(f, "terminal session is {state:?}"),
            Self::ActiveSession => f.write_str("terminal process is still active"),
            Self::Backpressure => f.write_str("terminal input queue is full; retry shortly"),
        }
    }
}

impl std::error::Error for TerminalError {}

/// Ordered event from a terminal session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalEvent {
    /// Bounded raw PTY output. Sequence starts at one per session.
    Output {
        /// Session identity.
        id: TerminalId,
        /// Per-session output sequence.
        sequence: u64,
        /// Raw PTY bytes.
        bytes: Vec<u8>,
    },
    /// Final state, delivered after previously queued output.
    StateChanged {
        /// Session identity.
        id: TerminalId,
        /// Final state.
        state: TerminalState,
        /// Exit code when available.
        exit_code: Option<u32>,
        /// Whether the owned process group still has live processes.
        process_group_active: bool,
        /// Safe-to-display failure message.
        error: Option<String>,
    },
    /// Session process and handles were closed.
    Closed {
        /// Session identity.
        id: TerminalId,
    },
}

#[derive(Debug)]
enum ReaderMessage {
    Data(Vec<u8>),
    End,
    Error(String),
}

struct Session {
    profile: TerminalProfile,
    working_directory: PathBuf,
    state: TerminalState,
    exit_code: Option<u32>,
    error: Option<String>,
    rows: u16,
    columns: u16,
    child: Option<Box<dyn Child + Send + Sync>>,
    master: Option<Box<dyn MasterPty + Send>>,
    process_group: Option<i32>,
    output_receiver: Option<Receiver<ReaderMessage>>,
    pending_output: VecDeque<u8>,
    input_sender: Option<SyncSender<Vec<u8>>>,
    reader_thread: Option<JoinHandle<()>>,
    writer_thread: Option<JoinHandle<()>>,
    output_sequence: u64,
    state_notified: bool,
    reader_finished: bool,
}

/// Owns bounded local PTY sessions. Construction and profile registration start no shell or worker.
pub struct TerminalManager {
    options: TerminalOptions,
    next_id: u64,
    profiles: BTreeMap<String, TerminalProfile>,
    sessions: BTreeMap<TerminalId, Session>,
    closed_events: VecDeque<TerminalId>,
}

impl fmt::Debug for TerminalManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TerminalManager")
            .field("session_count", &self.sessions.len())
            .field("profile_ids", &self.profiles.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Default for TerminalManager {
    fn default() -> Self {
        Self::new(TerminalOptions::default()).expect("default terminal limits are valid")
    }
}

impl TerminalManager {
    /// Creates a manager without starting any PTY, shell, or worker thread.
    ///
    /// # Errors
    /// Returns an error when a service limit is zero.
    pub fn new(options: TerminalOptions) -> Result<Self, TerminalError> {
        if options.max_sessions == 0
            || options.max_sessions > MAX_TERMINAL_SESSIONS
            || options.output_queue_chunks == 0
            || options.output_queue_chunks > MAX_TERMINAL_OUTPUT_QUEUE_CHUNKS
            || options.input_queue_chunks == 0
            || options.input_queue_chunks > MAX_TERMINAL_INPUT_QUEUE_CHUNKS
        {
            return Err(TerminalError::Invalid(format!(
                "terminal limits must be 1..={MAX_TERMINAL_SESSIONS} sessions, 1..={MAX_TERMINAL_OUTPUT_QUEUE_CHUNKS} output chunks, and 1..={MAX_TERMINAL_INPUT_QUEUE_CHUNKS} input chunks"
            )));
        }
        let profile = TerminalProfile::detected_default();
        let profiles = BTreeMap::from([(profile.id.clone(), profile)]);
        Ok(Self {
            options,
            next_id: 0,
            profiles,
            sessions: BTreeMap::new(),
            closed_events: VecDeque::new(),
        })
    }

    /// Registers a profile without starting its shell.
    ///
    /// # Errors
    /// Returns an error for invalid fields or a duplicate profile identifier.
    pub fn register_profile(&mut self, profile: TerminalProfile) -> Result<(), TerminalError> {
        validate_profile(&profile)?;
        if self.profiles.contains_key(&profile.id) {
            return Err(TerminalError::Invalid(format!(
                "terminal profile '{}' already exists",
                profile.id
            )));
        }
        self.profiles.insert(profile.id.clone(), profile);
        Ok(())
    }

    /// Lists configured profiles in identifier order.
    #[must_use]
    pub fn profiles(&self) -> Vec<&TerminalProfile> {
        self.profiles.values().collect()
    }

    /// Creates a Created session without allocating a PTY or starting a process.
    ///
    /// Hosts must show the resolved shell, argv, directory, environment key names, actor, and
    /// associated workspace before explicit approval and `start_session`.
    ///
    /// # Errors
    /// Returns an error for an unknown profile, invalid size/directory, exhausted ids, or session cap.
    pub fn create_session(
        &mut self,
        profile_id: &str,
        workspace_root: Option<&Path>,
        working_directory_override: Option<&Path>,
        rows: u16,
        columns: u16,
    ) -> Result<TerminalSessionInfo, TerminalError> {
        let profile = self
            .profiles
            .get(profile_id)
            .cloned()
            .ok_or(TerminalError::NotFound)?;
        self.create_session_with_profile(
            profile,
            workspace_root,
            working_directory_override,
            rows,
            columns,
        )
    }

    /// Creates a Created session from an explicit profile without registering it globally.
    ///
    /// Hosts must show the resolved command, argv, directory, environment key names, actor, and
    /// associated workspace before explicit approval and `start_session`.
    ///
    /// # Errors
    /// Returns an error for an invalid profile, invalid size/directory, exhausted ids, or session cap.
    pub fn create_session_with_profile(
        &mut self,
        profile: TerminalProfile,
        workspace_root: Option<&Path>,
        working_directory_override: Option<&Path>,
        rows: u16,
        columns: u16,
    ) -> Result<TerminalSessionInfo, TerminalError> {
        validate_profile(&profile)?;
        validate_size(rows, columns)?;
        if self.sessions.len() >= self.options.max_sessions {
            return Err(TerminalError::Invalid(
                "maximum terminal session count reached".into(),
            ));
        }
        let directory = resolve_working_directory(
            &profile.working_directory,
            workspace_root,
            working_directory_override,
        )?;
        self.next_id = self.next_id.checked_add(1).ok_or_else(|| {
            TerminalError::Invalid("terminal session identifiers exhausted".into())
        })?;
        let id = TerminalId(self.next_id);
        let session = Session {
            profile,
            working_directory: directory,
            state: TerminalState::Created,
            exit_code: None,
            error: None,
            rows,
            columns,
            child: None,
            master: None,
            process_group: None,
            output_receiver: None,
            pending_output: VecDeque::new(),
            input_sender: None,
            reader_thread: None,
            writer_thread: None,
            output_sequence: 0,
            state_notified: false,
            reader_finished: false,
        };
        let info = session_info(id, &session);
        self.sessions.insert(id, session);
        Ok(info)
    }

    /// Returns immutable shell details for a graphical approval prompt.
    ///
    /// # Errors
    /// Returns an error for an unknown session.
    pub fn approval_details(
        &self,
        id: TerminalId,
    ) -> Result<TerminalApprovalDetails, TerminalError> {
        let session = self.sessions.get(&id).ok_or(TerminalError::NotFound)?;
        Ok(TerminalApprovalDetails {
            profile_id: session.profile.id.clone(),
            profile_name: session.profile.name.clone(),
            executable: session.profile.executable.clone(),
            arguments: session.profile.arguments.clone(),
            working_directory: session.working_directory.clone(),
            environment_keys: session.profile.environment_keys(),
            fallback_executable: session.profile.fallback_executable.clone(),
        })
    }

    /// Starts a reserved session after explicit host authorization.
    ///
    /// The detected default shell retries /bin/sh or cmd.exe after startup failure. Custom
    /// profiles do not silently change shells. Arguments are passed directly as argv.
    ///
    /// # Errors
    /// Returns an error for an unknown/non-Created session or a startup failure. Failure leaves
    /// the stored session in Failed so the host can display and close it.
    pub fn start_session(&mut self, id: TerminalId) -> Result<TerminalSessionInfo, TerminalError> {
        let options = self.options;
        let session = self.sessions.get_mut(&id).ok_or(TerminalError::NotFound)?;
        if session.state != TerminalState::Created {
            return Err(TerminalError::InvalidState(session.state.clone()));
        }
        session.state = TerminalState::Starting;
        if let Err(error) = start_pty(session, options) {
            session.state = TerminalState::Failed;
            session.error = Some(error.to_string());
            return Err(error);
        }
        session.state = TerminalState::Running;
        Ok(session_info(id, session))
    }

    /// Returns lifecycle, exit code, and process-group state.
    ///
    /// # Errors
    /// Returns an error when the session is missing or its child handle fails.
    pub fn session(&mut self, id: TerminalId) -> Result<TerminalSessionInfo, TerminalError> {
        let session = self.sessions.get_mut(&id).ok_or(TerminalError::NotFound)?;
        poll_session(session)?;
        Ok(session_info(id, session))
    }

    /// Lists sessions in identifier order.
    ///
    /// # Errors
    /// Returns an error when a child status query fails.
    pub fn sessions(&mut self) -> Result<Vec<TerminalSessionInfo>, TerminalError> {
        let ids = self.sessions.keys().copied().collect::<Vec<_>>();
        ids.into_iter().map(|id| self.session(id)).collect()
    }

    /// Queues binary input without blocking the host request loop.
    ///
    /// # Errors
    /// Returns an error for an unknown/non-running session, an oversized chunk, or a full queue.
    pub fn write(&self, id: TerminalId, bytes: &[u8]) -> Result<(), TerminalError> {
        if bytes.is_empty() || bytes.len() > MAX_TERMINAL_INPUT_BYTES {
            return Err(TerminalError::Invalid(format!(
                "terminal input must be 1..={MAX_TERMINAL_INPUT_BYTES} bytes"
            )));
        }
        let session = self.sessions.get(&id).ok_or(TerminalError::NotFound)?;
        if session.state != TerminalState::Running {
            return Err(TerminalError::InvalidState(session.state.clone()));
        }
        session
            .input_sender
            .as_ref()
            .ok_or_else(|| TerminalError::Io("terminal input worker is unavailable".into()))?
            .try_send(bytes.to_vec())
            .map_err(|error| match error {
                TrySendError::Full(_) => TerminalError::Backpressure,
                TrySendError::Disconnected(_) => {
                    TerminalError::Io("terminal input worker stopped".into())
                }
            })
    }

    /// Resizes the PTY and records its dimensions.
    ///
    /// # Errors
    /// Returns an error for invalid dimensions, an unknown/non-running session, or PTY failure.
    pub fn resize(&mut self, id: TerminalId, rows: u16, columns: u16) -> Result<(), TerminalError> {
        validate_size(rows, columns)?;
        let session = self.sessions.get_mut(&id).ok_or(TerminalError::NotFound)?;
        if session.state != TerminalState::Running {
            return Err(TerminalError::InvalidState(session.state.clone()));
        }
        session
            .master
            .as_ref()
            .ok_or_else(|| TerminalError::Io("terminal PTY is unavailable".into()))?
            .resize(PtySize {
                rows,
                cols: columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| TerminalError::Io(error.to_string()))?;
        session.rows = rows;
        session.columns = columns;
        Ok(())
    }

    /// Drains ordered events. If `include_output` is false, bytes stay in bounded reader queues.
    #[must_use]
    pub fn drain_events(&mut self, include_output: bool) -> Vec<TerminalEvent> {
        let mut events = self
            .closed_events
            .drain(..)
            .map(|id| TerminalEvent::Closed { id })
            .collect::<Vec<_>>();
        let ids = self.sessions.keys().copied().collect::<Vec<_>>();
        let mut delivered_bytes = 0;
        for id in ids {
            let Some(session) = self.sessions.get_mut(&id) else {
                continue;
            };
            let _ = poll_session(session);
            if !include_output {
                continue;
            }
            while delivered_bytes < MAX_TERMINAL_OUTPUT_BYTES_PER_DRAIN {
                if !session.pending_output.is_empty() {
                    let count = (MAX_TERMINAL_OUTPUT_BYTES_PER_DRAIN - delivered_bytes)
                        .min(session.pending_output.len());
                    let bytes = session.pending_output.drain(..count).collect::<Vec<_>>();
                    delivered_bytes += bytes.len();
                    session.output_sequence = session.output_sequence.saturating_add(1);
                    events.push(TerminalEvent::Output {
                        id,
                        sequence: session.output_sequence,
                        bytes,
                    });
                    continue;
                }
                let Some(receiver) = session.output_receiver.as_ref() else {
                    break;
                };
                match receiver.try_recv() {
                    Ok(ReaderMessage::Data(bytes)) => {
                        let count = (MAX_TERMINAL_OUTPUT_BYTES_PER_DRAIN - delivered_bytes)
                            .min(bytes.len());
                        session
                            .pending_output
                            .extend(bytes[count..].iter().copied());
                        let bytes = bytes[..count].to_vec();
                        delivered_bytes += bytes.len();
                        session.output_sequence = session.output_sequence.saturating_add(1);
                        events.push(TerminalEvent::Output {
                            id,
                            sequence: session.output_sequence,
                            bytes,
                        });
                    }
                    Ok(ReaderMessage::End) => session.reader_finished = true,
                    Ok(ReaderMessage::Error(message)) => {
                        session.error = Some(message);
                        session.reader_finished = true;
                        if !session.state.is_finished() {
                            session.state = TerminalState::Failed;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        session.reader_finished = true;
                        break;
                    }
                }
                if session.reader_finished {
                    break;
                }
            }
            if session.state.is_finished() && session.reader_finished && !session.state_notified {
                session.state_notified = true;
                events.push(TerminalEvent::StateChanged {
                    id,
                    state: session.state.clone(),
                    exit_code: session.exit_code,
                    process_group_active: process_group_active(session),
                    error: session.error.clone(),
                });
            }
        }
        events
    }

    /// Closes a session. A live process group requires explicit termination confirmation.
    ///
    /// Unix sends SIGTERM then SIGKILL after a bounded grace period. Windows terminates the
    /// `ConPTY` child only; descendants may outlive it.
    ///
    /// # Errors
    /// Returns `ActiveSession` if a live process exists without confirmation, or a PTY cleanup error.
    pub fn close_session(
        &mut self,
        id: TerminalId,
        terminate_active: bool,
    ) -> Result<(), TerminalError> {
        let session = self.sessions.get_mut(&id).ok_or(TerminalError::NotFound)?;
        poll_session(session)?;
        let active = session.state == TerminalState::Running || process_group_active(session);
        if active && !terminate_active {
            return Err(TerminalError::ActiveSession);
        }
        if active {
            terminate_session(session)?;
        }
        release_workers(session)?;
        self.sessions.remove(&id);
        self.closed_events.push_back(id);
        Ok(())
    }

    /// Terminates and releases all sessions when the host shuts down.
    pub fn shutdown(&mut self) {
        let ids = self.sessions.keys().copied().collect::<Vec<_>>();
        for id in ids {
            let _ = self.close_session(id, true);
        }
    }
}

impl Drop for TerminalManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Redacted shell details that a host should present before starting a session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalApprovalDetails {
    /// Profile identifier.
    pub profile_id: String,
    /// Profile display name.
    pub profile_name: String,
    /// Shell executable path.
    pub executable: PathBuf,
    /// Direct argv vector.
    pub arguments: Vec<String>,
    /// Initial working directory.
    pub working_directory: PathBuf,
    /// Environment override names only.
    pub environment_keys: Vec<String>,
    /// Fallback executable, if any.
    pub fallback_executable: Option<PathBuf>,
}

fn validate_profile(profile: &TerminalProfile) -> Result<(), TerminalError> {
    if profile.id.trim().is_empty()
        || profile.id.len() > 128
        || profile.id.chars().any(char::is_control)
        || profile.name.trim().is_empty()
        || profile.name.len() > 128
        || profile.name.chars().any(char::is_control)
        || profile.executable.as_os_str().is_empty()
        || profile.arguments.len() > 128
        || profile
            .arguments
            .iter()
            .any(|argument| argument.contains('\0'))
    {
        return Err(TerminalError::Invalid("invalid terminal profile".into()));
    }
    for (key, value) in &profile.environment {
        if key.is_empty() || key.contains('=') || key.contains('\0') || value.contains('\0') {
            return Err(TerminalError::Invalid(
                "invalid terminal environment override".into(),
            ));
        }
    }
    Ok(())
}

fn validate_size(rows: u16, columns: u16) -> Result<(), TerminalError> {
    if rows == 0 || columns == 0 || rows > MAX_DIMENSION || columns > MAX_DIMENSION {
        return Err(TerminalError::Invalid(format!(
            "terminal dimensions must be between 1 and {MAX_DIMENSION}"
        )));
    }
    Ok(())
}

fn resolve_working_directory(
    behavior: &TerminalWorkingDirectory,
    workspace_root: Option<&Path>,
    override_path: Option<&Path>,
) -> Result<PathBuf, TerminalError> {
    let requested = if let Some(path) = override_path {
        path.to_path_buf()
    } else {
        match behavior {
            TerminalWorkingDirectory::Workspace => {
                workspace_root.map_or_else(|| home_directory().to_path_buf(), Path::to_path_buf)
            }
            TerminalWorkingDirectory::Home => home_directory().to_path_buf(),
            TerminalWorkingDirectory::Fixed(path) => path.clone(),
        }
    };
    let canonical = requested
        .canonicalize()
        .map_err(|error| TerminalError::Io(format!("cannot open terminal directory: {error}")))?;
    if !canonical.is_dir() {
        return Err(TerminalError::Invalid(
            "terminal working directory is not a directory".into(),
        ));
    }
    Ok(canonical)
}

fn home_directory() -> &'static Path {
    static HOME: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map_or_else(
                || std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                PathBuf::from,
            )
    })
}

fn build_command(profile: &TerminalProfile, executable: &Path, cwd: &Path) -> CommandBuilder {
    let mut command = CommandBuilder::new(executable);
    command.args(&profile.arguments);
    command.cwd(cwd);
    for (key, value) in &profile.environment {
        command.env(key, value);
    }
    command
}

fn start_pty(session: &mut Session, options: TerminalOptions) -> Result<(), TerminalError> {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: session.rows,
            cols: session.columns,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| TerminalError::Io(error.to_string()))?;
    let process_group = pair.master.process_group_leader();
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| TerminalError::Io(error.to_string()))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| TerminalError::Io(error.to_string()))?;
    let mut child = match pair.slave.spawn_command(build_command(
        &session.profile,
        &session.profile.executable,
        &session.working_directory,
    )) {
        Ok(child) => child,
        Err(primary_error) => {
            let Some(fallback) = session.profile.fallback_executable.as_ref() else {
                return Err(TerminalError::Io(primary_error.to_string()));
            };
            pair.slave.spawn_command(build_command(&session.profile, fallback, &session.working_directory))
                .map_err(|fallback_error| TerminalError::Io(format!(
                    "configured shell failed ({primary_error}); fallback failed ({fallback_error})"
                )))?
        }
    };
    let (output_sender, output_receiver) = mpsc::sync_channel(options.output_queue_chunks);
    let reader_thread = thread::Builder::new()
        .name("koren-terminal-reader".into())
        .spawn(move || read_pty(reader, &output_sender))
        .map_err(|error| {
            terminate_spawned_child(child.as_mut(), process_group);
            TerminalError::Io(format!("cannot start terminal reader: {error}"))
        })?;
    let (input_sender, input_receiver) = mpsc::sync_channel(options.input_queue_chunks);
    let writer_thread = match thread::Builder::new()
        .name("koren-terminal-writer".into())
        .spawn(move || write_pty(writer, &input_receiver))
    {
        Ok(worker) => worker,
        Err(error) => {
            let mut child = child;
            terminate_spawned_child(child.as_mut(), process_group);
            drop(pair.master);
            drop(output_receiver);
            let _ = reader_thread.join();
            return Err(TerminalError::Io(format!(
                "cannot start terminal writer: {error}"
            )));
        }
    };
    session.process_group = process_group;
    session.child = Some(child);
    session.master = Some(pair.master);
    session.output_receiver = Some(output_receiver);
    session.input_sender = Some(input_sender);
    session.reader_thread = Some(reader_thread);
    session.writer_thread = Some(writer_thread);
    Ok(())
}

fn read_pty(mut reader: Box<dyn Read + Send>, sender: &SyncSender<ReaderMessage>) {
    let mut buffer = vec![0; MAX_TERMINAL_OUTPUT_CHUNK_BYTES];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => {
                let _ = sender.send(ReaderMessage::End);
                return;
            }
            Ok(length) => {
                if sender
                    .send(ReaderMessage::Data(buffer[..length].to_vec()))
                    .is_err()
                {
                    return;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => {
                let _ = sender.send(ReaderMessage::Error(error.to_string()));
                return;
            }
        }
    }
}

fn write_pty(mut writer: Box<dyn Write + Send>, receiver: &Receiver<Vec<u8>>) {
    while let Ok(bytes) = receiver.recv() {
        if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
            return;
        }
    }
}

fn poll_session(session: &mut Session) -> Result<(), TerminalError> {
    if let Some(child) = session.child.as_mut() {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| TerminalError::Io(error.to_string()))?
        {
            session.exit_code = Some(status.exit_code());
            session.state = TerminalState::Exited;
        }
    }
    Ok(())
}

fn terminate_spawned_child(child: &mut dyn Child, process_group: Option<i32>) {
    #[cfg(unix)]
    if let Some(group) = process_group {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(group),
            Some(nix::sys::signal::Signal::SIGKILL),
        );
    }
    let _ = child.kill();
    let deadline = Instant::now() + SHUTDOWN_GRACE;
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn session_info(id: TerminalId, session: &Session) -> TerminalSessionInfo {
    TerminalSessionInfo {
        id,
        profile_id: session.profile.id.clone(),
        working_directory: session.working_directory.clone(),
        state: session.state.clone(),
        exit_code: session.exit_code,
        error: session.error.clone(),
        process_group_active: process_group_active(session),
        rows: session.rows,
        columns: session.columns,
    }
}

fn process_group_active(session: &Session) -> bool {
    #[cfg(unix)]
    if let Some(group) = session.process_group {
        return match nix::sys::signal::killpg(nix::unistd::Pid::from_raw(group), None) {
            Err(nix::errno::Errno::ESRCH) => false,
            Ok(()) | Err(_) => true,
        };
    }
    session.state == TerminalState::Running
}

fn terminate_session(session: &mut Session) -> Result<(), TerminalError> {
    #[cfg(unix)]
    if let Some(group) = session.process_group {
        let pid = nix::unistd::Pid::from_raw(group);
        if let Err(error) = nix::sys::signal::killpg(pid, Some(nix::sys::signal::Signal::SIGTERM)) {
            if error != nix::errno::Errno::ESRCH {
                return Err(TerminalError::Io(error.to_string()));
            }
        }
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        while Instant::now() < deadline && process_group_active(session) {
            thread::sleep(Duration::from_millis(10));
        }
        if process_group_active(session) {
            let _ = nix::sys::signal::killpg(pid, Some(nix::sys::signal::Signal::SIGKILL));
        }
    } else if let Some(child) = session.child.as_mut() {
        let _ = child.kill();
    }
    #[cfg(not(unix))]
    if let Some(child) = session.child.as_mut() {
        let _ = child.kill();
    }

    let deadline = Instant::now() + SHUTDOWN_GRACE;
    loop {
        let finished = match session.child.as_mut() {
            Some(child) => child
                .try_wait()
                .map_err(|error| TerminalError::Io(error.to_string()))?
                .is_some(),
            None => true,
        };
        if finished || Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    session.state = TerminalState::Terminated;
    Ok(())
}

fn release_workers(session: &mut Session) -> Result<(), TerminalError> {
    session.input_sender.take();
    session.master.take();
    session.output_receiver.take();
    for worker in [session.writer_thread.take(), session.reader_thread.take()]
        .into_iter()
        .flatten()
    {
        let deadline = Instant::now() + WORKER_JOIN_GRACE;
        while !worker.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        if worker.is_finished() {
            worker
                .join()
                .map_err(|_| TerminalError::Io("terminal I/O worker panicked".into()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;

    fn fixture_profile(mode: &str) -> TerminalProfile {
        TerminalProfile {
            id: format!("fixture-{mode}"),
            name: "Controlled fixture".into(),
            executable: std::env::current_exe().unwrap(),
            arguments: vec![
                "--exact".into(),
                "tests::pty_child_fixture".into(),
                "--nocapture".into(),
            ],
            working_directory: TerminalWorkingDirectory::Home,
            environment: BTreeMap::from([("KOREN_TERMINAL_FIXTURE_MODE".into(), mode.into())]),
            fallback_executable: None,
        }
    }

    fn manager_with(mode: &str) -> TerminalManager {
        let mut manager = TerminalManager::default();
        manager.register_profile(fixture_profile(mode)).unwrap();
        manager
    }

    fn create_start(manager: &mut TerminalManager, profile: &str) -> TerminalId {
        let created = manager.create_session(profile, None, None, 24, 80).unwrap();
        assert_eq!(created.state, TerminalState::Created);
        assert_eq!(
            manager.start_session(created.id).unwrap().state,
            TerminalState::Running
        );
        created.id
    }

    fn drain_output(manager: &mut TerminalManager, id: TerminalId, timeout: Duration) -> Vec<u8> {
        let deadline = Instant::now() + timeout;
        let mut output = Vec::new();
        while Instant::now() < deadline {
            for event in manager.drain_events(true) {
                if let TerminalEvent::Output {
                    id: output_id,
                    bytes,
                    ..
                } = event
                {
                    if output_id == id {
                        output.extend(bytes);
                    }
                }
            }
            if String::from_utf8_lossy(&output).contains("echo:") {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        output
    }

    #[test]
    fn pty_child_fixture() {
        let Ok(mode) = std::env::var("KOREN_TERMINAL_FIXTURE_MODE") else {
            return;
        };
        if mode == "echo" {
            let mut line = String::new();
            std::io::BufReader::new(std::io::stdin())
                .read_line(&mut line)
                .unwrap();
            print!("echo:{line}");
            std::io::stdout().flush().unwrap();
        } else if mode == "stream" {
            for index in 0..64 {
                println!("ordered-{index}");
            }
            std::io::stdout().flush().unwrap();
        } else if mode == "large" {
            let chunk = vec![b'x'; MAX_TERMINAL_OUTPUT_CHUNK_BYTES];
            print!("KOREN_LARGE_BEGIN");
            for _ in 0..128 {
                std::io::stdout().write_all(&chunk).unwrap();
            }
            std::io::stdout().flush().unwrap();
        } else if mode == "wait" {
            let mut buffer = [0; 1];
            let _ = std::io::stdin().read(&mut buffer);
        }
    }

    #[test]
    fn sessions_deliver_input_output_exit_and_have_unique_ids() {
        let mut manager = manager_with("echo");
        let first = create_start(&mut manager, "fixture-echo");
        let second = create_start(&mut manager, "fixture-echo");
        assert_ne!(first, second);
        manager.write(first, b"hello-pty\n").unwrap();
        let output = drain_output(&mut manager, first, Duration::from_secs(3));
        assert!(String::from_utf8_lossy(&output).contains("echo:hello-pty"));
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline
            && manager.session(first).unwrap().state == TerminalState::Running
        {
            let _ = manager.drain_events(true);
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(manager.session(first).unwrap().state, TerminalState::Exited);
        manager.close_session(second, true).unwrap();
        manager.close_session(first, false).unwrap();
    }

    #[test]
    fn interactive_fixture_records_pty_start_and_round_trip_timing() {
        let mut manager = manager_with("echo");
        let reserve_start = Instant::now();
        let created = manager
            .create_session("fixture-echo", None, None, 24, 80)
            .unwrap();
        let reserve_time = reserve_start.elapsed();
        let pty_start = Instant::now();
        manager.start_session(created.id).unwrap();
        let pty_and_child_start_time = pty_start.elapsed();
        let input_start = Instant::now();
        manager.write(created.id, b"latency-probe\n").unwrap();
        let output = drain_output(&mut manager, created.id, Duration::from_secs(3));
        let round_trip_time = input_start.elapsed();
        assert!(String::from_utf8_lossy(&output).contains("echo:latency-probe"));
        eprintln!(
            "terminal fixture timing: reserve={reserve_time:?}, pty_and_child_start={pty_and_child_start_time:?}, input_to_echo={round_trip_time:?}"
        );
        manager.close_session(created.id, true).unwrap();
    }

    #[test]
    fn output_order_resize_and_invalid_session_operations() {
        let mut manager = manager_with("stream");
        let id = create_start(&mut manager, "fixture-stream");
        manager.resize(id, 40, 100).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut output = Vec::new();
        while Instant::now() < deadline && !String::from_utf8_lossy(&output).contains("ordered-63")
        {
            for event in manager.drain_events(true) {
                if let TerminalEvent::Output {
                    id: output_id,
                    bytes,
                    ..
                } = event
                {
                    if output_id == id {
                        output.extend(bytes);
                    }
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
        let text = String::from_utf8_lossy(&output);
        assert!(text.find("ordered-0").unwrap() < text.find("ordered-63").unwrap());
        assert_eq!(manager.session(id).unwrap().columns, 100);
        assert_eq!(
            manager.write(TerminalId(999), b"x"),
            Err(TerminalError::NotFound)
        );
        manager.close_session(id, true).unwrap();
        assert!(matches!(manager.session(id), Err(TerminalError::NotFound)));
    }

    #[test]
    fn large_output_is_bounded_per_drain_and_delivered_without_loss() {
        let mut manager = manager_with("large");
        let transfer_start = Instant::now();
        let id = create_start(&mut manager, "fixture-large");
        let deadline = Instant::now() + Duration::from_secs(6);
        let mut drains = 0usize;
        let marker = b"KOREN_LARGE_BEGIN";
        let expected_size = marker.len() + 2 * 1024 * 1024;
        let mut output = Vec::new();
        while Instant::now() < deadline {
            let events = manager.drain_events(true);
            let bytes = events
                .iter()
                .map(|event| match event {
                    TerminalEvent::Output {
                        id: event_id,
                        bytes,
                        ..
                    } if *event_id == id => bytes.len(),
                    _ => 0,
                })
                .sum::<usize>();
            assert!(bytes <= MAX_TERMINAL_OUTPUT_BYTES_PER_DRAIN);
            for event in events {
                if let TerminalEvent::Output {
                    id: event_id,
                    bytes,
                    ..
                } = event
                {
                    if event_id == id {
                        output.extend(bytes);
                    }
                }
            }
            drains += 1;
            let complete = output
                .windows(marker.len())
                .position(|window| window == marker)
                .is_some_and(|offset| output.len() >= offset + expected_size);
            if complete {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        let marker_offset = output
            .windows(marker.len())
            .position(|window| window == marker)
            .unwrap();
        assert!(output.len() >= marker_offset + expected_size);
        assert!(
            output[marker_offset + marker.len()..marker_offset + expected_size]
                .iter()
                .all(|byte| *byte == b'x')
        );
        assert!(drains > 1);
        eprintln!(
            "terminal controlled-child output: 2 MiB payload delivered in {:?}",
            transfer_start.elapsed()
        );
        manager.close_session(id, true).unwrap();
    }

    #[test]
    fn startup_failure_and_profile_environment_redaction() {
        let mut manager = TerminalManager::default();
        let profile = TerminalProfile {
            id: "missing".into(),
            name: "Missing shell".into(),
            executable: PathBuf::from("/this/command/does/not/exist"),
            arguments: vec![],
            working_directory: TerminalWorkingDirectory::Home,
            environment: BTreeMap::from([("TOKEN".into(), "secret-value".into())]),
            fallback_executable: None,
        };
        assert!(!format!("{profile:?}").contains("secret-value"));
        manager.register_profile(profile).unwrap();
        let created = manager
            .create_session("missing", None, None, 24, 80)
            .unwrap();
        assert!(manager.start_session(created.id).is_err());
        assert_eq!(
            manager.session(created.id).unwrap().state,
            TerminalState::Failed
        );
        manager.close_session(created.id, false).unwrap();
    }

    #[test]
    fn active_close_requires_confirmation_and_shutdown_cleans_up() {
        let mut manager = manager_with("wait");
        let id = create_start(&mut manager, "fixture-wait");
        assert_eq!(
            manager.close_session(id, false),
            Err(TerminalError::ActiveSession)
        );
        manager.shutdown();
        assert_eq!(
            manager.sessions().unwrap(),
            Vec::<TerminalSessionInfo>::new()
        );
    }

    #[test]
    fn shell_detection_and_workspace_directory_are_lazy() {
        let profile = TerminalProfile::detected_default();
        assert_eq!(profile.id, "default");
        let mut manager = TerminalManager::default();
        let root = std::env::temp_dir();
        let info = manager
            .create_session("default", Some(&root), None, 24, 80)
            .unwrap();
        assert_eq!(info.state, TerminalState::Created);
        assert_eq!(info.working_directory, root.canonicalize().unwrap());
        manager.close_session(info.id, false).unwrap();
    }

    #[test]
    fn manager_rejects_unbounded_resource_configuration() {
        assert!(matches!(
            TerminalManager::new(TerminalOptions {
                max_sessions: MAX_TERMINAL_SESSIONS + 1,
                ..TerminalOptions::default()
            }),
            Err(TerminalError::Invalid(_))
        ));
        assert!(matches!(
            TerminalManager::new(TerminalOptions {
                output_queue_chunks: MAX_TERMINAL_OUTPUT_QUEUE_CHUNKS + 1,
                ..TerminalOptions::default()
            }),
            Err(TerminalError::Invalid(_))
        ));
        assert!(matches!(
            TerminalManager::new(TerminalOptions {
                input_queue_chunks: 0,
                ..TerminalOptions::default()
            }),
            Err(TerminalError::Invalid(_))
        ));
    }
}
