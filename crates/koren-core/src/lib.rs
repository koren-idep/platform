//! Small, frontend-independent lifecycle primitives for Koren Platform.

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
