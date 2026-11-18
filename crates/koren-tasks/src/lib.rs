//! Bounded, asynchronous direct-process task execution for IDE hosts.

use koren_core::PlatformEvent;
use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    io::Read,
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

const CHILD_STATUS_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// Manager-local task definition identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TaskId(u64);
impl TaskId {
    /// Reconstructs an identifier received from a trusted host protocol.
    #[must_use]
    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }
    /// Numeric value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}
/// Manager-local process run identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TaskRunId(u64);
impl TaskRunId {
    /// Reconstructs an identifier received from a trusted host protocol.
    #[must_use]
    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }
    /// Numeric value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}
/// Supported generic task category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskCategory {
    /// Build output.
    Build,
    /// Static validation.
    Check,
    /// Test execution.
    Test,
    /// Cleanup.
    Clean,
    /// Launch application.
    Run,
    /// Provider-defined operation.
    Custom,
}
/// Caller class used by the host authorization policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskCaller {
    /// Interactive human user.
    InteractiveUser,
    /// Installed extension identifier.
    Extension(String),
    /// Future coding agent identity.
    CodingAgent(String),
}
/// Host authorization capability. The runner never accepts a protocol-supplied approval flag.
pub trait TaskAuthorizer: Send + Sync {
    /// Approves this caller to start this task definition.
    ///
    /// # Errors
    ///
    /// Returns a denial when this caller lacks approval.
    fn authorize(&self, caller: &TaskCaller, task: &TaskDefinition) -> Result<(), TaskError>;
}
/// Task definition, independent from execution permission.
#[derive(Clone, Eq, PartialEq)]
pub struct TaskDefinition {
    /// Stable within this manager/provider scope.
    pub key: String,
    /// Human-readable name.
    pub display_name: String,
    /// Category.
    pub category: TaskCategory,
    /// Executable path or name resolved by the operating system.
    pub executable: PathBuf,
    /// Direct argv parameters; no shell expansion is performed.
    pub arguments: Vec<String>,
    /// Process working directory.
    pub working_directory: PathBuf,
    /// Environment overrides (never included in snapshots/events).
    pub environment: BTreeMap<String, String>,
    /// Optional wall-clock timeout.
    pub timeout: Option<Duration>,
    /// Optional logical project association.
    pub project_id: Option<u64>,
}
impl fmt::Debug for TaskDefinition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskDefinition")
            .field("key", &self.key)
            .field("display_name", &self.display_name)
            .field("category", &self.category)
            .field("executable", &self.executable)
            .field("arguments", &self.arguments)
            .field("working_directory", &self.working_directory)
            .field(
                "environment_keys",
                &self.environment.keys().collect::<Vec<_>>(),
            )
            .field("timeout", &self.timeout)
            .field("project_id", &self.project_id)
            .finish()
    }
}
/// Task service limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskOptions {
    /// Maximum simultaneously active processes.
    pub max_concurrent: usize,
    /// Maximum retained bytes per output stream.
    pub max_output_bytes_per_stream: usize,
    /// Maximum queued lifecycle/output events.
    pub max_events: usize,
}
impl Default for TaskOptions {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            max_output_bytes_per_stream: 1024 * 1024,
            max_events: 4096,
        }
    }
}
/// Task state. Spawn failures differ from non-zero process exits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskState {
    /// Not yet started (reserved for future scheduling).
    Pending,
    /// Process is active.
    Running,
    /// Exit code zero.
    Succeeded {
        /// Process exit code.
        exit_code: i32,
    },
    /// Non-zero or unavailable exit code.
    Failed {
        /// Non-zero exit code, absent when the OS did not provide one.
        exit_code: Option<i32>,
    },
    /// Explicit cancellation.
    Cancelled,
    /// Timeout elapsed.
    TimedOut,
    /// Operating-system spawn failed.
    SpawnFailed {
        /// Operating-system spawn error.
        message: String,
    },
}
impl TaskState {
    /// Whether no further state transition can occur.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        !matches!(self, Self::Pending | Self::Running)
    }
}
/// Captured output and terminal state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskSnapshot {
    /// Run identity.
    pub run_id: TaskRunId,
    /// Task definition identity.
    pub task_id: TaskId,
    /// Current state.
    pub state: TaskState,
    /// Decoded lossy UTF-8 standard output.
    pub stdout: String,
    /// Decoded lossy UTF-8 standard error.
    pub stderr: String,
    /// At least one output stream exceeded its retention limit.
    pub output_truncated: bool,
}
/// Execution failure.
#[derive(Debug)]
pub enum TaskError {
    /// Task or run identifier does not exist.
    NotFound,
    /// Service limit or configuration violation.
    Invalid(String),
    /// Host denied the caller.
    Unauthorized(String),
    /// Filesystem or process error.
    Io(String),
    /// An approval request no longer exists.
    ApprovalNotFound,
    /// An approval request expired before it was decided.
    ApprovalExpired,
    /// An approval request was invalidated by project, workspace, or session closure.
    ApprovalInvalidated,
}
impl fmt::Display for TaskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("task or run was not found"),
            Self::Invalid(s) | Self::Io(s) => f.write_str(s),
            Self::Unauthorized(s) => write!(f, "task execution denied: {s}"),
            Self::ApprovalNotFound => f.write_str("execution approval request was not found"),
            Self::ApprovalExpired => f.write_str("execution approval request expired"),
            Self::ApprovalInvalidated => {
                f.write_str("execution approval request was invalidated by closure")
            }
        }
    }
}
impl std::error::Error for TaskError {}

#[derive(Debug)]
struct RunData {
    task_id: TaskId,
    state: TaskState,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    truncated: bool,
}
#[derive(Debug)]
struct Run {
    data: Mutex<RunData>,
    child: Arc<Mutex<Option<Child>>>,
    cancel: Arc<AtomicBool>,
}
/// Bounded task-event payload. Ordering is FIFO within this service; oldest events are dropped at capacity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskEvent {
    /// Run identity.
    pub run_id: TaskRunId,
    /// Event kind.
    pub kind: TaskEventKind,
}
/// Task notification kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskEventKind {
    /// Process launched.
    Started,
    /// Output appended.
    OutputAvailable,
    /// Process finished.
    Finished,
    /// Cancelled.
    Cancelled,
    /// Spawn or execution failure.
    Failed,
}

/// Owns task definitions, concurrent runs, and bounded FIFO events.
#[derive(Debug)]
pub struct TaskManager {
    options: TaskOptions,
    next_task: u64,
    next_run: u64,
    definitions: BTreeMap<TaskId, TaskDefinition>,
    runs: BTreeMap<TaskRunId, Arc<Run>>,
    events: Arc<Mutex<VecDeque<TaskEvent>>>,
    core_events: Arc<Mutex<VecDeque<PlatformEvent>>>,
}

/// One-time identifier for a host-mediated task execution request.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExecutionRequestId(u64);
impl ExecutionRequestId {
    /// Reconstructs an identifier received from the host protocol.
    #[must_use]
    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }
    /// Numeric manager-local value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Exact task definition and actor captured when execution was requested.
///
/// Environment values are retained in this in-memory snapshot to bind approval to the
/// actual execution. Its debug representation redacts those values.
#[derive(Clone)]
pub struct TaskExecutionSnapshot {
    /// Task definition identifier.
    pub task_id: TaskId,
    /// Immutable task definition captured for this request.
    pub definition: TaskDefinition,
    /// Host-supplied actor identity.
    pub caller: TaskCaller,
    /// Workspace owning this request; used to invalidate pending work on close.
    pub workspace_root: PathBuf,
}
impl fmt::Debug for TaskExecutionSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskExecutionSnapshot")
            .field("task_id", &self.task_id)
            .field("key", &self.definition.key)
            .field("executable", &self.definition.executable)
            .field("arguments", &self.definition.arguments)
            .field("working_directory", &self.definition.working_directory)
            .field(
                "environment_keys",
                &self.definition.environment.keys().collect::<Vec<_>>(),
            )
            .field("project_id", &self.definition.project_id)
            .field("caller", &self.caller)
            .field("workspace_root", &self.workspace_root)
            .finish()
    }
}

/// Policy outcome for a new execution request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskAuthorizationDecision {
    /// Execute immediately under the host's deliberately permissive policy.
    Allow,
    /// Reject without creating an approval request.
    Deny(String),
    /// Wait for a separate host approval decision.
    RequireApproval,
}

/// Host policy evaluated against the immutable execution request snapshot.
pub trait TaskExecutionPolicy: Send + Sync {
    /// Chooses immediate execution, denial, or explicit approval.
    fn decide(&self, request: &TaskExecutionSnapshot) -> TaskAuthorizationDecision;
}

/// Pending request exposed to a host for an approval prompt.
#[derive(Clone)]
pub struct TaskExecutionRequest {
    /// One-time request identifier.
    pub id: ExecutionRequestId,
    /// Immutable execution snapshot.
    pub snapshot: TaskExecutionSnapshot,
}
impl fmt::Debug for TaskExecutionRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskExecutionRequest")
            .field("id", &self.id)
            .field("snapshot", &self.snapshot)
            .finish()
    }
}

/// Result of applying the host's initial execution policy.
#[derive(Debug)]
pub enum TaskExecutionRequestOutcome {
    /// The host policy allowed the request and a run was started.
    Started(TaskRunId),
    /// The host must approve or deny this one-time request.
    ApprovalRequired(Box<TaskExecutionRequest>),
}

struct PendingExecution {
    request: TaskExecutionRequest,
    deadline: Instant,
}

/// Owns one-time approval requests and binds each approval to an immutable task snapshot.
///
/// Pending requests do not consume task concurrency slots. Hosts must explicitly decide each
/// request. Closing the associated project, workspace, or service invalidates it.
#[derive(Default)]
pub struct TaskApprovalManager {
    next_id: u64,
    pending: BTreeMap<ExecutionRequestId, PendingExecution>,
}
impl fmt::Debug for TaskApprovalManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskApprovalManager")
            .field("pending_count", &self.pending.len())
            .finish_non_exhaustive()
    }
}

impl TaskManager {
    /// Creates an empty task service.
    ///
    /// # Errors
    ///
    /// Returns an error if any resource limit is zero.
    pub fn new(options: TaskOptions) -> Result<Self, TaskError> {
        if options.max_concurrent == 0
            || options.max_output_bytes_per_stream == 0
            || options.max_events == 0
        {
            return Err(TaskError::Invalid(
                "task limits must be greater than zero".into(),
            ));
        }
        Ok(Self {
            options,
            next_task: 1,
            next_run: 1,
            definitions: BTreeMap::new(),
            runs: BTreeMap::new(),
            events: Arc::new(Mutex::new(VecDeque::new())),
            core_events: Arc::new(Mutex::new(VecDeque::new())),
        })
    }
    /// Registers a task definition without granting permission to execute it.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid or duplicate task key, or exhausted identifiers.
    pub fn register(&mut self, definition: TaskDefinition) -> Result<TaskId, TaskError> {
        if definition.key.trim().is_empty()
            || definition.display_name.trim().is_empty()
            || definition.executable.as_os_str().is_empty()
        {
            return Err(TaskError::Invalid(
                "task key, display name and executable must not be empty".into(),
            ));
        }
        if self
            .definitions
            .values()
            .any(|d| d.key == definition.key && d.project_id == definition.project_id)
        {
            return Err(TaskError::Invalid(
                "duplicate task key in project scope".into(),
            ));
        }
        let id = TaskId(self.next_task);
        self.next_task = self
            .next_task
            .checked_add(1)
            .ok_or_else(|| TaskError::Invalid("task id space exhausted".into()))?;
        self.definitions.insert(id, definition);
        Ok(id)
    }
    /// Lists task ids in deterministic registration order.
    #[must_use]
    pub fn list(&self) -> Vec<(TaskId, &TaskDefinition)> {
        self.definitions.iter().map(|(a, b)| (*a, b)).collect()
    }
    /// Removes a definition after its one-shot approval was consumed. Existing runs retain output.
    /// The caller must first invalidate or consume any pending approval for this task ID.
    pub fn unregister(&mut self, id: TaskId) -> Option<TaskDefinition> {
        self.definitions.remove(&id)
    }
    fn definition(&self, id: TaskId) -> Result<&TaskDefinition, TaskError> {
        self.definitions.get(&id).ok_or(TaskError::NotFound)
    }
    /// Starts an authorized task. Process creation happens synchronously; waiting and output collection use background threads.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown task, authorization denial, a concurrency limit,
    /// invalid service state, or exhausted identifiers.
    pub fn start(
        &mut self,
        id: TaskId,
        caller: &TaskCaller,
        authorizer: &dyn TaskAuthorizer,
    ) -> Result<TaskRunId, TaskError> {
        let definition = self.definitions.get(&id).ok_or(TaskError::NotFound)?;
        authorizer.authorize(caller, definition)?;
        let active = self
            .runs
            .values()
            .filter(|r| {
                r.data
                    .lock()
                    .is_ok_and(|d| matches!(d.state, TaskState::Pending | TaskState::Running))
            })
            .count();
        if active >= self.options.max_concurrent {
            return Err(TaskError::Invalid(
                "maximum concurrent task count reached".into(),
            ));
        }
        let run_id = TaskRunId(self.next_run);
        self.next_run = self
            .next_run
            .checked_add(1)
            .ok_or_else(|| TaskError::Invalid("run id space exhausted".into()))?;
        let child = match spawn_process(definition) {
            Ok(c) => c,
            Err(e) => {
                self.record_spawn_failure(id, run_id, e.to_string());
                return Ok(run_id);
            }
        };
        let started = Instant::now();
        let run = launch_run(
            id,
            run_id,
            child,
            definition.timeout,
            started,
            self.options,
            &self.events,
            &self.core_events,
        );
        self.runs.insert(run_id, run);
        Ok(run_id)
    }
    fn record_spawn_failure(&mut self, task_id: TaskId, run_id: TaskRunId, message: String) {
        let run = Arc::new(Run {
            data: Mutex::new(RunData {
                task_id,
                state: TaskState::SpawnFailed { message },
                stdout: vec![],
                stderr: vec![],
                truncated: false,
            }),
            child: Arc::new(Mutex::new(None)),
            cancel: Arc::new(AtomicBool::new(false)),
        });
        self.runs.insert(run_id, run);
        push_event(
            &self.events,
            self.options.max_events,
            TaskEvent {
                run_id,
                kind: TaskEventKind::Failed,
            },
        );
        push_core_event(
            &self.core_events,
            self.options.max_events,
            PlatformEvent::TaskFailed {
                run_id: run_id.get(),
            },
        );
    }
    /// Returns a consistent task output/status snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error if the run is unknown or its state lock was poisoned.
    pub fn status(&self, id: TaskRunId) -> Result<TaskSnapshot, TaskError> {
        let run = self.runs.get(&id).ok_or(TaskError::NotFound)?;
        let d = run
            .data
            .lock()
            .map_err(|_| TaskError::Io("task state lock poisoned".into()))?;
        Ok(snapshot(id, d.task_id, &d))
    }
    /// Returns retained output and status; output is capped per stream and continues draining after truncation.
    ///
    /// # Errors
    ///
    /// Returns the same errors as status.
    pub fn output(&self, id: TaskRunId) -> Result<TaskSnapshot, TaskError> {
        self.status(id)
    }
    /// Requests cancellation of a running direct child process.
    ///
    /// # Errors
    ///
    /// Returns an error if the run is unknown, state is poisoned, or the OS refuses cancellation.
    pub fn cancel(&self, id: TaskRunId) -> Result<(), TaskError> {
        let run = self.runs.get(&id).ok_or(TaskError::NotFound)?;
        let data = run
            .data
            .lock()
            .map_err(|_| TaskError::Io("task state lock poisoned".into()))?;
        if data.state.is_finished() {
            return Ok(());
        }
        run.cancel.store(true, Ordering::Release);
        if let Ok(mut child) = run.child.lock() {
            if let Some(child) = child.as_mut() {
                if let Err(error) = child.kill() {
                    if child.try_wait().ok().flatten().is_none() {
                        return Err(TaskError::Io(error.to_string()));
                    }
                    run.cancel.store(false, Ordering::Release);
                }
            }
        }
        Ok(())
    }
    /// Removes a completed run and its retained output.
    ///
    /// # Errors
    ///
    /// Returns an error if the run is unknown, still active, or its state lock is poisoned.
    pub fn cleanup(&mut self, id: TaskRunId) -> Result<(), TaskError> {
        let run = self.runs.get(&id).ok_or(TaskError::NotFound)?;
        if !run
            .data
            .lock()
            .map_err(|_| TaskError::Io("task state lock poisoned".into()))?
            .state
            .is_finished()
        {
            return Err(TaskError::Invalid("cannot clean up a running task".into()));
        }
        self.runs.remove(&id);
        Ok(())
    }
    /// Drains task events in service FIFO order.
    pub fn drain_events(&mut self) -> Vec<TaskEvent> {
        self.events
            .lock()
            .map(|mut e| e.drain(..).collect())
            .unwrap_or_default()
    }
    /// Drains corresponding shared platform lifecycle notifications.
    pub fn drain_platform_events(&mut self) -> Vec<PlatformEvent> {
        self.core_events
            .lock()
            .map(|mut e| e.drain(..).collect())
            .unwrap_or_default()
    }
    /// Cancels all current direct children. Descendant process groups are not managed portably.
    pub fn shutdown(&self) {
        for run in self.runs.values() {
            if let Ok(d) = run.data.lock() {
                if matches!(d.state, TaskState::Running | TaskState::Pending) {
                    run.cancel.store(true, Ordering::Release);
                    if let Ok(mut c) = run.child.lock() {
                        if let Some(c) = c.as_mut() {
                            let _ = c.kill();
                        }
                    }
                }
            }
        }
    }
}

const MAX_PENDING_APPROVALS: usize = 256;

impl TaskApprovalManager {
    /// Creates an empty approval service.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies a host policy to one task and stores an exact one-time approval request when needed.
    ///
    /// At most 256 unexpired requests are retained. Each request expires after `ttl` and is
    /// independent of task concurrency slots.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown task, denied request, invalid scope or lifetime,
    /// exhausted request IDs, or a full pending queue.
    pub fn request_execution(
        &mut self,
        tasks: &mut TaskManager,
        task_id: TaskId,
        caller: TaskCaller,
        workspace_root: PathBuf,
        policy: &dyn TaskExecutionPolicy,
        ttl: Duration,
    ) -> Result<TaskExecutionRequestOutcome, TaskError> {
        self.expire_requests();
        if ttl.is_zero() || workspace_root.as_os_str().is_empty() {
            return Err(TaskError::Invalid(
                "approval lifetime and workspace root must be non-empty".into(),
            ));
        }
        let snapshot = TaskExecutionSnapshot {
            task_id,
            definition: tasks.definition(task_id)?.clone(),
            caller,
            workspace_root,
        };
        match policy.decide(&snapshot) {
            TaskAuthorizationDecision::Allow => {
                let run_id = start_snapshot(tasks, &snapshot)?;
                Ok(TaskExecutionRequestOutcome::Started(run_id))
            }
            TaskAuthorizationDecision::Deny(reason) => Err(TaskError::Unauthorized(reason)),
            TaskAuthorizationDecision::RequireApproval => {
                if self.pending.len() >= MAX_PENDING_APPROVALS {
                    return Err(TaskError::Invalid(
                        "maximum pending execution approval count reached".into(),
                    ));
                }
                let id = ExecutionRequestId(
                    self.next_id
                        .checked_add(1)
                        .ok_or_else(|| TaskError::Invalid("approval id space exhausted".into()))?,
                );
                self.next_id = id.0;
                let deadline = Instant::now()
                    .checked_add(ttl)
                    .ok_or_else(|| TaskError::Invalid("approval lifetime is too large".into()))?;
                let request = TaskExecutionRequest { id, snapshot };
                let returned_request = request.clone();
                self.pending
                    .insert(id, PendingExecution { request, deadline });
                Ok(TaskExecutionRequestOutcome::ApprovalRequired(Box::new(
                    returned_request,
                )))
            }
        }
    }

    /// Lists unexpired requests in creation order, removing any that have expired.
    pub fn pending_requests(&mut self) -> Vec<&TaskExecutionRequest> {
        self.expire_requests();
        self.pending.values().map(|entry| &entry.request).collect()
    }

    /// Returns the remaining lifetime of one unexpired request.
    ///
    /// # Errors
    ///
    /// Returns an error when the request is missing or expired.
    pub fn remaining_lifetime(&self, id: ExecutionRequestId) -> Result<Duration, TaskError> {
        let pending = self.pending.get(&id).ok_or(TaskError::ApprovalNotFound)?;
        let remaining = pending.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            Err(TaskError::ApprovalExpired)
        } else {
            Ok(remaining)
        }
    }

    /// Approves and executes a pending request exactly once.
    ///
    /// Calling this method is the host's authorization decision. The task definition must still
    /// match the stored snapshot byte-for-byte, including environment overrides.
    ///
    /// # Errors
    ///
    /// Returns an error for missing, expired, or invalidated requests and task start failures.
    pub fn approve(
        &mut self,
        tasks: &mut TaskManager,
        id: ExecutionRequestId,
    ) -> Result<TaskRunId, TaskError> {
        let pending = self
            .pending
            .remove(&id)
            .ok_or(TaskError::ApprovalNotFound)?;
        if Instant::now() >= pending.deadline {
            return Err(TaskError::ApprovalExpired);
        }
        start_snapshot(tasks, &pending.request.snapshot)
    }

    /// Rejects and consumes a pending request.
    ///
    /// # Errors
    ///
    /// Returns an error if the request does not exist or has expired.
    pub fn deny(&mut self, id: ExecutionRequestId) -> Result<(), TaskError> {
        let pending = self
            .pending
            .remove(&id)
            .ok_or(TaskError::ApprovalNotFound)?;
        if Instant::now() >= pending.deadline {
            return Err(TaskError::ApprovalExpired);
        }
        Ok(())
    }

    /// Invalidates all pending requests associated with a project.
    pub fn invalidate_project(&mut self, project_id: u64) -> Vec<ExecutionRequestId> {
        self.invalidate_where(|request| request.snapshot.definition.project_id == Some(project_id))
    }

    /// Invalidates all pending requests associated with a workspace root.
    pub fn invalidate_workspace(
        &mut self,
        workspace_root: &std::path::Path,
    ) -> Vec<ExecutionRequestId> {
        self.invalidate_where(|request| request.snapshot.workspace_root == workspace_root)
    }

    /// Invalidates every pending request, for example when the owning session closes.
    pub fn shutdown(&mut self) -> Vec<ExecutionRequestId> {
        let ids = self.pending.keys().copied().collect();
        self.pending.clear();
        ids
    }

    /// Removes expired requests and returns their identifiers.
    pub fn expire_requests(&mut self) -> Vec<ExecutionRequestId> {
        let now = Instant::now();
        let ids = self
            .pending
            .iter()
            .filter(|(_, request)| now >= request.deadline)
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in &ids {
            self.pending.remove(id);
        }
        ids
    }

    fn invalidate_where(
        &mut self,
        predicate: impl Fn(&TaskExecutionRequest) -> bool,
    ) -> Vec<ExecutionRequestId> {
        let ids = self
            .pending
            .iter()
            .filter(|(_, pending)| predicate(&pending.request))
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in &ids {
            self.pending.remove(id);
        }
        ids
    }
}

struct SnapshotAuthorizer<'a>(&'a TaskDefinition);
impl TaskAuthorizer for SnapshotAuthorizer<'_> {
    fn authorize(&self, _caller: &TaskCaller, task: &TaskDefinition) -> Result<(), TaskError> {
        if task == self.0 {
            Ok(())
        } else {
            Err(TaskError::Unauthorized(
                "task definition changed after approval request".into(),
            ))
        }
    }
}

fn start_snapshot(
    tasks: &mut TaskManager,
    snapshot: &TaskExecutionSnapshot,
) -> Result<TaskRunId, TaskError> {
    let authorizer = SnapshotAuthorizer(&snapshot.definition);
    tasks.start(snapshot.task_id, &snapshot.caller, &authorizer)
}

struct OutputReader {
    run_id: TaskRunId,
    limit: usize,
    max_events: usize,
    stdout: bool,
    events: Arc<Mutex<VecDeque<TaskEvent>>>,
    core_events: Arc<Mutex<VecDeque<PlatformEvent>>>,
}
fn spawn_process(definition: &TaskDefinition) -> std::io::Result<Child> {
    let mut command = Command::new(&definition.executable);
    command
        .args(&definition.arguments)
        .current_dir(&definition.working_directory)
        .envs(&definition.environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.spawn()
}
// Groups pipe setup and monitor startup so infrastructure failures kill and reap the child.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn launch_run(
    task_id: TaskId,
    run_id: TaskRunId,
    mut child: Child,
    timeout: Option<Duration>,
    started: Instant,
    options: TaskOptions,
    events: &Arc<Mutex<VecDeque<TaskEvent>>>,
    core_events: &Arc<Mutex<VecDeque<PlatformEvent>>>,
) -> Arc<Run> {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let child = Arc::new(Mutex::new(Some(child)));
    let run = Arc::new(Run {
        data: Mutex::new(RunData {
            task_id,
            state: TaskState::Running,
            stdout: vec![],
            stderr: vec![],
            truncated: false,
        }),
        child: child.clone(),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    push_event(
        events,
        options.max_events,
        TaskEvent {
            run_id,
            kind: TaskEventKind::Started,
        },
    );
    push_core_event(
        core_events,
        options.max_events,
        PlatformEvent::TaskStarted {
            run_id: run_id.get(),
        },
    );
    let readers_done = match (stdout, stderr) {
        (Some(stdout), Some(stderr)) => start_readers(
            stdout,
            stderr,
            run.clone(),
            run_id,
            options,
            events,
            core_events,
        ),
        _ => Err(std::io::Error::other("child output pipe unavailable")),
    };
    if let Ok(readers_done) = readers_done {
        let supervise_run = run.clone();
        let supervise_child = child.clone();
        let supervise_events = events.clone();
        let supervise_core = core_events.clone();
        if thread::Builder::new()
            .name(format!("koren-task-{}", run_id.get()))
            .spawn(move || {
                supervise(
                    run_id,
                    supervise_run,
                    supervise_child,
                    timeout,
                    started,
                    options.max_events,
                    supervise_events,
                    supervise_core,
                    readers_done,
                );
            })
            .is_err()
        {
            kill_and_reap(&child);
            complete_run(
                &run,
                run_id,
                TaskState::Failed { exit_code: None },
                options.max_events,
                events,
                core_events,
            );
        }
    } else {
        kill_and_reap(&child);
        complete_run(
            &run,
            run_id,
            TaskState::Failed { exit_code: None },
            options.max_events,
            events,
            core_events,
        );
    }
    run
}
fn start_readers<R: Read + Send + 'static, S: Read + Send + 'static>(
    stdout: R,
    stderr: S,
    run: Arc<Run>,
    run_id: TaskRunId,
    options: TaskOptions,
    events: &Arc<Mutex<VecDeque<TaskEvent>>>,
    core_events: &Arc<Mutex<VecDeque<PlatformEvent>>>,
) -> std::io::Result<Receiver<()>> {
    let (done_tx, done_rx) = mpsc::channel();
    let common = |is_stdout| OutputReader {
        run_id,
        limit: options.max_output_bytes_per_stream,
        max_events: options.max_events,
        stdout: is_stdout,
        events: events.clone(),
        core_events: core_events.clone(),
    };
    let out_run = run.clone();
    start_reader(stdout, out_run, common(true), done_tx.clone())?;
    start_reader(stderr, run, common(false), done_tx)?;
    Ok(done_rx)
}
fn start_reader<R: Read + Send + 'static>(
    reader: R,
    run: Arc<Run>,
    config: OutputReader,
    done: Sender<()>,
) -> std::io::Result<()> {
    let name = format!(
        "koren-task-{}/{}",
        config.run_id.get(),
        if config.stdout { "stdout" } else { "stderr" }
    );
    thread::Builder::new()
        .name(name)
        .spawn(move || {
            read_output(reader, &run, &config);
            let _ = done.send(());
        })
        .map(|_| ())
}
// These are independent OS resources and event sinks moved to a dedicated monitor thread;
// wrapping them in a one-use runtime object would add an otherwise unnecessary abstraction.
#[allow(clippy::needless_pass_by_value, clippy::too_many_arguments)]
fn supervise(
    run_id: TaskRunId,
    run: Arc<Run>,
    child: Arc<Mutex<Option<Child>>>,
    timeout: Option<Duration>,
    started: Instant,
    max_events: usize,
    events: Arc<Mutex<VecDeque<TaskEvent>>>,
    core_events: Arc<Mutex<VecDeque<PlatformEvent>>>,
    readers_done: Receiver<()>,
) {
    let mut timed_out = false;
    loop {
        if run.cancel.load(Ordering::Acquire) {
            if let Ok(mut c) = child.lock() {
                if let Some(c) = c.as_mut() {
                    let _ = c.kill();
                }
            }
        }
        let waited = child
            .lock()
            .ok()
            .and_then(|mut c| c.as_mut().map(Child::try_wait));
        match waited {
            Some(Ok(Some(status))) => {
                wait_for_readers(&readers_done);
                let state = if timed_out {
                    TaskState::TimedOut
                } else if run.cancel.load(Ordering::Acquire) {
                    TaskState::Cancelled
                } else {
                    status_state(status)
                };
                complete_run(&run, run_id, state, max_events, &events, &core_events);
                return;
            }
            Some(Err(_)) => {
                kill_and_reap(&child);
                wait_for_readers(&readers_done);
                complete_run(
                    &run,
                    run_id,
                    TaskState::Failed { exit_code: None },
                    max_events,
                    &events,
                    &core_events,
                );
                return;
            }
            _ => {}
        }
        if !timed_out && timeout.is_some_and(|limit| started.elapsed() >= limit) {
            timed_out = true;
            kill_and_reap(&child);
        }
        thread::park_timeout(CHILD_STATUS_POLL_INTERVAL);
    }
}
fn wait_for_readers(readers: &Receiver<()>) {
    let deadline = Instant::now() + Duration::from_millis(200);
    for _ in 0..2 {
        if readers
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_err()
        {
            break;
        }
    }
}
fn kill_and_reap(child: &Arc<Mutex<Option<Child>>>) {
    if let Ok(mut child) = child.lock() {
        if let Some(child) = child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
fn complete_run(
    run: &Run,
    run_id: TaskRunId,
    state: TaskState,
    max_events: usize,
    events: &Arc<Mutex<VecDeque<TaskEvent>>>,
    core_events: &Arc<Mutex<VecDeque<PlatformEvent>>>,
) {
    let kind = match &state {
        TaskState::Succeeded { .. } => TaskEventKind::Finished,
        TaskState::Cancelled => TaskEventKind::Cancelled,
        _ => TaskEventKind::Failed,
    };
    run.data
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .state = state;
    push_event(events, max_events, TaskEvent { run_id, kind });
    let event = match kind {
        TaskEventKind::Finished => PlatformEvent::TaskFinished {
            run_id: run_id.get(),
        },
        TaskEventKind::Cancelled => PlatformEvent::TaskCancelled {
            run_id: run_id.get(),
        },
        _ => PlatformEvent::TaskFailed {
            run_id: run_id.get(),
        },
    };
    push_core_event(core_events, max_events, event);
}
impl Drop for TaskManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}
fn snapshot(id: TaskRunId, task_id: TaskId, d: &RunData) -> TaskSnapshot {
    TaskSnapshot {
        run_id: id,
        task_id,
        state: d.state.clone(),
        stdout: String::from_utf8_lossy(&d.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&d.stderr).into_owned(),
        output_truncated: d.truncated,
    }
}
fn status_state(s: ExitStatus) -> TaskState {
    if s.success() {
        TaskState::Succeeded {
            exit_code: s.code().unwrap_or(0),
        }
    } else {
        TaskState::Failed {
            exit_code: s.code(),
        }
    }
}
fn push_event(events: &Arc<Mutex<VecDeque<TaskEvent>>>, limit: usize, event: TaskEvent) {
    if let Ok(mut e) = events.lock() {
        if e.len() == limit {
            e.pop_front();
        }
        e.push_back(event);
    }
}
fn push_core_event(
    events: &Arc<Mutex<VecDeque<PlatformEvent>>>,
    limit: usize,
    event: PlatformEvent,
) {
    if let Ok(mut e) = events.lock() {
        if e.len() == limit {
            e.pop_front();
        }
        e.push_back(event);
    }
}
fn read_output<R: Read>(mut reader: R, run: &Run, config: &OutputReader) {
    let mut buffer = [0u8; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut data = run
                    .data
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if data.state.is_finished() {
                    break;
                }
                {
                    let target = if config.stdout {
                        &mut data.stdout
                    } else {
                        &mut data.stderr
                    };
                    let remain = config.limit.saturating_sub(target.len());
                    let take = remain.min(n);
                    target.extend_from_slice(&buffer[..take]);
                    if take < n {
                        data.truncated = true;
                    }
                }
                push_event(
                    &config.events,
                    config.max_events,
                    TaskEvent {
                        run_id: config.run_id,
                        kind: TaskEventKind::OutputAvailable,
                    },
                );
                push_core_event(
                    &config.core_events,
                    config.max_events,
                    PlatformEvent::TaskOutputAvailable {
                        run_id: config.run_id.get(),
                    },
                );
                drop(data);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, io::Write};
    struct Allow;
    impl TaskAuthorizer for Allow {
        fn authorize(&self, _: &TaskCaller, _: &TaskDefinition) -> Result<(), TaskError> {
            Ok(())
        }
    }
    struct Deny;
    impl TaskAuthorizer for Deny {
        fn authorize(&self, _: &TaskCaller, _: &TaskDefinition) -> Result<(), TaskError> {
            Err(TaskError::Unauthorized("policy denied".into()))
        }
    }
    struct RequireApproval;
    impl TaskExecutionPolicy for RequireApproval {
        fn decide(&self, _: &TaskExecutionSnapshot) -> TaskAuthorizationDecision {
            TaskAuthorizationDecision::RequireApproval
        }
    }
    struct AllowImmediately;
    impl TaskExecutionPolicy for AllowImmediately {
        fn decide(&self, _: &TaskExecutionSnapshot) -> TaskAuthorizationDecision {
            TaskAuthorizationDecision::Allow
        }
    }
    struct DenyExecution;
    impl TaskExecutionPolicy for DenyExecution {
        fn decide(&self, _: &TaskExecutionSnapshot) -> TaskAuthorizationDecision {
            TaskAuthorizationDecision::Deny("host denied".into())
        }
    }
    fn def(key: &str) -> TaskDefinition {
        TaskDefinition {
            key: key.into(),
            display_name: key.into(),
            category: TaskCategory::Custom,
            executable: env::current_exe().unwrap(),
            arguments: vec![
                "--exact".into(),
                "tests::fixture".into(),
                "--nocapture".into(),
            ],
            working_directory: env::current_dir().unwrap(),
            environment: BTreeMap::from([("KOREN_TASK_FIXTURE".into(), key.into())]),
            timeout: Some(Duration::from_secs(5)),
            project_id: None,
        }
    }
    #[test]
    fn fixture() {
        if let Ok(v) = env::var("KOREN_TASK_FIXTURE") {
            if v == "hold" {
                println!("ready");
                std::io::stdout().flush().unwrap();
                loop {
                    thread::park_timeout(Duration::from_secs(1));
                }
            }
            if v == "large" {
                println!("{}", "x".repeat(2048));
            } else {
                println!("fixture:{v}");
            }
            eprintln!("error:{v}");
            if v == "fail" {
                std::process::exit(7);
            }
        }
    }
    #[test]
    fn unregister_after_start_preserves_one_shot_output() {
        let mut tasks = TaskManager::new(TaskOptions::default()).unwrap();
        let id = tasks.register(def("one-shot")).unwrap();
        let run = tasks
            .start(id, &TaskCaller::InteractiveUser, &Allow)
            .unwrap();
        assert!(tasks.unregister(id).is_some());
        assert_eq!(tasks.list().len(), 0);
        assert!(matches!(
            wait_finished(&tasks, run).state,
            TaskState::Succeeded { .. }
        ));
        assert!(
            tasks
                .output(run)
                .unwrap()
                .stdout
                .contains("fixture:one-shot")
        );
    }

    #[test]
    fn execution_authorization_output_status_and_failure_are_explicit() {
        let mut m = TaskManager::new(TaskOptions::default()).unwrap();
        let id = m.register(def("ok")).unwrap();
        assert!(matches!(
            m.start(id, &TaskCaller::CodingAgent("agent".into()), &Deny),
            Err(TaskError::Unauthorized(_))
        ));
        let r = m.start(id, &TaskCaller::InteractiveUser, &Allow).unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while !m.status(r).unwrap().state.is_finished() {
            assert!(Instant::now() < end);
            thread::yield_now();
        }
        let s = m.output(r).unwrap();
        assert!(matches!(s.state, TaskState::Succeeded { .. }));
        assert!(s.stdout.contains("fixture:ok"));
        assert!(s.stderr.contains("error:ok"));
        let failed = m.register(def("fail")).unwrap();
        let failed_run = m
            .start(failed, &TaskCaller::InteractiveUser, &Allow)
            .unwrap();
        assert_eq!(
            wait_finished(&m, failed_run).state,
            TaskState::Failed { exit_code: Some(7) }
        );
        let missing = m
            .register(TaskDefinition {
                executable: "missing-koren-task-command".into(),
                ..def("missing")
            })
            .unwrap();
        let x = m
            .start(missing, &TaskCaller::InteractiveUser, &Allow)
            .unwrap();
        assert!(matches!(
            m.status(x).unwrap().state,
            TaskState::SpawnFailed { .. }
        ));
    }
    #[test]
    fn bounded_output_and_cleanup() {
        let mut m = TaskManager::new(TaskOptions {
            max_concurrent: 2,
            max_output_bytes_per_stream: 16,
            max_events: 64,
        })
        .unwrap();
        let id = m.register(def("large")).unwrap();
        let r = m.start(id, &TaskCaller::InteractiveUser, &Allow).unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while !m.status(r).unwrap().state.is_finished() {
            assert!(Instant::now() < end);
            thread::yield_now();
        }
        assert!(m.output(r).unwrap().stdout.len() <= 16);
        assert!(m.output(r).unwrap().output_truncated);
        let events = m.drain_events();
        assert_eq!(events.first().map(|e| e.kind), Some(TaskEventKind::Started));
        assert_eq!(events.last().map(|e| e.kind), Some(TaskEventKind::Finished));
        let finished = events
            .iter()
            .position(|event| event.kind == TaskEventKind::Finished)
            .unwrap();
        assert!(
            events[..finished]
                .iter()
                .any(|event| event.kind == TaskEventKind::OutputAvailable)
        );
        assert!(
            !events[finished + 1..]
                .iter()
                .any(|event| event.kind == TaskEventKind::OutputAvailable)
        );
        m.cleanup(r).unwrap();
        assert!(m.status(r).is_err());
    }
    fn wait_finished(manager: &TaskManager, id: TaskRunId) -> TaskSnapshot {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            let s = manager.status(id).unwrap();
            if s.state.is_finished() {
                return s;
            }
            assert!(Instant::now() < end, "task did not finish before deadline");
            thread::yield_now();
        }
    }
    fn wait_output(manager: &TaskManager, id: TaskRunId, needle: &str) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if manager.output(id).unwrap().stdout.contains(needle) {
                return;
            }
            assert!(
                Instant::now() < end,
                "task did not emit synchronization output"
            );
            thread::yield_now();
        }
    }
    #[test]
    fn cancellation_timeout_concurrency_and_invalid_directory_are_bounded() {
        let mut m = TaskManager::new(TaskOptions {
            max_concurrent: 1,
            ..TaskOptions::default()
        })
        .unwrap();
        let hold = m.register(def("hold")).unwrap();
        let run = m.start(hold, &TaskCaller::InteractiveUser, &Allow).unwrap();
        wait_output(&m, run, "ready");
        let ordinary = m.register(def("ok")).unwrap();
        assert!(matches!(
            m.start(ordinary, &TaskCaller::InteractiveUser, &Allow),
            Err(TaskError::Invalid(_))
        ));
        m.cancel(run).unwrap();
        assert_eq!(wait_finished(&m, run).state, TaskState::Cancelled);
        let mut timeout_def = def("hold-timeout");
        timeout_def
            .environment
            .insert("KOREN_TASK_FIXTURE".into(), "hold".into());
        timeout_def.timeout = Some(Duration::from_millis(80));
        let timeout = m.register(timeout_def).unwrap();
        let timed = m
            .start(timeout, &TaskCaller::InteractiveUser, &Allow)
            .unwrap();
        assert_eq!(wait_finished(&m, timed).state, TaskState::TimedOut);
        let invalid = m
            .register(TaskDefinition {
                working_directory: env::temp_dir().join("missing-koren-cwd"),
                ..def("invalid-cwd")
            })
            .unwrap();
        let bad = m
            .start(invalid, &TaskCaller::InteractiveUser, &Allow)
            .unwrap();
        assert!(matches!(
            m.status(bad).unwrap().state,
            TaskState::SpawnFailed { .. }
        ));
        let shutdown_task = m
            .register(TaskDefinition {
                key: "shutdown".into(),
                ..def("hold")
            })
            .unwrap();
        let shutdown_run = m
            .start(shutdown_task, &TaskCaller::InteractiveUser, &Allow)
            .unwrap();
        wait_output(&m, shutdown_run, "ready");
        m.shutdown();
        assert_eq!(wait_finished(&m, shutdown_run).state, TaskState::Cancelled);
    }
    #[test]
    fn independent_children_run_concurrently() {
        let mut manager = TaskManager::new(TaskOptions {
            max_concurrent: 2,
            ..TaskOptions::default()
        })
        .unwrap();
        let mut first_definition = def("hold-a");
        first_definition
            .environment
            .insert("KOREN_TASK_FIXTURE".into(), "hold".into());
        let first = manager.register(first_definition).unwrap();
        let mut second_definition = def("hold-b");
        second_definition
            .environment
            .insert("KOREN_TASK_FIXTURE".into(), "hold".into());
        let second = manager.register(second_definition).unwrap();
        let first_run = manager
            .start(first, &TaskCaller::InteractiveUser, &Allow)
            .unwrap();
        let second_run = manager
            .start(second, &TaskCaller::InteractiveUser, &Allow)
            .unwrap();
        wait_output(&manager, first_run, "ready");
        wait_output(&manager, second_run, "ready");
        assert_eq!(manager.status(first_run).unwrap().state, TaskState::Running);
        assert_eq!(
            manager.status(second_run).unwrap().state,
            TaskState::Running
        );
        manager.cancel(first_run).unwrap();
        manager.cancel(second_run).unwrap();
        assert_eq!(
            wait_finished(&manager, first_run).state,
            TaskState::Cancelled
        );
        assert_eq!(
            wait_finished(&manager, second_run).state,
            TaskState::Cancelled
        );
    }

    #[test]
    fn approval_is_one_time_and_bound_to_the_stored_command_snapshot() {
        let mut tasks = TaskManager::new(TaskOptions::default()).unwrap();
        let mut definition = def("approval-snapshot");
        definition
            .environment
            .insert("TOKEN".into(), "SECRET_VALUE".into());
        assert!(format!("{definition:?}").contains("environment_keys"));
        assert!(!format!("{definition:?}").contains("SECRET_VALUE"));
        let task_id = tasks.register(definition).unwrap();
        let mut approvals = TaskApprovalManager::new();
        let outcome = approvals
            .request_execution(
                &mut tasks,
                task_id,
                TaskCaller::InteractiveUser,
                env::current_dir().unwrap(),
                &RequireApproval,
                Duration::from_secs(5),
            )
            .unwrap();
        let TaskExecutionRequestOutcome::ApprovalRequired(mut request) = outcome else {
            panic!("default host policy must require approval");
        };
        assert_eq!(request.id.get(), 1);
        assert_eq!(approvals.pending_requests().len(), 1);
        assert!(format!("{request:?}").contains("environment_keys"));
        assert!(!format!("{request:?}").contains("SECRET_VALUE"));

        // The public response is a copy; mutating it cannot change the stored command.
        request.snapshot.definition.arguments.clear();
        let run_id = approvals.approve(&mut tasks, request.id).unwrap();
        assert!(matches!(
            wait_finished(&tasks, run_id).state,
            TaskState::Succeeded { .. }
        ));
        assert!(
            tasks
                .output(run_id)
                .unwrap()
                .stdout
                .contains("fixture:approval-snapshot")
        );
        assert!(matches!(
            approvals.approve(&mut tasks, request.id),
            Err(TaskError::ApprovalNotFound)
        ));

        let mut long_definition = def("approved-cancellation");
        long_definition
            .environment
            .insert("KOREN_TASK_FIXTURE".into(), "hold".into());
        let long_task = tasks.register(long_definition).unwrap();
        let long_request = approvals
            .request_execution(
                &mut tasks,
                long_task,
                TaskCaller::InteractiveUser,
                env::current_dir().unwrap(),
                &RequireApproval,
                Duration::from_secs(5),
            )
            .unwrap();
        let TaskExecutionRequestOutcome::ApprovalRequired(long_request) = long_request else {
            panic!("expected cancellation task approval");
        };
        let long_run = approvals.approve(&mut tasks, long_request.id).unwrap();
        wait_output(&tasks, long_run, "ready");
        tasks.cancel(long_run).unwrap();
        assert_eq!(wait_finished(&tasks, long_run).state, TaskState::Cancelled);
    }

    #[test]
    fn authorization_decisions_do_not_start_denied_or_pending_tasks() {
        let mut tasks = TaskManager::new(TaskOptions {
            max_concurrent: 1,
            ..TaskOptions::default()
        })
        .unwrap();
        let task_id = tasks.register(def("denied-before-spawn")).unwrap();
        let mut approvals = TaskApprovalManager::new();
        assert!(matches!(
            approvals.request_execution(
                &mut tasks,
                task_id,
                TaskCaller::InteractiveUser,
                env::current_dir().unwrap(),
                &DenyExecution,
                Duration::from_secs(5),
            ),
            Err(TaskError::Unauthorized(_))
        ));
        assert!(matches!(
            approvals.request_execution(
                &mut tasks,
                task_id,
                TaskCaller::InteractiveUser,
                env::current_dir().unwrap(),
                &RequireApproval,
                Duration::from_secs(5),
            ),
            Ok(TaskExecutionRequestOutcome::ApprovalRequired(_))
        ));
        assert!(tasks.status(TaskRunId::from_raw(1)).is_err());
        assert_eq!(approvals.pending_requests().len(), 1);

        let run_id = match approvals
            .request_execution(
                &mut tasks,
                task_id,
                TaskCaller::InteractiveUser,
                env::current_dir().unwrap(),
                &AllowImmediately,
                Duration::from_secs(5),
            )
            .unwrap()
        {
            TaskExecutionRequestOutcome::Started(id) => id,
            TaskExecutionRequestOutcome::ApprovalRequired(_) => panic!("expected immediate allow"),
        };
        assert!(matches!(
            wait_finished(&tasks, run_id).state,
            TaskState::Succeeded { .. }
        ));
    }

    #[test]
    fn approvals_are_distinct_expire_and_invalidate_on_scope_close() {
        let mut tasks = TaskManager::new(TaskOptions::default()).unwrap();
        let mut project_task = def("project-approval");
        project_task.project_id = Some(42);
        let task_id = tasks.register(project_task).unwrap();
        let mut approvals = TaskApprovalManager::new();
        let workspace = env::current_dir().unwrap();
        let make_request = |approvals: &mut TaskApprovalManager, tasks: &mut TaskManager, ttl| {
            approvals
                .request_execution(
                    tasks,
                    task_id,
                    TaskCaller::CodingAgent("agent-a".into()),
                    workspace.clone(),
                    &RequireApproval,
                    ttl,
                )
                .unwrap()
        };
        let first = make_request(&mut approvals, &mut tasks, Duration::from_secs(5));
        let second = make_request(&mut approvals, &mut tasks, Duration::from_secs(5));
        let (
            TaskExecutionRequestOutcome::ApprovalRequired(first),
            TaskExecutionRequestOutcome::ApprovalRequired(second),
        ) = (first, second)
        else {
            panic!("expected two pending approvals");
        };
        assert_ne!(first.id, second.id);
        assert_eq!(approvals.invalidate_project(42), vec![first.id, second.id]);
        assert!(matches!(
            approvals.approve(&mut tasks, first.id),
            Err(TaskError::ApprovalNotFound)
        ));

        let workspace_request = make_request(&mut approvals, &mut tasks, Duration::from_secs(5));
        let TaskExecutionRequestOutcome::ApprovalRequired(workspace_request) = workspace_request
        else {
            panic!("expected pending approval");
        };
        assert_eq!(
            approvals.invalidate_workspace(&workspace),
            vec![workspace_request.id]
        );

        let expiring = make_request(&mut approvals, &mut tasks, Duration::from_millis(1));
        let TaskExecutionRequestOutcome::ApprovalRequired(expiring) = expiring else {
            panic!("expected pending approval");
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while approvals.expire_requests().is_empty() {
            assert!(Instant::now() < deadline, "approval did not expire");
            thread::yield_now();
        }
        assert!(matches!(
            approvals.approve(&mut tasks, expiring.id),
            Err(TaskError::ApprovalNotFound)
        ));

        let session = make_request(&mut approvals, &mut tasks, Duration::from_secs(5));
        let TaskExecutionRequestOutcome::ApprovalRequired(session) = session else {
            panic!("expected pending approval");
        };
        assert_eq!(approvals.shutdown(), vec![session.id]);
        assert!(approvals.pending_requests().is_empty());
    }
}
