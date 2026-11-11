//! Small, frontend-independent lifecycle primitives for Koren Platform.

use std::collections::VecDeque;
use std::path::PathBuf;

/// Ordered notification emitted by a platform service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlatformEvent {
    /// A workspace became available.
    WorkspaceOpened {
        /// Canonical root path of the workspace.
        root: PathBuf,
    },
    /// A workspace was closed.
    WorkspaceClosed {
        /// Canonical root path that was closed.
        root: PathBuf,
    },
    /// A document buffer was created.
    DocumentOpened {
        /// Stable identifier within its document manager.
        document_id: u64,
        /// Workspace root, or `None` for an untitled document.
        workspace_root: Option<PathBuf>,
        /// Workspace relative path, or `None` for an untitled document.
        path: Option<PathBuf>,
        /// Initial buffer revision.
        revision: u64,
    },
    /// A document buffer changed.
    DocumentChanged {
        /// Stable identifier within its document manager.
        document_id: u64,
        /// Revision after the change.
        revision: u64,
    },
    /// A document was saved successfully.
    DocumentSaved {
        /// Stable identifier within its document manager.
        document_id: u64,
        /// Saved revision.
        revision: u64,
    },
    /// A document was closed.
    DocumentClosed {
        /// Stable identifier within its document manager.
        document_id: u64,
    },
    /// A save detected that its backing file changed externally.
    SaveConflictDetected {
        /// Stable identifier within its document manager.
        document_id: u64,
        /// Buffer revision whose save encountered the conflict.
        revision: u64,
    },
    /// A logical project was registered.
    ProjectRegistered {
        /// Manager-local project identifier.
        project_id: u64,
    },
    /// A logical project was removed.
    ProjectRemoved {
        /// Manager-local project identifier.
        project_id: u64,
    },
    /// A task process started.
    TaskStarted {
        /// Manager-local run identifier.
        run_id: u64,
    },
    /// New process output was captured.
    TaskOutputAvailable {
        /// Manager-local run identifier.
        run_id: u64,
    },
    /// A task completed successfully or unsuccessfully.
    TaskFinished {
        /// Manager-local run identifier.
        run_id: u64,
    },
    /// A task was cancelled.
    TaskCancelled {
        /// Manager-local run identifier.
        run_id: u64,
    },
    /// A task failed to start or encountered an execution error.
    TaskFailed {
        /// Manager-local run identifier.
        run_id: u64,
    },
}

/// In-process FIFO event queue owned and drained by a platform service consumer.
///
/// Events are delivered synchronously in insertion order when drained. They are
/// not persisted or broadcast to listeners; the owner is responsible for
/// draining the queue and deciding how to distribute notifications.
#[derive(Debug, Default)]
pub struct EventQueue {
    events: VecDeque<PlatformEvent>,
}

impl EventQueue {
    /// Creates an empty event queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an event to the end of the queue.
    pub fn push(&mut self, event: PlatformEvent) {
        self.events.push_back(event);
    }

    /// Removes all queued events in their original order.
    pub fn drain(&mut self) -> Vec<PlatformEvent> {
        self.events.drain(..).collect()
    }
}

/// Current lifecycle phase of a [`Platform`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleState {
    /// Constructed, but not initialized.
    Uninitialized,
    /// Ready for platform services to be used.
    Running,
    /// Shut down; this instance cannot be restarted.
    Stopped,
}

/// Structured lifecycle failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformError {
    /// The requested transition is invalid for the current state.
    InvalidTransition {
        /// State from which the invalid operation was requested.
        state: LifecycleState,
        /// Name of the requested operation.
        operation: &'static str,
    },
}

impl std::fmt::Display for PlatformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTransition { state, operation } => {
                write!(f, "cannot {operation} while platform is {state:?}")
            }
        }
    }
}

impl std::error::Error for PlatformError {}

/// Stable, minimal data made available to platform consumers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlatformContext {
    /// Platform package version.
    pub version: &'static str,
}

/// Owns the platform lifecycle and its immutable context.
#[derive(Debug)]
pub struct Platform {
    state: LifecycleState,
    context: PlatformContext,
}

impl Default for Platform {
    fn default() -> Self {
        Self::new()
    }
}

impl Platform {
    /// Creates a platform instance in the uninitialized state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: LifecycleState::Uninitialized,
            context: PlatformContext {
                version: env!("CARGO_PKG_VERSION"),
            },
        }
    }

    /// Initializes this instance. It can only be called once.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::InvalidTransition`] unless the state is
    /// [`LifecycleState::Uninitialized`].
    pub fn initialize(&mut self) -> Result<(), PlatformError> {
        if self.state != LifecycleState::Uninitialized {
            return Err(PlatformError::InvalidTransition {
                state: self.state,
                operation: "initialize",
            });
        }
        self.state = LifecycleState::Running;
        Ok(())
    }

    /// Shuts down a running instance. A stopped instance cannot be restarted.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::InvalidTransition`] unless the state is
    /// [`LifecycleState::Running`].
    pub fn shutdown(&mut self) -> Result<(), PlatformError> {
        if self.state != LifecycleState::Running {
            return Err(PlatformError::InvalidTransition {
                state: self.state,
                operation: "shut down",
            });
        }
        self.state = LifecycleState::Stopped;
        Ok(())
    }

    /// Returns the current lifecycle state.
    #[must_use]
    pub const fn state(&self) -> LifecycleState {
        self.state
    }

    /// Returns the immutable platform context.
    #[must_use]
    pub const fn context(&self) -> &PlatformContext {
        &self.context
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_queue_drains_in_fifo_order() {
        let mut events = EventQueue::new();
        events.push(PlatformEvent::WorkspaceOpened {
            root: PathBuf::from("/workspace"),
        });
        events.push(PlatformEvent::DocumentChanged {
            document_id: 3,
            revision: 4,
        });
        assert_eq!(
            events.drain(),
            vec![
                PlatformEvent::WorkspaceOpened {
                    root: PathBuf::from("/workspace")
                },
                PlatformEvent::DocumentChanged {
                    document_id: 3,
                    revision: 4
                }
            ]
        );
        assert_eq!(events.drain(), Vec::<PlatformEvent>::new());
    }

    #[test]
    fn lifecycle_transitions_are_explicit_and_terminal() {
        let mut platform = Platform::new();
        assert_eq!(platform.state(), LifecycleState::Uninitialized);
        assert!(platform.shutdown().is_err());
        platform.initialize().unwrap();
        assert_eq!(platform.state(), LifecycleState::Running);
        assert!(platform.initialize().is_err());
        platform.shutdown().unwrap();
        assert_eq!(platform.state(), LifecycleState::Stopped);
        assert!(platform.initialize().is_err());
        assert!(platform.shutdown().is_err());
    }

    #[test]
    fn context_reports_package_version() {
        assert_eq!(Platform::new().context().version, env!("CARGO_PKG_VERSION"));
    }
}
