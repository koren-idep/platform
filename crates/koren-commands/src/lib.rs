//! A deterministic registry for host and future application or extension commands.

use std::collections::BTreeMap;

/// Handler signature for a command. Arguments are currently simple strings.
pub type CommandHandler = fn(&[String]) -> Result<String, CommandError>;

/// Failure raised while registering or invoking commands.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandError {
    /// An identifier is already registered.
    DuplicateRegistration(String),
    /// No command has the requested identifier.
    UnknownCommand(String),
    /// The command rejected its arguments or operation.
    Execution(String),
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateRegistration(id) => write!(f, "command '{id}' is already registered"),
            Self::UnknownCommand(id) => write!(f, "unknown command '{id}'"),
            Self::Execution(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for CommandError {}

#[derive(Clone)]
struct Command {
    description: String,
    handler: CommandHandler,
}

/// Registry of unique commands, enumerated in identifier order.
#[derive(Default)]
pub struct CommandRegistry {
    commands: BTreeMap<String, Command>,
}

impl CommandRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a command. Identifiers must be non-empty and unique.
    ///
    /// # Errors
    ///
    /// Returns [`CommandError::DuplicateRegistration`] for a repeated id, or
    /// [`CommandError::Execution`] when the identifier is malformed.
    pub fn register(
        &mut self,
        id: impl Into<String>,
        description: impl Into<String>,
        handler: CommandHandler,
    ) -> Result<(), CommandError> {
        let id = id.into();
        if id.is_empty() || id.trim() != id {
            return Err(CommandError::Execution(
                "command identifier must be non-empty and have no surrounding whitespace".into(),
            ));
        }
        if self.commands.contains_key(&id) {
            return Err(CommandError::DuplicateRegistration(id));
        }
        self.commands.insert(
            id,
            Command {
                description: description.into(),
                handler,
            },
        );
        Ok(())
    }

    /// Returns registered command identifiers in deterministic lexical order.
    #[must_use]
    pub fn ids(&self) -> Vec<&str> {
        self.commands.keys().map(String::as_str).collect()
    }

    /// Returns a command's human-readable description, if registered.
    #[must_use]
    pub fn description(&self, id: &str) -> Option<&str> {
        self.commands
            .get(id)
            .map(|command| command.description.as_str())
    }

    /// Invokes a registered command with string arguments.
    ///
    /// # Errors
    ///
    /// Returns [`CommandError::UnknownCommand`] when no handler is registered,
    /// or the error returned by the handler.
    pub fn invoke(&self, id: &str, args: &[String]) -> Result<String, CommandError> {
        let command = self
            .commands
            .get(id)
            .ok_or_else(|| CommandError::UnknownCommand(id.into()))?;
        (command.handler)(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo(args: &[String]) -> Result<String, CommandError> {
        if args.first().is_some_and(|argument| argument == "error") {
            Err(CommandError::Execution("requested test failure".into()))
        } else {
            Ok(args.join(" "))
        }
    }

    #[test]
    fn registration_invocation_and_enumeration_are_deterministic() {
        let mut registry = CommandRegistry::new();
        registry.register("z.last", "last", echo).unwrap();
        registry.register("a.first", "first", echo).unwrap();
        assert_eq!(registry.ids(), ["a.first", "z.last"]);
        assert_eq!(registry.description("a.first"), Some("first"));
        assert_eq!(
            registry
                .invoke("z.last", &["hello".into(), "world".into()])
                .unwrap(),
            "hello world"
        );
    }

    #[test]
    fn duplicate_and_unknown_commands_are_errors() {
        let mut registry = CommandRegistry::new();
        registry.register("core.ping", "ping", echo).unwrap();
        assert_eq!(
            registry
                .register("core.ping", "duplicate", echo)
                .unwrap_err(),
            CommandError::DuplicateRegistration("core.ping".into())
        );
        assert_eq!(
            registry.invoke("missing", &[]).unwrap_err(),
            CommandError::UnknownCommand("missing".into())
        );
    }
}
