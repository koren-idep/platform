//! Experimental, strongly typed internal request and response contracts.

use serde::{Deserialize, Serialize};

/// Initial experimental protocol version. Breaking changes may increment it.
pub const PROTOCOL_VERSION: u16 = 1;

/// Supported request operations in protocol version 1.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum RequestMethod {
    /// Check that the platform is responsive.
    Ping,
    /// Enumerate commands registered by the host.
    ListCommands,
    /// Invoke a registered command without arguments.
    ExecuteCommand {
        /// Identifier of the command to invoke.
        command_id: String,
    },
    /// Read information about a local directory.
    WorkspaceInfo {
        /// Local filesystem path to inspect.
        path: String,
    },
}

/// Versioned request envelope for internal consumers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Protocol schema version.
    pub version: u16,
    /// Caller-assigned request identifier.
    pub request_id: u64,
    /// Typed operation.
    pub method: RequestMethod,
}

impl Request {
    /// Creates a request using the current experimental schema version.
    #[must_use]
    pub fn new(request_id: u64, method: RequestMethod) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id,
            method,
        }
    }
}

/// Structured protocol failure categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The peer uses an unsupported schema version.
    UnsupportedVersion,
    /// The requested operation or command is not available.
    NotFound,
    /// A request is malformed or violates a contract rule.
    InvalidRequest,
    /// An operation failed because of the local environment.
    Internal,
}

/// Structured error payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProtocolError {
    /// Machine-readable category.
    pub code: ErrorCode,
    /// Human-readable explanation.
    pub message: String,
}

/// Typed successful response values.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ResponseValue {
    /// Ping response.
    Pong,
    /// Available command identifiers.
    Commands(Vec<String>),
    /// Text returned by a command.
    CommandOutput(String),
    /// Root path and top-level entry count.
    Workspace {
        /// Canonical workspace root path.
        root: String,
        /// Number of top-level entries.
        entries: usize,
    },
}

/// Versioned response envelope for internal consumers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// Protocol schema version.
    pub version: u16,
    /// Identifier copied from the request.
    pub request_id: u64,
    /// Either a typed value or a structured error.
    pub result: Result<ResponseValue, ProtocolError>,
}

impl Response {
    /// Creates a response using the current experimental schema version.
    #[must_use]
    pub fn new(request_id: u64, result: Result<ResponseValue, ProtocolError>) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id,
            result,
        }
    }
}

/// Checks that an incoming request uses this build's schema version.
///
/// # Errors
///
/// Returns [`ProtocolError`] with [`ErrorCode::UnsupportedVersion`] if the
/// supplied version differs from [`PROTOCOL_VERSION`].
pub fn validate_version(version: u16) -> Result<(), ProtocolError> {
    if version == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(ProtocolError {
            code: ErrorCode::UnsupportedVersion,
            message: format!(
                "protocol version {version} is unsupported; expected {PROTOCOL_VERSION}"
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_and_has_stable_tagged_shape() {
        let request = Request::new(
            7,
            RequestMethod::ExecuteCommand {
                command_id: "core.ping".into(),
            },
        );
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            json,
            r#"{"version":1,"request_id":7,"method":{"method":"execute_command","params":{"command_id":"core.ping"}}}"#
        );
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), request);
    }

    #[test]
    fn response_serializes_structured_errors_and_values() {
        let error = ProtocolError {
            code: ErrorCode::NotFound,
            message: "unknown command".into(),
        };
        let response = Response::new(9, Err(error));
        let decoded: Response =
            serde_json::from_str(&serde_json::to_string(&response).unwrap()).unwrap();
        assert_eq!(decoded, response);
        let value = Response::new(10, Ok(ResponseValue::Pong));
        assert_eq!(
            serde_json::from_str::<Response>(&serde_json::to_string(&value).unwrap()).unwrap(),
            value
        );
    }

    #[test]
    fn protocol_version_is_checked() {
        assert!(validate_version(PROTOCOL_VERSION).is_ok());
        assert_eq!(
            validate_version(0).unwrap_err().code,
            ErrorCode::UnsupportedVersion
        );
    }
}
