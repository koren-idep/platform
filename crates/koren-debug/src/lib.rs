//! Language-neutral, bounded Debug Adapter Protocol client services.
//!
//! The crate launches only locally configured stdio adapters, after a host authorizes both the
//! adapter and the requested debug target. It does not implement a language debugger or provide
//! a process sandbox.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    collections::BTreeMap,
    fmt, fs,
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// Maximum UTF-8 DAP payload accepted from or sent to an adapter.
pub const MAX_DAP_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
/// Maximum bytes in a DAP header block.
pub const MAX_DAP_HEADER_BYTES: usize = 8 * 1024;
/// Maximum independent debug sessions in one manager.
pub const MAX_DEBUG_SESSIONS: usize = 16;
/// Maximum source breakpoints retained per source file.
pub const MAX_SOURCE_BREAKPOINTS: usize = 2_000;
/// Maximum frame or variable page requested at once.
pub const MAX_DEBUG_PAGE_SIZE: usize = 200;
/// Maximum accepted adapter profile arguments.
pub const MAX_ADAPTER_ARGUMENTS: usize = 128;
/// Maximum accepted adapter-specific JSON settings.
pub const MAX_CONFIGURATION_BYTES: usize = 256 * 1024;
const COMMAND_QUEUE_CAPACITY: usize = 128;
const ADAPTER_QUEUE_CAPACITY: usize = 128;
const EVENT_QUEUE_CAPACITY: usize = 2_048;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(not(test))]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(test)]
const REQUEST_TIMEOUT: Duration = Duration::from_millis(200);
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
const SHUTDOWN_WAIT: Duration = Duration::from_millis(500);
const MAX_DIAGNOSTIC_BYTES: usize = 8 * 1024;

/// DAP request type used to start or attach to a debug target.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebugRequestType {
    /// Ask the adapter to launch a new debuggee.
    Launch,
    /// Attach to an existing process; disconnect must leave it running.
    Attach,
}

/// A local stdio adapter definition contributed by a host or extension.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebugAdapterDefinition {
    /// Stable provider identity.
    pub id: String,
    /// User-facing adapter name.
    pub name: String,
    /// Adapter executable. It is passed directly to the process API, never shell parsed.
    pub command: String,
    /// Direct adapter argument vector.
    #[serde(default)]
    pub arguments: Vec<String>,
    /// Adapter working directory.
    pub working_directory: PathBuf,
    /// Explicit environment overrides.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

impl fmt::Debug for DebugAdapterDefinition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DebugAdapterDefinition")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("command", &self.command)
            .field("arguments", &self.arguments)
            .field("working_directory", &self.working_directory)
            .field(
                "environment_keys",
                &self.environment.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Language-neutral launch or attach configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebugConfiguration {
    /// Stable configuration identity.
    pub id: String,
    /// User-facing configuration name.
    pub name: String,
    /// Registered adapter provider identity.
    pub adapter_id: String,
    /// Whether this configuration launches or attaches to a debuggee.
    pub request: DebugRequestType,
    /// Program path for launch, or optional executable metadata for attach.
    pub program: Option<PathBuf>,
    /// Process identifier required for attach.
    pub process_id: Option<u32>,
    /// Program arguments; these are DAP target arguments, not adapter arguments.
    #[serde(default)]
    pub arguments: Vec<String>,
    /// Target working directory.
    pub working_directory: PathBuf,
    /// Explicit target environment overrides.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    /// Adapter-specific DAP launch or attach settings.
    #[serde(default)]
    pub adapter_settings: Map<String, Value>,
    /// Stop at the first instruction when the adapter supports it.
    #[serde(default)]
    pub stop_on_entry: bool,
}

impl DebugConfiguration {
    /// Validates common fields and rejects adapter settings that could override host-reviewed fields.
    ///
    /// # Errors
    /// Returns an error when a common field is invalid, an adapter setting is reserved, or the
    /// adapter-specific settings exceed the configured bound.
    pub fn validate(&self) -> Result<(), DebugError> {
        if self.id.trim().is_empty()
            || self.id.len() > 128
            || self.name.trim().is_empty()
            || self.name.len() > 128
            || self.adapter_id.trim().is_empty()
            || self.working_directory.as_os_str().is_empty()
            || self.arguments.len() > MAX_ADAPTER_ARGUMENTS
            || self
                .arguments
                .iter()
                .any(|argument| argument.contains('\0'))
        {
            return Err(DebugError::InvalidConfiguration(
                "invalid debug configuration".into(),
            ));
        }
        match self.request {
            DebugRequestType::Launch
                if self
                    .program
                    .as_ref()
                    .is_none_or(|path| path.as_os_str().is_empty()) =>
            {
                return Err(DebugError::InvalidConfiguration(
                    "launch configurations require a program path".into(),
                ));
            }
            DebugRequestType::Attach if self.process_id.is_none_or(|pid| pid == 0) => {
                return Err(DebugError::InvalidConfiguration(
                    "attach configurations require a nonzero process id".into(),
                ));
            }
            _ => {}
        }
        for key in &[
            "request",
            "program",
            "args",
            "cwd",
            "env",
            "processId",
            "noDebug",
            "stopOnEntry",
        ] {
            if self.adapter_settings.contains_key(*key) {
                return Err(DebugError::InvalidConfiguration(format!(
                    "adapter settings may not override host-reviewed field '{key}'"
                )));
            }
        }
        let encoded = serde_json::to_vec(&self.adapter_settings)
            .map_err(|error| DebugError::InvalidConfiguration(error.to_string()))?;
        if encoded.len() > MAX_CONFIGURATION_BYTES {
            return Err(DebugError::InvalidConfiguration(
                "adapter-specific settings exceed the size limit".into(),
            ));
        }
        for (key, value) in &self.environment {
            if key.is_empty() || key.contains('=') || key.contains('\0') || value.contains('\0') {
                return Err(DebugError::InvalidConfiguration(
                    "invalid target environment override".into(),
                ));
            }
        }
        Ok(())
    }
}

/// Public details a host presents for separate adapter and target approvals.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugAuthorizationDetails {
    /// Adapter process definition.
    pub adapter: DebugAdapterDefinition,
    /// Exact launch or attach configuration.
    pub configuration: DebugConfiguration,
}

/// Host authorization boundary for adapter process startup and debug target access.
pub trait DebugAuthorizer {
    /// Approves starting this exact local adapter process.
    fn authorize_adapter(&mut self, adapter: &DebugAdapterDefinition) -> bool;
    /// Approves this exact launch or attach configuration separately from the adapter process.
    fn authorize_target(&mut self, configuration: &DebugConfiguration) -> bool;
}

/// Debug session lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebugSessionState {
    /// Reserved before an adapter is started.
    Created,
    /// Adapter process and DAP initialization are active.
    Initializing,
    /// Adapter is waiting for configuration to finish.
    Configuring,
    /// Debuggee is executing.
    Running,
    /// Debuggee is stopped and its variable references are valid.
    Paused,
    /// Host is disconnecting or terminating the session.
    Terminating,
    /// Adapter session has ended.
    Terminated,
    /// Adapter or protocol failed.
    Failed,
}

/// Adapter capabilities relevant to the shared debugger UI.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DebugCapabilities {
    /// Optional initialization and session-management requests.
    pub requests: DebugRequestCapabilities,
    /// Program lifecycle features.
    pub execution: DebugExecutionCapabilities,
    /// Bounded inspection features.
    pub inspection: DebugInspectionCapabilities,
}

/// Optional DAP requests advertised by an adapter.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DebugRequestCapabilities {
    /// Adapter accepts the optional `configurationDone` request.
    pub configuration_done: bool,
    /// Adapter supports restart requests.
    pub restart: bool,
    /// Adapter supports cancel requests.
    pub cancel: bool,
}

/// DAP debuggee execution capabilities.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DebugExecutionCapabilities {
    /// Adapter supports graceful `terminate` before disconnecting a launched target.
    pub terminate: bool,
}

/// DAP inspection capabilities.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DebugInspectionCapabilities {
    /// Adapter supports paging variable collections.
    pub variable_paging: bool,
    /// Adapter can evaluate expressions. Evaluation is not exposed by this phase.
    pub evaluate_for_hovers: bool,
}

/// Current immutable public view of a debug session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DebugSessionInfo {
    /// Manager-local session identity.
    pub session_id: u64,
    /// Adapter provider identity.
    pub adapter_id: String,
    /// Selected configuration identity.
    pub configuration_id: String,
    /// Launch or attach request type.
    pub request: DebugRequestType,
    /// Session lifecycle state.
    pub state: DebugSessionState,
    /// Negotiated adapter capabilities.
    pub capabilities: DebugCapabilities,
    /// Current stop-generation token. It changes on resume, stop, and termination.
    pub stop_generation: u64,
    /// Thread that most recently stopped, if known.
    pub stopped_thread_id: Option<i64>,
    /// Last exit code reported by the adapter.
    pub exit_code: Option<i32>,
    /// Safe-to-display adapter or protocol error.
    pub error: Option<String>,
}

/// Editor breakpoint request using zero-based source coordinates.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreakpointSpec {
    /// Zero-based source line.
    pub line: u32,
    /// Optional zero-based source column.
    pub column: Option<u32>,
    /// Optional adapter condition.
    pub condition: Option<String>,
    /// Optional hit-count condition.
    pub hit_condition: Option<String>,
    /// Optional logpoint message.
    pub log_message: Option<String>,
    /// Whether the breakpoint is enabled.
    pub enabled: bool,
}

/// Stored breakpoint and the adapter's latest verification result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceBreakpoint {
    /// Manager-local breakpoint identity.
    pub id: u64,
    /// Zero-based requested source line.
    pub line: u32,
    /// Optional zero-based requested source column.
    pub column: Option<u32>,
    /// Optional adapter condition.
    pub condition: Option<String>,
    /// Optional hit-count condition.
    pub hit_condition: Option<String>,
    /// Optional logpoint message.
    pub log_message: Option<String>,
    /// Whether the breakpoint is enabled.
    pub enabled: bool,
    /// Whether the adapter verified the breakpoint.
    pub verified: Option<bool>,
    /// Adapter-resolved zero-based line, if supplied.
    pub actual_line: Option<u32>,
    /// Adapter message or unverified reason.
    pub message: Option<String>,
}

/// DAP variable reference paired with the stop generation that created it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VariableReference {
    /// Session that owns this adapter reference.
    pub session_id: u64,
    /// Stop generation at which the reference was received.
    pub stop_generation: u64,
    /// Adapter-assigned DAP `variablesReference`.
    pub adapter_reference: u64,
}

/// Asynchronous event emitted by one independent DAP session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DebugEvent {
    /// Lifecycle state changed.
    StateChanged {
        /// Manager-local session identity.
        session_id: u64,
        /// Current public state.
        info: DebugSessionInfo,
    },
    /// Correlated response to a host operation.
    Response {
        /// Manager-local session identity.
        session_id: u64,
        /// Host operation identity.
        request_id: u64,
        /// DAP command name.
        command: String,
        /// Whether the adapter accepted the request.
        success: bool,
        /// Adapter response body, when present.
        body: Option<Value>,
        /// Adapter rejection or diagnostic message.
        message: Option<String>,
    },
    /// Text output event from the adapter or debuggee.
    Output {
        /// Manager-local session identity.
        session_id: u64,
        /// DAP output category.
        category: String,
        /// Bounded plain-text output.
        output: String,
        /// Source path when the adapter supplied one.
        source: Option<String>,
        /// Zero-based source line when supplied.
        line: Option<u32>,
    },
    /// Adapter diagnostics from stderr.
    AdapterDiagnostic {
        /// Manager-local session identity.
        session_id: u64,
        /// Bounded diagnostic line.
        message: String,
    },
    /// Adapter updated a source breakpoint.
    BreakpointChanged {
        /// Manager-local session identity.
        session_id: u64,
        /// Source path reported by the adapter.
        source: String,
        /// Adapter breakpoint data or normalized stored breakpoint.
        breakpoint: Value,
    },
    /// Unhandled adapter event, preserved as bounded JSON for host consumers.
    AdapterEvent {
        /// Manager-local session identity.
        session_id: u64,
        /// DAP event name.
        event: String,
        /// Bounded event body.
        body: Option<Value>,
    },
    /// Adapter process exited or failed unexpectedly.
    Error {
        /// Manager-local session identity.
        session_id: u64,
        /// Safe-to-display error message.
        message: String,
    },
    /// Adapter and session resources were released.
    Closed {
        /// Manager-local session identity.
        session_id: u64,
    },
}

/// Errors from configuration, authorization, requests, transport, and lifecycle operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DebugError {
    /// Invalid adapter or launch configuration.
    InvalidConfiguration(String),
    /// Adapter or configuration does not exist.
    NotFound,
    /// Host authorization denied adapter startup or target access.
    AuthorizationDenied,
    /// Session is in a state that does not allow this operation.
    InvalidState(DebugSessionState),
    /// Requested adapter capability is unavailable.
    Unsupported(String),
    /// DAP request or event queue is full.
    Backpressure,
    /// A paused-state variable reference is stale or belongs to another session.
    StaleVariableReference,
    /// Adapter process, framing, or protocol operation failed.
    Transport(String),
}

impl fmt::Display for DebugError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration(message) | Self::Transport(message) => f.write_str(message),
            Self::NotFound => f.write_str("debug adapter, configuration, or session was not found"),
            Self::AuthorizationDenied => {
                f.write_str("debug adapter or target authorization was denied")
            }
            Self::InvalidState(state) => write!(f, "debug session is {state:?}"),
            Self::Unsupported(capability) => {
                write!(f, "debug adapter does not support {capability}")
            }
            Self::Backpressure => f.write_str("debug adapter or event queue is full"),
            Self::StaleVariableReference => {
                f.write_str("variable reference expired when execution resumed")
            }
        }
    }
}

impl std::error::Error for DebugError {}

#[derive(Clone, Debug)]
enum SessionCommand {
    Request {
        request_id: u64,
        command: String,
        arguments: Value,
    },
    ReplaceBreakpoints {
        source: PathBuf,
    },
    Stop,
}

/// Nonblocking command handle for one independently owned debug adapter actor.
#[derive(Clone)]
pub struct DebugSessionHandle {
    info: Arc<Mutex<DebugSessionInfo>>,
    commands: SyncSender<SessionCommand>,
    next_request: Arc<AtomicU64>,
}

impl fmt::Debug for DebugSessionHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DebugSessionHandle")
            .field("info", &self.info())
            .finish_non_exhaustive()
    }
}

impl DebugSessionHandle {
    /// Returns the current session snapshot.
    #[must_use]
    pub fn info(&self) -> DebugSessionInfo {
        lock(&self.info).clone()
    }

    /// Requests continue in the stopped thread.
    ///
    /// # Errors
    /// Returns an error when the session is not paused or its bounded command queue is full.
    pub fn continue_execution(&self) -> Result<u64, DebugError> {
        let info = self.info();
        if info.state != DebugSessionState::Paused {
            return Err(DebugError::InvalidState(info.state));
        }
        self.request(
            "continue",
            json!({"threadId": info.stopped_thread_id.unwrap_or(0)}),
        )
    }

    /// Requests pause in the current execution.
    ///
    /// # Errors
    /// Returns an error when the session is not running or its bounded command queue is full.
    pub fn pause(&self, thread_id: i64) -> Result<u64, DebugError> {
        let info = self.info();
        if info.state != DebugSessionState::Running {
            return Err(DebugError::InvalidState(info.state));
        }
        self.request("pause", json!({"threadId": thread_id}))
    }

    /// Requests step over in a stopped thread.
    ///
    /// # Errors
    /// Returns an error when the session is not paused or its bounded command queue is full.
    pub fn step_over(&self, thread_id: i64) -> Result<u64, DebugError> {
        self.require_paused()?;
        self.request(
            "next",
            json!({"threadId": thread_id, "singleThread": false}),
        )
    }

    /// Requests step into in a stopped thread.
    ///
    /// # Errors
    /// Returns an error when the session is not paused or its bounded command queue is full.
    pub fn step_into(&self, thread_id: i64) -> Result<u64, DebugError> {
        self.require_paused()?;
        self.request(
            "stepIn",
            json!({"threadId": thread_id, "singleThread": false}),
        )
    }

    /// Requests step out in a stopped thread.
    ///
    /// # Errors
    /// Returns an error when the session is not paused or its bounded command queue is full.
    pub fn step_out(&self, thread_id: i64) -> Result<u64, DebugError> {
        self.require_paused()?;
        self.request(
            "stepOut",
            json!({"threadId": thread_id, "singleThread": false}),
        )
    }

    /// Requests the current thread list.
    ///
    /// # Errors
    /// Returns an error when the session is finished or its bounded command queue is full.
    pub fn threads(&self) -> Result<u64, DebugError> {
        self.require_live()?;
        self.request("threads", json!({}))
    }

    /// Requests one bounded stack-trace page.
    ///
    /// # Errors
    /// Returns an error when the session is not paused or its bounded command queue is full.
    pub fn stack_trace(
        &self,
        thread_id: i64,
        start_frame: usize,
        levels: usize,
    ) -> Result<u64, DebugError> {
        self.require_paused()?;
        let levels = levels.clamp(1, MAX_DEBUG_PAGE_SIZE);
        self.request(
            "stackTrace",
            json!({"threadId": thread_id, "startFrame": start_frame, "levels": levels}),
        )
    }

    /// Requests scopes for a current stack frame.
    ///
    /// # Errors
    /// Returns an error when the session is not paused or its bounded command queue is full.
    pub fn scopes(&self, frame_id: i64) -> Result<u64, DebugError> {
        self.require_paused()?;
        self.request("scopes", json!({"frameId": frame_id}))
    }

    /// Requests one bounded variable page, rejecting references from older stops.
    ///
    /// # Errors
    /// Returns an error when the reference is stale, the session is not paused, or its command
    /// queue is full.
    pub fn variables(
        &self,
        reference: VariableReference,
        filter: Option<&str>,
        start: usize,
        count: usize,
    ) -> Result<u64, DebugError> {
        let info = self.info();
        if info.state != DebugSessionState::Paused {
            return Err(DebugError::InvalidState(info.state));
        }
        if reference.session_id != info.session_id
            || reference.stop_generation != info.stop_generation
        {
            return Err(DebugError::StaleVariableReference);
        }
        let count = count.clamp(1, MAX_DEBUG_PAGE_SIZE);
        let mut arguments = json!({"variablesReference": reference.adapter_reference, "start": start, "count": count});
        if let Some(filter) = filter {
            if filter == "named" || filter == "indexed" {
                arguments["filter"] = Value::String(filter.to_owned());
            }
        }
        if !info.capabilities.inspection.variable_paging {
            if let Some(object) = arguments.as_object_mut() {
                object.remove("start");
                object.remove("count");
            }
        }
        self.request("variables", arguments)
    }

    /// Requests restart only when the adapter advertised that capability.
    ///
    /// # Errors
    /// Returns an error when restart is unsupported, the session is finished, or the command
    /// queue is full.
    pub fn restart(&self) -> Result<u64, DebugError> {
        let info = self.info();
        if !info.capabilities.requests.restart {
            return Err(DebugError::Unsupported("restart".into()));
        }
        self.require_live()?;
        self.request("restart", json!({"arguments": {}}))
    }

    /// Stops or disconnects the adapter. Attached targets are always left running.
    ///
    /// # Errors
    /// Returns an error when the session is already finished or its bounded command queue is full.
    pub fn stop(&self) -> Result<(), DebugError> {
        let mut info = lock(&self.info);
        if matches!(
            info.state,
            DebugSessionState::Terminated | DebugSessionState::Failed
        ) {
            return Err(DebugError::InvalidState(info.state));
        }
        self.commands
            .try_send(SessionCommand::Stop)
            .map_err(|error| map_send_error(&error))?;
        info.state = DebugSessionState::Terminating;
        info.stop_generation = info.stop_generation.saturating_add(1);
        Ok(())
    }

    fn require_paused(&self) -> Result<(), DebugError> {
        let info = self.info();
        if info.state == DebugSessionState::Paused {
            Ok(())
        } else {
            Err(DebugError::InvalidState(info.state))
        }
    }

    fn require_live(&self) -> Result<(), DebugError> {
        let info = self.info();
        if matches!(
            info.state,
            DebugSessionState::Running | DebugSessionState::Paused
        ) {
            Ok(())
        } else {
            Err(DebugError::InvalidState(info.state))
        }
    }

    fn request(&self, command: &str, arguments: Value) -> Result<u64, DebugError> {
        let request_id = self.next_request.fetch_add(1, Ordering::Relaxed);
        self.commands
            .try_send(SessionCommand::Request {
                request_id,
                command: command.to_owned(),
                arguments,
            })
            .map_err(|error| map_send_error(&error))?;
        Ok(request_id)
    }

    fn replace_breakpoints(&self, source: PathBuf) -> Result<(), DebugError> {
        self.commands
            .try_send(SessionCommand::ReplaceBreakpoints { source })
            .map_err(|error| map_send_error(&error))
    }
}

struct SessionRecord {
    handle: DebugSessionHandle,
    thread: Option<JoinHandle<()>>,
}

/// Owns adapter definitions, configurations, breakpoints, and independent DAP sessions.
pub struct DebugManager {
    next_session_id: u64,
    next_breakpoint_id: u64,
    adapters: BTreeMap<String, DebugAdapterDefinition>,
    configurations: BTreeMap<String, DebugConfiguration>,
    breakpoints: Arc<Mutex<BTreeMap<PathBuf, Vec<SourceBreakpoint>>>>,
    sessions: BTreeMap<u64, SessionRecord>,
    event_tx: SyncSender<DebugEvent>,
    event_rx: Receiver<DebugEvent>,
}

impl fmt::Debug for DebugManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DebugManager")
            .field("adapter_count", &self.adapters.len())
            .field("configuration_count", &self.configurations.len())
            .field("session_count", &self.sessions.len())
            .finish_non_exhaustive()
    }
}

impl Default for DebugManager {
    fn default() -> Self {
        let (event_tx, event_rx) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
        Self {
            next_session_id: 0,
            next_breakpoint_id: 0,
            adapters: BTreeMap::new(),
            configurations: BTreeMap::new(),
            breakpoints: Arc::new(Mutex::new(BTreeMap::new())),
            sessions: BTreeMap::new(),
            event_tx,
            event_rx,
        }
    }
}

impl DebugManager {
    /// Creates a manager without launching an adapter or starting any actor thread.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a local adapter definition without starting it.
    ///
    /// # Errors
    /// Returns an error for invalid definitions or an identifier that is already registered.
    pub fn register_adapter(&mut self, adapter: DebugAdapterDefinition) -> Result<(), DebugError> {
        validate_adapter(&adapter)?;
        if self.adapters.contains_key(&adapter.id) {
            return Err(DebugError::InvalidConfiguration(
                "duplicate debug adapter id".into(),
            ));
        }
        self.adapters.insert(adapter.id.clone(), adapter);
        Ok(())
    }

    /// Registers a validated configuration for an existing adapter.
    ///
    /// # Errors
    /// Returns an error when validation fails, the adapter is missing, or the identifier is used.
    pub fn register_configuration(
        &mut self,
        configuration: DebugConfiguration,
    ) -> Result<(), DebugError> {
        configuration.validate()?;
        if !self.adapters.contains_key(&configuration.adapter_id) {
            return Err(DebugError::NotFound);
        }
        if self.configurations.contains_key(&configuration.id) {
            return Err(DebugError::InvalidConfiguration(
                "duplicate debug configuration id".into(),
            ));
        }
        self.configurations
            .insert(configuration.id.clone(), configuration);
        Ok(())
    }

    /// Removes a registered configuration while leaving any running session untouched.
    ///
    /// # Errors
    /// Returns [`DebugError::NotFound`] when the configuration is not registered.
    pub fn unregister_configuration(&mut self, id: &str) -> Result<(), DebugError> {
        self.configurations
            .remove(id)
            .map(|_| ())
            .ok_or(DebugError::NotFound)
    }

    /// Removes a registered adapter that no configuration references.
    ///
    /// # Errors
    /// Returns [`DebugError::NotFound`] when the adapter is not registered, or
    /// [`DebugError::InvalidConfiguration`] while a configuration still references it.
    pub fn unregister_adapter(&mut self, id: &str) -> Result<(), DebugError> {
        if self
            .configurations
            .values()
            .any(|configuration| configuration.adapter_id == id)
        {
            return Err(DebugError::InvalidConfiguration(
                "debug adapter is still referenced by a configuration".into(),
            ));
        }
        self.adapters
            .remove(id)
            .map(|_| ())
            .ok_or(DebugError::NotFound)
    }

    /// Lists registered local adapters.
    #[must_use]
    pub fn adapters(&self) -> Vec<DebugAdapterDefinition> {
        self.adapters.values().cloned().collect()
    }

    /// Lists validated launch or attach configurations.
    #[must_use]
    pub fn configurations(&self) -> Vec<DebugConfiguration> {
        self.configurations.values().cloned().collect()
    }

    /// Returns exact adapter and target details for a configuration approval prompt.
    ///
    /// # Errors
    /// Returns [`DebugError::NotFound`] when the configuration or adapter is not registered.
    pub fn authorization_details(
        &self,
        configuration_id: &str,
    ) -> Result<DebugAuthorizationDetails, DebugError> {
        let configuration = self
            .configurations
            .get(configuration_id)
            .ok_or(DebugError::NotFound)?
            .clone();
        let adapter = self
            .adapters
            .get(&configuration.adapter_id)
            .ok_or(DebugError::NotFound)?
            .clone();
        Ok(DebugAuthorizationDetails {
            adapter,
            configuration,
        })
    }

    /// Starts one adapter actor after separately authorizing the adapter and target.
    ///
    /// The method only spawns the actor thread; process startup and DAP initialization happen on
    /// that independent thread. A failed adapter cannot block this caller or other sessions.
    ///
    /// # Errors
    /// Returns an error when the session limit is reached, either authorization is denied, the
    /// identifier space is exhausted, or the session actor cannot be created.
    pub fn start_session(
        &mut self,
        configuration_id: &str,
        authorizer: &mut impl DebugAuthorizer,
    ) -> Result<DebugSessionInfo, DebugError> {
        if self.sessions.len() >= MAX_DEBUG_SESSIONS {
            return Err(DebugError::InvalidConfiguration(
                "maximum debug session count reached".into(),
            ));
        }
        let details = self.authorization_details(configuration_id)?;
        if !authorizer.authorize_adapter(&details.adapter)
            || !authorizer.authorize_target(&details.configuration)
        {
            return Err(DebugError::AuthorizationDenied);
        }
        self.next_session_id = self.next_session_id.checked_add(1).ok_or_else(|| {
            DebugError::InvalidConfiguration("debug session identifiers exhausted".into())
        })?;
        let session_id = self.next_session_id;
        let initial = DebugSessionInfo {
            session_id,
            adapter_id: details.adapter.id.clone(),
            configuration_id: details.configuration.id.clone(),
            request: details.configuration.request,
            state: DebugSessionState::Initializing,
            capabilities: DebugCapabilities::default(),
            stop_generation: 0,
            stopped_thread_id: None,
            exit_code: None,
            error: None,
        };
        let info = Arc::new(Mutex::new(initial.clone()));
        let (commands, receiver) = mpsc::sync_channel(COMMAND_QUEUE_CAPACITY);
        let handle = DebugSessionHandle {
            info: info.clone(),
            commands,
            next_request: Arc::new(AtomicU64::new(1)),
        };
        let startup = SessionStartup {
            info,
            adapter: details.adapter,
            configuration: details.configuration,
            commands: receiver,
            breakpoints: self.breakpoints.clone(),
            events: self.event_tx.clone(),
        };
        let thread = thread::Builder::new()
            .name(format!("koren-dap-{session_id}"))
            .spawn(move || run_session(startup))
            .map_err(|error| {
                DebugError::Transport(format!("could not start debug actor: {error}"))
            })?;
        self.sessions.insert(
            session_id,
            SessionRecord {
                handle,
                thread: Some(thread),
            },
        );
        self.publish(DebugEvent::StateChanged {
            session_id,
            info: initial.clone(),
        })?;
        Ok(initial)
    }

    /// Returns a handle to one active, initializing, or completed session.
    ///
    /// # Errors
    /// Returns [`DebugError::NotFound`] when no session has that identifier.
    pub fn session(&self, session_id: u64) -> Result<DebugSessionHandle, DebugError> {
        self.sessions
            .get(&session_id)
            .map(|record| record.handle.clone())
            .ok_or(DebugError::NotFound)
    }

    /// Returns the current public state of all sessions.
    #[must_use]
    pub fn sessions(&self) -> Vec<DebugSessionInfo> {
        self.sessions
            .values()
            .map(|record| record.handle.info())
            .collect()
    }

    /// Replaces all requested breakpoints for one source and synchronizes active sessions.
    ///
    /// # Errors
    /// Returns an error when the source path is not absolute, requested coordinates exceed DAP
    /// limits, the breakpoint count exceeds the limit, or an active session queue is full.
    pub fn set_source_breakpoints(
        &mut self,
        source: &Path,
        requested: Vec<BreakpointSpec>,
    ) -> Result<Vec<SourceBreakpoint>, DebugError> {
        if !source.is_absolute() || requested.len() > MAX_SOURCE_BREAKPOINTS {
            return Err(DebugError::InvalidConfiguration(
                "source path must be absolute and breakpoint count bounded".into(),
            ));
        }
        let source = normalize_source(source);
        let mut existing = lock(&self.breakpoints);
        let old = existing.get(&source).cloned().unwrap_or_default();
        let mut result = Vec::with_capacity(requested.len());
        for spec in requested {
            let prior = old
                .iter()
                .find(|item| item.line == spec.line && item.column == spec.column);
            let id = if let Some(prior) = prior {
                prior.id
            } else {
                self.next_breakpoint_id =
                    self.next_breakpoint_id.checked_add(1).ok_or_else(|| {
                        DebugError::InvalidConfiguration("breakpoint identifiers exhausted".into())
                    })?;
                self.next_breakpoint_id
            };
            if spec.line > i32::MAX as u32
                || spec.column.is_some_and(|column| column > i32::MAX as u32)
            {
                return Err(DebugError::InvalidConfiguration(
                    "breakpoint coordinates exceed DAP limits".into(),
                ));
            }
            result.push(SourceBreakpoint {
                id,
                line: spec.line,
                column: spec.column,
                condition: spec.condition,
                hit_condition: spec.hit_condition,
                log_message: spec.log_message,
                enabled: spec.enabled,
                verified: prior.and_then(|old| old.verified),
                actual_line: prior.and_then(|old| old.actual_line),
                message: prior.and_then(|old| old.message.clone()),
            });
        }
        existing.insert(source.clone(), result.clone());
        drop(existing);
        for record in self.sessions.values() {
            let info = record.handle.info();
            if !matches!(
                info.state,
                DebugSessionState::Terminated | DebugSessionState::Failed
            ) {
                record.handle.replace_breakpoints(source.clone())?;
            }
        }
        Ok(result)
    }

    /// Lists the current requested and adapter-verified source breakpoints.
    #[must_use]
    pub fn source_breakpoints(&self, source: &Path) -> Vec<SourceBreakpoint> {
        lock(&self.breakpoints)
            .get(&normalize_source(source))
            .cloned()
            .unwrap_or_default()
    }

    /// Drains bounded adapter events without blocking.
    #[must_use]
    pub fn drain_events(&self) -> Vec<DebugEvent> {
        self.event_rx.try_iter().collect()
    }

    /// Requests bounded teardown of every owned adapter and debug session.
    pub fn shutdown(&mut self) {
        for record in self.sessions.values() {
            let _ = record.handle.commands.try_send(SessionCommand::Stop);
        }
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        loop {
            if self
                .sessions
                .values()
                .all(|record| record.thread.as_ref().is_none_or(JoinHandle::is_finished))
                || Instant::now() >= deadline
            {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        for record in self.sessions.values_mut() {
            if record.thread.as_ref().is_some_and(JoinHandle::is_finished) {
                if let Some(join) = record.thread.take() {
                    let _ = join.join();
                }
            }
        }
    }

    fn publish(&self, event: DebugEvent) -> Result<(), DebugError> {
        self.event_tx
            .try_send(event)
            .map_err(|error| map_send_error(&error))
    }
}

impl Drop for DebugManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn validate_adapter(adapter: &DebugAdapterDefinition) -> Result<(), DebugError> {
    if adapter.id.trim().is_empty()
        || adapter.id.len() > 128
        || adapter.name.trim().is_empty()
        || adapter.name.len() > 128
        || adapter.command.trim().is_empty()
        || adapter.command.contains('\0')
        || adapter.arguments.len() > MAX_ADAPTER_ARGUMENTS
        || adapter
            .arguments
            .iter()
            .any(|argument| argument.contains('\0'))
        || adapter.working_directory.as_os_str().is_empty()
    {
        return Err(DebugError::InvalidConfiguration(
            "invalid debug adapter definition".into(),
        ));
    }
    for (key, value) in &adapter.environment {
        if key.is_empty() || key.contains('=') || key.contains('\0') || value.contains('\0') {
            return Err(DebugError::InvalidConfiguration(
                "invalid adapter environment override".into(),
            ));
        }
    }
    Ok(())
}

fn normalize_source(source: &Path) -> PathBuf {
    fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf())
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn map_send_error<T>(error: &TrySendError<T>) -> DebugError {
    match error {
        TrySendError::Full(_) => DebugError::Backpressure,
        TrySendError::Disconnected(_) => {
            DebugError::Transport("debug session actor stopped".into())
        }
    }
}

struct SessionStartup {
    info: Arc<Mutex<DebugSessionInfo>>,
    adapter: DebugAdapterDefinition,
    configuration: DebugConfiguration,
    commands: Receiver<SessionCommand>,
    breakpoints: Arc<Mutex<BTreeMap<PathBuf, Vec<SourceBreakpoint>>>>,
    events: SyncSender<DebugEvent>,
}

enum AdapterInput {
    Message(Value),
    Failed(String),
    Closed,
}

#[derive(Clone)]
struct PendingRequest {
    request_id: u64,
    command: String,
    source: Option<PathBuf>,
    requested_breakpoints: Vec<SourceBreakpoint>,
}

fn run_session(startup: SessionStartup) {
    let session_id = lock(&startup.info).session_id;
    let mut command = Command::new(&startup.adapter.command);
    command
        .args(&startup.adapter.arguments)
        .current_dir(&startup.adapter.working_directory)
        .envs(&startup.adapter.environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            fail_session(&startup, format!("debug adapter could not start: {error}"));
            return;
        }
    };
    let Some(stdin) = child.stdin.take() else {
        let _ = child.kill();
        fail_session(&startup, "debug adapter stdin was unavailable".into());
        return;
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        fail_session(&startup, "debug adapter stdout was unavailable".into());
        return;
    };
    let Some(stderr) = child.stderr.take() else {
        let _ = child.kill();
        fail_session(&startup, "debug adapter stderr was unavailable".into());
        return;
    };
    let (adapter_tx, adapter_rx) = mpsc::sync_channel(ADAPTER_QUEUE_CAPACITY);
    let reader_tx = adapter_tx.clone();
    let reader = thread::Builder::new()
        .name(format!("koren-dap-read-{session_id}"))
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_message(&mut reader) {
                    Ok(Some(message)) => {
                        if reader_tx.send(AdapterInput::Message(message)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {
                        let _ = reader_tx.send(AdapterInput::Closed);
                        break;
                    }
                    Err(error) => {
                        let _ = reader_tx.send(AdapterInput::Failed(error.to_string()));
                        break;
                    }
                }
            }
        });
    let Ok(reader) = reader else {
        let _ = child.kill();
        fail_session(&startup, "could not start DAP reader thread".into());
        return;
    };
    let output_tx = startup.events.clone();
    let stderr_thread = thread::Builder::new()
        .name(format!("koren-dap-stderr-{session_id}"))
        .spawn(move || {
            let mut lines = BufReader::new(stderr).lines();
            while let Some(Ok(mut line)) = lines.next() {
                if line.len() > MAX_DIAGNOSTIC_BYTES {
                    line.truncate(MAX_DIAGNOSTIC_BYTES);
                }
                if output_tx
                    .send(DebugEvent::AdapterDiagnostic {
                        session_id,
                        message: line,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

    let mut actor = SessionActor {
        startup,
        child,
        stdin,
        adapter_rx,
        reader: Some(reader),
        stderr_thread: stderr_thread.ok(),
        next_sequence: 1,
        next_event_sequence: 0,
        pending: BTreeMap::new(),
        startup_progress: StartupProgress::default(),
        stopped: false,
        disconnect_after_terminate: false,
        stop_deadline: None,
        last_activity: Instant::now(),
    };
    actor.run();
}

struct SessionActor {
    startup: SessionStartup,
    child: Child,
    stdin: ChildStdin,
    adapter_rx: Receiver<AdapterInput>,
    reader: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
    next_sequence: u64,
    next_event_sequence: u64,
    pending: BTreeMap<u64, PendingRequest>,
    startup_progress: StartupProgress,
    stopped: bool,
    disconnect_after_terminate: bool,
    stop_deadline: Option<Instant>,
    last_activity: Instant,
}

#[derive(Default)]
struct StartupProgress {
    initialized_event: bool,
    initialized_response: bool,
    launch_response: bool,
}

impl SessionActor {
    fn run(&mut self) {
        if let Err(error) = self.initialize() {
            if self.info().state != DebugSessionState::Failed {
                self.fail(error.to_string());
            }
            self.cleanup(true);
            return;
        }
        loop {
            if let Ok(command) = self.startup.commands.recv_timeout(Duration::from_millis(8)) {
                match command {
                    SessionCommand::Request {
                        request_id,
                        command,
                        arguments,
                    } => {
                        if let Err(error) =
                            self.send_operation(request_id, &command, arguments, None, Vec::new())
                        {
                            self.emit(DebugEvent::Error {
                                session_id: self.session_id(),
                                message: error.to_string(),
                            });
                        }
                    }
                    SessionCommand::ReplaceBreakpoints { source } => self.sync_source(&source),
                    SessionCommand::Stop => {
                        self.set_state(DebugSessionState::Terminating, None);
                        self.stop_deadline = Some(Instant::now() + STOP_TIMEOUT);
                        self.begin_disconnect();
                    }
                }
            }
            match self.adapter_rx.try_recv() {
                Ok(AdapterInput::Message(message)) => self.handle_message(&message),
                Ok(AdapterInput::Failed(message)) => {
                    self.fail(message);
                    break;
                }
                Ok(AdapterInput::Closed) => {
                    if !self.stopped
                        && !matches!(
                            self.info().state,
                            DebugSessionState::Terminated | DebugSessionState::Failed
                        )
                    {
                        self.fail("debug adapter closed its output unexpectedly".into());
                    }
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    self.fail("debug adapter reader stopped".into());
                    break;
                }
                Err(TryRecvError::Empty) => {}
            }
            if self
                .stop_deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
            {
                self.cleanup(true);
                break;
            }
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    if !self.stopped {
                        self.fail(format!("debug adapter exited unexpectedly ({status})"));
                    }
                    break;
                }
                Ok(None) => {}
                Err(error) => {
                    self.fail(format!("could not query debug adapter: {error}"));
                    break;
                }
            }
            if self
                .pending
                .values()
                .any(|pending| pending.command != "launch" && pending.command != "attach")
                && self.last_activity.elapsed() > REQUEST_TIMEOUT
            {
                self.fail("debug adapter request timed out".into());
                break;
            }
        }
        self.cleanup(false);
    }

    fn initialize(&mut self) -> Result<(), DebugError> {
        let sequence = self.send_request(
            "initialize",
            json!({
                "clientID": "koren-code",
                "clientName": "Koren Code Editor",
                "adapterID": self.startup.adapter.id,
                "locale": "en",
                "linesStartAt1": true,
                "columnsStartAt1": true,
                "pathFormat": "path",
                "supportsVariableType": true,
                "supportsVariablePaging": true,
                "supportsRunInTerminalRequest": false,
                "supportsProgressReporting": true,
                "supportsInvalidatedEvent": true
            }),
        )?;
        self.await_initialization(sequence)?;
        self.set_state(DebugSessionState::Configuring, None);
        self.configure_sources()?;
        self.await_launch()?;
        if self.info().state == DebugSessionState::Configuring {
            self.set_state(DebugSessionState::Running, None);
        }
        Ok(())
    }

    fn await_initialization(&mut self, initialize_sequence: u64) -> Result<(), DebugError> {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        while Instant::now() < deadline
            && (!self.startup_progress.initialized_response
                || !self.startup_progress.initialized_event)
        {
            self.receive_startup_message("initialization", Some(initialize_sequence))?;
            if self.startup_progress.initialized_response
                && !self.startup_progress.launch_response
                && !self
                    .pending
                    .values()
                    .any(|pending| pending.command == "launch" || pending.command == "attach")
            {
                let command = match self.startup.configuration.request {
                    DebugRequestType::Launch => "launch",
                    DebugRequestType::Attach => "attach",
                };
                let arguments = configuration_arguments(&self.startup.configuration);
                self.send_operation(0, command, arguments, None, Vec::new())?;
            }
            if self.info().state == DebugSessionState::Failed {
                return Err(DebugError::Transport(
                    self.info()
                        .error
                        .unwrap_or_else(|| "DAP initialize failed".into()),
                ));
            }
        }
        if !self.startup_progress.initialized_response || !self.startup_progress.initialized_event {
            return Err(DebugError::Transport(format!(
                "debug adapter did not complete initialization request {initialize_sequence}"
            )));
        }
        Ok(())
    }

    fn receive_startup_message(
        &mut self,
        phase: &str,
        initialize_sequence: Option<u64>,
    ) -> Result<(), DebugError> {
        match self.adapter_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(AdapterInput::Message(message)) => {
                if let Some(sequence) = initialize_sequence {
                    self.handle_initialize_message(&message, sequence);
                } else {
                    self.handle_message(&message);
                }
            }
            Ok(AdapterInput::Failed(message)) => return Err(DebugError::Transport(message)),
            Ok(AdapterInput::Closed) => {
                return Err(DebugError::Transport(format!(
                    "debug adapter closed during {phase}"
                )));
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Some(status) = self
                    .child
                    .try_wait()
                    .map_err(|error| DebugError::Transport(error.to_string()))?
                {
                    return Err(DebugError::Transport(format!(
                        "debug adapter exited during {phase} ({status})"
                    )));
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(DebugError::Transport(format!(
                    "debug adapter reader stopped during {phase}"
                )));
            }
        }
        Ok(())
    }

    fn configure_sources(&mut self) -> Result<(), DebugError> {
        let sources = lock(&self.startup.breakpoints)
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for source in sources {
            self.sync_source(&source);
        }
        if self.info().capabilities.requests.configuration_done {
            let request_id = self.alloc_event_id();
            self.send_operation(request_id, "configurationDone", json!({}), None, Vec::new())?;
        }
        Ok(())
    }

    fn await_launch(&mut self) -> Result<(), DebugError> {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        while Instant::now() < deadline && !self.startup_progress.launch_response {
            self.receive_startup_message("launch", None)?;
        }
        if !self.startup_progress.launch_response {
            return Err(DebugError::Transport(
                "debug adapter launch timed out".into(),
            ));
        }
        Ok(())
    }

    fn handle_initialize_message(&mut self, message: &Value, initialize_sequence: u64) {
        match message.get("type").and_then(Value::as_str) {
            Some("response")
                if message.get("request_seq").and_then(Value::as_u64)
                    == Some(initialize_sequence) =>
            {
                let success = message
                    .get("success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !success {
                    self.fail(
                        message
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("DAP initialize request failed")
                            .to_owned(),
                    );
                    self.startup_progress.initialized_response = true;
                    return;
                }
                let body = message.get("body").and_then(Value::as_object);
                if let Some(body) = body {
                    lock(&self.startup.info).capabilities = parse_capabilities(body);
                }
                self.startup_progress.initialized_response = true;
                self.last_activity = Instant::now();
            }
            _ => self.handle_message(message),
        }
    }

    fn handle_message(&mut self, message: &Value) {
        self.last_activity = Instant::now();
        match message.get("type").and_then(Value::as_str) {
            Some("response") => self.handle_response(message),
            Some("event") => self.handle_event(message),
            Some("request") => self.reject_reverse_request(message),
            _ => self.fail("debug adapter sent a malformed DAP message".into()),
        }
    }

    fn handle_response(&mut self, message: &Value) {
        let Some(sequence) = message.get("request_seq").and_then(Value::as_u64) else {
            self.fail("DAP response omitted request_seq".into());
            return;
        };
        let Some(pending) = self.pending.remove(&sequence) else {
            return;
        };
        let success = message
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let body = message.get("body").cloned();
        let text = message
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if pending.command == "terminate" && self.disconnect_after_terminate {
            self.disconnect_after_terminate = false;
            self.send_disconnect(false);
        }
        if pending.command == "launch" || pending.command == "attach" {
            self.startup_progress.launch_response = success;
        }
        if pending.command == "setBreakpoints" {
            if let Some(source) = pending.source.as_ref() {
                self.apply_breakpoint_response(
                    source,
                    &pending.requested_breakpoints,
                    body.as_ref(),
                );
            }
        }
        if let Some(source) = pending.source.as_ref() {
            let _ = source;
        }
        if pending.request_id != 0 {
            self.emit(DebugEvent::Response {
                session_id: self.session_id(),
                request_id: pending.request_id,
                command: pending.command,
                success,
                body,
                message: text.clone(),
            });
        }
        if !success
            && matches!(
                self.info().state,
                DebugSessionState::Initializing | DebugSessionState::Configuring
            )
        {
            self.fail(text.unwrap_or_else(|| "debug adapter rejected startup".into()));
        }
    }

    fn handle_event(&mut self, message: &Value) {
        let Some(event) = message.get("event").and_then(Value::as_str) else {
            self.fail("DAP event omitted its event name".into());
            return;
        };
        let body = message.get("body").cloned();
        match event {
            "initialized" => {
                self.startup_progress.initialized_event = true;
                self.set_state(DebugSessionState::Configuring, None);
            }
            "stopped" => {
                let thread_id = body
                    .as_ref()
                    .and_then(|body| body.get("threadId"))
                    .and_then(Value::as_i64);
                {
                    let mut info = lock(&self.startup.info);
                    info.state = DebugSessionState::Paused;
                    info.stop_generation = info.stop_generation.saturating_add(1);
                    info.stopped_thread_id = thread_id;
                }
                self.emit_state();
            }
            "continued" => {
                {
                    let mut info = lock(&self.startup.info);
                    info.state = DebugSessionState::Running;
                    info.stop_generation = info.stop_generation.saturating_add(1);
                    info.stopped_thread_id = None;
                }
                self.emit_state();
            }
            "exited" => {
                lock(&self.startup.info).exit_code = body
                    .as_ref()
                    .and_then(|body| body.get("exitCode"))
                    .and_then(Value::as_i64)
                    .and_then(|code| i32::try_from(code).ok());
                self.set_state(DebugSessionState::Terminated, None);
            }
            "terminated" => self.set_state(DebugSessionState::Terminated, None),
            "output" => {
                let output = body
                    .as_ref()
                    .and_then(|body| body.get("output"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let output = truncate_string(output, MAX_DIAGNOSTIC_BYTES);
                let category = body
                    .as_ref()
                    .and_then(|body| body.get("category"))
                    .and_then(Value::as_str)
                    .unwrap_or("console")
                    .to_owned();
                let source = body
                    .as_ref()
                    .and_then(|body| body.get("source"))
                    .and_then(|source| source.get("path"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let line = body
                    .as_ref()
                    .and_then(|body| body.get("line"))
                    .and_then(Value::as_u64)
                    .and_then(|line| u32::try_from(line).ok())
                    .map(|line| line.saturating_sub(1));
                self.emit(DebugEvent::Output {
                    session_id: self.session_id(),
                    category,
                    output,
                    source,
                    line,
                });
            }
            "breakpoint" => {
                let breakpoint = body
                    .as_ref()
                    .and_then(|body| body.get("breakpoint"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let source = breakpoint
                    .get("source")
                    .and_then(|source| source.get("path"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                self.emit(DebugEvent::BreakpointChanged {
                    session_id: self.session_id(),
                    source,
                    breakpoint,
                });
            }
            _ => self.emit(DebugEvent::AdapterEvent {
                session_id: self.session_id(),
                event: event.to_owned(),
                body,
            }),
        }
    }

    fn reject_reverse_request(&mut self, message: &Value) {
        let request_seq = message.get("seq").and_then(Value::as_u64).unwrap_or(0);
        let command = message
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let response = json!({
            "seq": self.alloc_sequence(),
            "type": "response",
            "request_seq": request_seq,
            "command": command,
            "success": false,
            "message": if command == "runInTerminal" { "runInTerminal is not supported; the adapter may not launch a process through Koren" } else { "adapter reverse requests are not supported" }
        });
        if let Err(error) = write_message(&mut self.stdin, &response) {
            self.fail(error.to_string());
        }
        self.emit(DebugEvent::AdapterDiagnostic {
            session_id: self.session_id(),
            message: format!("Rejected unsupported adapter request '{command}'"),
        });
    }

    fn send_operation(
        &mut self,
        request_id: u64,
        command: &str,
        arguments: Value,
        source: Option<PathBuf>,
        requested_breakpoints: Vec<SourceBreakpoint>,
    ) -> Result<(), DebugError> {
        let sequence = self.send_request(command, arguments)?;
        self.pending.insert(
            sequence,
            PendingRequest {
                request_id,
                command: command.to_owned(),
                source,
                requested_breakpoints,
            },
        );
        Ok(())
    }

    fn send_request(&mut self, command: &str, arguments: Value) -> Result<u64, DebugError> {
        let sequence = self.alloc_sequence();
        let mut message = Map::new();
        message.insert("seq".into(), json!(sequence));
        message.insert("type".into(), json!("request"));
        message.insert("command".into(), json!(command));
        message.insert("arguments".into(), arguments);
        let message = Value::Object(message);
        write_message(&mut self.stdin, &message)
            .map_err(|error| DebugError::Transport(error.to_string()))?;
        self.last_activity = Instant::now();
        Ok(sequence)
    }

    fn sync_source(&mut self, source: &Path) {
        let entries = lock(&self.startup.breakpoints)
            .get(source)
            .cloned()
            .unwrap_or_default();
        let active = entries
            .iter()
            .filter(|entry| entry.enabled)
            .cloned()
            .collect::<Vec<_>>();
        let source_value = json!({"name":source.file_name().and_then(|name| name.to_str()).unwrap_or_default(),"path":source});
        let breakpoints = active
            .iter()
            .map(|entry| {
                let mut value = json!({"line":entry.line.saturating_add(1)});
                if let Some(column) = entry.column {
                    value["column"] = json!(column.saturating_add(1));
                }
                if let Some(condition) = &entry.condition {
                    value["condition"] = json!(condition);
                }
                if let Some(hit_condition) = &entry.hit_condition {
                    value["hitCondition"] = json!(hit_condition);
                }
                if let Some(log_message) = &entry.log_message {
                    value["logMessage"] = json!(log_message);
                }
                value
            })
            .collect::<Vec<_>>();
        let request_id = self.alloc_event_id();
        let args = json!({"source":source_value,"breakpoints":breakpoints,"sourceModified":false});
        let _ = self.send_operation(
            request_id,
            "setBreakpoints",
            args,
            Some(source.to_path_buf()),
            active,
        );
    }

    fn apply_breakpoint_response(
        &mut self,
        source: &Path,
        requested: &[SourceBreakpoint],
        body: Option<&Value>,
    ) {
        let response = body
            .and_then(|body| body.get("breakpoints"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut entries = lock(&self.startup.breakpoints);
        let Some(current) = entries.get_mut(source) else {
            return;
        };
        let mut published = Vec::new();
        for (index, item) in requested.iter().enumerate() {
            let Some(stored) = current.iter_mut().find(|stored| stored.id == item.id) else {
                continue;
            };
            let result = response.get(index);
            stored.verified = result
                .and_then(|result| result.get("verified"))
                .and_then(Value::as_bool)
                .or(Some(false));
            stored.actual_line = result
                .and_then(|result| result.get("line"))
                .and_then(Value::as_u64)
                .and_then(|line| u32::try_from(line).ok())
                .map(|line| line.saturating_sub(1));
            stored.message = result
                .and_then(|result| result.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            published.push(stored.clone());
        }
        drop(entries);
        let source_text = source.to_string_lossy().into_owned();
        for stored in published {
            self.emit(DebugEvent::BreakpointChanged {
                session_id: self.session_id(),
                source: source_text.clone(),
                breakpoint: serde_json::to_value(stored).unwrap_or(Value::Null),
            });
        }
    }

    fn begin_disconnect(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        let launched = self.info().request == DebugRequestType::Launch;
        let capabilities = self.info().capabilities;
        if launched && capabilities.execution.terminate {
            self.disconnect_after_terminate = true;
            let _ = self.send_operation(0, "terminate", json!({"restart":false}), None, Vec::new());
        } else {
            self.send_disconnect(launched);
        }
    }

    fn send_disconnect(&mut self, terminate_debuggee: bool) {
        let _ = self.send_operation(
            0,
            "disconnect",
            json!({"restart":false,"terminateDebuggee":terminate_debuggee}),
            None,
            Vec::new(),
        );
    }

    fn cleanup(&mut self, kill: bool) {
        if kill {
            let _ = self.child.kill();
        }
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        for worker in [&mut self.reader, &mut self.stderr_thread] {
            if worker.as_ref().is_some_and(JoinHandle::is_finished) {
                if let Some(handle) = worker.take() {
                    let _ = handle.join();
                }
            }
        }
        if !matches!(
            self.info().state,
            DebugSessionState::Failed | DebugSessionState::Terminated
        ) {
            self.set_state(DebugSessionState::Terminated, None);
        }
        self.emit(DebugEvent::Closed {
            session_id: self.session_id(),
        });
    }

    fn fail(&mut self, message: String) {
        self.set_state(DebugSessionState::Failed, Some(message.clone()));
        self.emit(DebugEvent::Error {
            session_id: self.session_id(),
            message,
        });
    }

    fn set_state(&mut self, state: DebugSessionState, error: Option<String>) {
        {
            let mut info = lock(&self.startup.info);
            info.state = state;
            if let Some(error) = error {
                info.error = Some(error);
            }
            if matches!(
                state,
                DebugSessionState::Running
                    | DebugSessionState::Terminated
                    | DebugSessionState::Failed
            ) {
                info.stop_generation = info.stop_generation.saturating_add(1);
                info.stopped_thread_id = None;
            }
        }
        self.emit_state();
    }

    fn emit_state(&self) {
        self.emit(DebugEvent::StateChanged {
            session_id: self.session_id(),
            info: self.info(),
        });
    }

    fn emit(&self, event: DebugEvent) {
        let _ = self.startup.events.try_send(event);
    }

    fn alloc_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        sequence
    }

    fn alloc_event_id(&mut self) -> u64 {
        self.next_event_sequence = self.next_event_sequence.saturating_add(1);
        self.next_event_sequence
    }

    fn session_id(&self) -> u64 {
        lock(&self.startup.info).session_id
    }
    fn info(&self) -> DebugSessionInfo {
        lock(&self.startup.info).clone()
    }
}

fn fail_session(startup: &SessionStartup, message: String) {
    {
        let mut info = lock(&startup.info);
        info.state = DebugSessionState::Failed;
        info.error = Some(message.clone());
    }
    let info = lock(&startup.info).clone();
    let _ = startup.events.send(DebugEvent::StateChanged {
        session_id: info.session_id,
        info,
    });
    let _ = startup.events.send(DebugEvent::Error {
        session_id: startup
            .info
            .lock()
            .map(|info| info.session_id)
            .unwrap_or_default(),
        message,
    });
}

fn configuration_arguments(configuration: &DebugConfiguration) -> Value {
    let mut arguments = configuration.adapter_settings.clone();
    match configuration.request {
        DebugRequestType::Launch => {
            arguments.insert("program".into(), json!(configuration.program));
            arguments.insert("args".into(), json!(configuration.arguments));
            arguments.insert("cwd".into(), json!(configuration.working_directory));
            arguments.insert("env".into(), json!(configuration.environment));
            arguments.insert("stopOnEntry".into(), json!(configuration.stop_on_entry));
        }
        DebugRequestType::Attach => {
            arguments.insert("processId".into(), json!(configuration.process_id));
            if let Some(program) = &configuration.program {
                arguments.insert("program".into(), json!(program));
            }
            arguments.insert("cwd".into(), json!(configuration.working_directory));
            arguments.insert("env".into(), json!(configuration.environment));
        }
    }
    Value::Object(arguments)
}

fn parse_capabilities(body: &Map<String, Value>) -> DebugCapabilities {
    let supported = |name: &str| body.get(name).and_then(Value::as_bool).unwrap_or(false);
    DebugCapabilities {
        requests: DebugRequestCapabilities {
            configuration_done: supported("supportsConfigurationDoneRequest"),
            restart: supported("supportsRestartRequest"),
            cancel: supported("supportsCancelRequest"),
        },
        execution: DebugExecutionCapabilities {
            terminate: supported("supportsTerminateRequest"),
        },
        inspection: DebugInspectionCapabilities {
            variable_paging: supported("supportsVariablePaging"),
            evaluate_for_hovers: supported("supportsEvaluateForHovers"),
        },
    }
}

fn truncate_string(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut content_length = None;
    let mut header_bytes = 0usize;
    loop {
        let mut line = Vec::new();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            if header_bytes == 0 {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "incomplete DAP header",
            ));
        }
        header_bytes = header_bytes.saturating_add(read);
        if header_bytes > MAX_DAP_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "DAP header exceeds size limit",
            ));
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
        let line = std::str::from_utf8(&line)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "DAP header is not ASCII"))?;
        let (name, value) = line
            .trim_end_matches(['\r', '\n'])
            .split_once(':')
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "malformed DAP header field")
            })?;
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "duplicate DAP Content-Length",
                ));
            }
            content_length = Some(value.trim().parse::<usize>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid DAP Content-Length")
            })?);
        }
    }
    let length = content_length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "DAP Content-Length is required")
    })?;
    if length > MAX_DAP_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "DAP message exceeds size limit",
        ));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn write_message(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    if body.len() > MAX_DAP_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "DAP message exceeds size limit",
        ));
    }
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        io::Cursor,
        sync::atomic::{AtomicUsize, Ordering},
    };

    static TEMP_ID: AtomicUsize = AtomicUsize::new(0);
    fn temp_dir() -> PathBuf {
        let path = env::temp_dir().join(format!(
            "koren-debug-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
    struct DenyTarget;
    impl DebugAuthorizer for DenyTarget {
        fn authorize_adapter(&mut self, _: &DebugAdapterDefinition) -> bool {
            true
        }
        fn authorize_target(&mut self, _: &DebugConfiguration) -> bool {
            false
        }
    }
    fn adapter(root: &Path) -> DebugAdapterDefinition {
        let python = if cfg!(windows) { "python" } else { "python3" };
        DebugAdapterDefinition {
            id: "fake".into(),
            name: "Fake DAP".into(),
            command: python.into(),
            arguments: vec![
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fake_adapter.py")
                    .to_string_lossy()
                    .into_owned(),
            ],
            working_directory: root.into(),
            environment: [("KOREN_FAKE_DAP_MODE".into(), "normal".into())]
                .into_iter()
                .collect(),
        }
    }
    fn configuration(root: &Path) -> DebugConfiguration {
        DebugConfiguration {
            id: "fake.launch".into(),
            name: "Fake launch".into(),
            adapter_id: "fake".into(),
            request: DebugRequestType::Launch,
            program: Some(root.join("fixture")),
            process_id: None,
            arguments: vec!["hello".into()],
            working_directory: root.into(),
            environment: BTreeMap::new(),
            adapter_settings: Map::new(),
            stop_on_entry: true,
        }
    }

    #[test]
    fn content_length_framing_counts_utf8_bytes_and_round_trips_messages() {
        let message = json!({"type":"event","event":"output","body":{"output":"λ"}});
        let mut bytes = Vec::new();
        write_message(&mut bytes, &message).unwrap();
        let mut reader = Cursor::new(bytes);
        assert_eq!(read_message(&mut reader).unwrap(), Some(message));
        assert_eq!(read_message(&mut reader).unwrap(), None);
    }

    #[test]
    fn framing_rejects_duplicate_missing_and_oversized_lengths() {
        for bytes in [
            b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}".as_slice(),
            b"X-Test: 2\r\n\r\n{}".as_slice(),
            b"Content-Length: 16777217\r\n\r\n".as_slice(),
        ] {
            assert!(read_message(&mut Cursor::new(bytes)).is_err());
        }
    }

    #[test]
    fn configuration_validation_preserves_common_fields_and_bounds_settings() {
        let root = temp_dir();
        let mut config = configuration(&root);
        config
            .adapter_settings
            .insert("program".into(), json!("override"));
        assert!(config.validate().is_err());
        let mut config = configuration(&root);
        config
            .adapter_settings
            .insert("options".into(), json!("ok"));
        assert!(config.validate().is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn denied_target_authorization_never_spawns_adapter_and_manager_is_lazy() {
        let root = temp_dir();
        let pid_file = root.join("spawned");
        let mut manager = DebugManager::new();
        manager.register_adapter(adapter(&root)).unwrap();
        manager
            .register_configuration(configuration(&root))
            .unwrap();
        assert_eq!(manager.sessions().len(), 0);
        assert!(
            manager
                .start_session("fake.launch", &mut DenyTarget)
                .is_err()
        );
        assert_eq!(manager.sessions().len(), 0);
        assert!(!pid_file.exists());
        assert!(manager.unregister_adapter("fake").is_err());
        manager.unregister_configuration("fake.launch").unwrap();
        manager.unregister_adapter("fake").unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    struct Allow;
    impl DebugAuthorizer for Allow {
        fn authorize_adapter(&mut self, _: &DebugAdapterDefinition) -> bool {
            true
        }
        fn authorize_target(&mut self, _: &DebugConfiguration) -> bool {
            true
        }
    }

    #[test]
    fn unanswered_dap_request_times_out_and_fails_only_its_session() {
        let root = temp_dir();
        let mut adapter = adapter(&root);
        adapter
            .environment
            .insert("KOREN_FAKE_DAP_MODE".into(), "drop-threads".into());
        let mut manager = DebugManager::new();
        manager.register_adapter(adapter).unwrap();
        manager
            .register_configuration(configuration(&root))
            .unwrap();
        let started = manager.start_session("fake.launch", &mut Allow).unwrap();
        let session = manager.session(started.session_id).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.info().state != DebugSessionState::Paused && Instant::now() < deadline {
            thread::yield_now();
        }
        assert_eq!(
            session.info().state,
            DebugSessionState::Paused,
            "{:?}",
            session.info()
        );
        session.threads().unwrap();
        while session.info().state != DebugSessionState::Failed && Instant::now() < deadline {
            thread::yield_now();
        }
        assert_eq!(
            session.info().state,
            DebugSessionState::Failed,
            "{:?}",
            session.info()
        );
        assert!(
            session
                .info()
                .error
                .as_deref()
                .is_some_and(|error| error.contains("timed out"))
        );
        manager.shutdown();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_breakpoints_replace_per_source_and_preserve_identity() {
        let root = temp_dir();
        let source = root.join("main.fake");
        fs::write(&source, "x\n").unwrap();
        let mut manager = DebugManager::new();
        let first = manager
            .set_source_breakpoints(
                &source,
                vec![BreakpointSpec {
                    line: 0,
                    column: None,
                    condition: None,
                    hit_condition: None,
                    log_message: None,
                    enabled: true,
                }],
            )
            .unwrap();
        let replaced = manager
            .set_source_breakpoints(
                &source,
                vec![BreakpointSpec {
                    line: 0,
                    column: None,
                    condition: Some("x".into()),
                    hit_condition: None,
                    log_message: None,
                    enabled: false,
                }],
            )
            .unwrap();
        assert_eq!(replaced.len(), 1);
        assert_eq!(replaced[0].id, first[0].id);
        assert_eq!(replaced[0].condition.as_deref(), Some("x"));
        assert_eq!(manager.source_breakpoints(&source), replaced);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn variable_references_expire_after_continue_or_a_new_stop() {
        let info = Arc::new(Mutex::new(DebugSessionInfo {
            session_id: 1,
            adapter_id: "a".into(),
            configuration_id: "c".into(),
            request: DebugRequestType::Launch,
            state: DebugSessionState::Paused,
            capabilities: DebugCapabilities::default(),
            stop_generation: 7,
            stopped_thread_id: Some(2),
            exit_code: None,
            error: None,
        }));
        let (tx, _rx) = mpsc::sync_channel(8);
        let handle = DebugSessionHandle {
            info: info.clone(),
            commands: tx,
            next_request: Arc::new(AtomicU64::new(1)),
        };
        let reference = VariableReference {
            session_id: 1,
            stop_generation: 7,
            adapter_reference: 12,
        };
        assert!(handle.variables(reference, None, 0, 1000).is_ok());
        lock(&info).stop_generation += 1;
        assert_eq!(
            handle.variables(reference, None, 0, 1),
            Err(DebugError::StaleVariableReference)
        );
    }

    #[test]
    fn helper_profiles_are_independent_and_distinguish_attach_cleanup() {
        let root = temp_dir();
        let launch = configuration(&root);
        let mut attach = launch.clone();
        attach.id = "fake.attach".into();
        attach.request = DebugRequestType::Attach;
        attach.process_id = Some(1234);
        attach.program = None;
        assert!(launch.validate().is_ok());
        assert!(attach.validate().is_ok());
        assert_eq!(configuration_arguments(&attach)["processId"], 1234);
        fs::remove_dir_all(root).unwrap();
    }
}
