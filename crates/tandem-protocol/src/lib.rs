//! The newline-delimited wire contract shared by SSH clients and stdio servers.
//!
//! Payloads may contain credentials. Parsing failures describe the contract and
//! never include input bytes; these envelopes deliberately do not implement Debug.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// Redacted JSON parse context. Serde's original message may quote private
/// input, so only its category and location cross the parsing boundary.
#[derive(Debug, thiserror::Error)]
#[error("{category:?} error at line {line}, column {column}")]
pub struct JsonDecodeError {
    pub category: serde_json::error::Category,
    pub line: usize,
    pub column: usize,
}

impl From<serde_json::Error> for JsonDecodeError {
    fn from(error: serde_json::Error) -> Self {
        Self {
            category: error.classify(),
            line: error.line(),
            column: error.column(),
        }
    }
}

/// Framing failures are independent of SSH, the desktop, and Agent execution.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("Invalid RPC request envelope")]
    InvalidRequest(#[source] JsonDecodeError),
    #[error("Invalid RPC command name; use 1 to 128 ASCII letters, digits, or underscores")]
    InvalidCommand,
    #[error("Invalid JSON; stdout must contain only RPC frames")]
    InvalidJson(#[source] JsonDecodeError),
    #[error("RPC frame must be an object")]
    ExpectedObject,
    #[error("Event frame cannot also be a response")]
    AmbiguousEvent,
    #[error("Response is missing a numeric request ID")]
    MissingRequestId,
    #[error("Error response must contain a recognized code and a message")]
    InvalidError(#[source] JsonDecodeError),
    #[error("Response must contain either value or error")]
    AmbiguousResponse,
    #[error("Could not serialize RPC frame")]
    Serialization(#[source] serde_json::Error),
    #[error("RPC frame exceeded the {limit} byte limit")]
    FrameTooLarge { limit: usize },
    #[error("Connection ended in the middle of an RPC frame")]
    TruncatedFrame,
    #[error("RPC I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Maximum JSON bytes in a frame, excluding its terminating newline.
pub const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;

/// Version of the remote daemon's connection and session ownership contract.
/// Version 4 forwards per-call token usage and skill invocation events.
pub const PROTOCOL_VERSION: u32 = 4;

/// One request; its numeric identifier correlates an eventual response.
#[derive(Deserialize, Serialize)]
pub struct RequestEnvelope {
    pub id: u64,
    pub command: String,
    #[serde(default)]
    pub payload: Value,
}

impl RequestEnvelope {
    /// Construct a request with a valid command name.
    ///
    /// # Errors
    /// Returns a protocol error for an empty, oversized, or non-identifier command.
    pub fn new(id: u64, command: &str, payload: Value) -> Result<Self, ProtocolError> {
        validate_command(command)?;
        Ok(Self {
            id,
            command: command.into(),
            payload,
        })
    }

    /// Parse a request without reflecting its potentially sensitive payload.
    ///
    /// # Errors
    /// Returns a protocol error for invalid JSON, missing fields, or an invalid command.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let request: Self = serde_json::from_slice(bytes)
            .map_err(|error| ProtocolError::InvalidRequest(error.into()))?;
        validate_command(&request.command)?;
        Ok(request)
    }
}

fn validate_command(command: &str) -> Result<(), ProtocolError> {
    if command.is_empty()
        || command.len() > 128
        || !command
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(ProtocolError::InvalidCommand);
    }
    Ok(())
}

/// Stable operation categories shared by servers and clients.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RpcErrorCode {
    UnsupportedCommand,
    InvalidPayload,
    SessionNotConnected,
    RuntimeUnavailable,
    RuntimeFailure,
    RuntimeBusy,
    RuntimeTimeout,
    ApprovalNotFound,
    StateDirectoryUnavailable,
    ReviewUnavailable,
    ReviewNotRepository,
    ReviewGitUnavailable,
    ReviewNotFound,
    ReviewExpired,
    ReviewInvalidInput,
    ReviewLimit,
    ReviewTimeout,
    ReviewFailure,
    Internal,
}

/// A remote operation failed while the underlying connection remained usable.
#[derive(Debug, Deserialize, Serialize, thiserror::Error)]
#[error("Remote server error ({code:?}): {message}")]
#[serde(deny_unknown_fields)]
pub struct RpcError {
    pub code: RpcErrorCode,
    pub message: String,
}

/// Responses and unsolicited events share the same ordered output stream.
#[derive(Serialize)]
#[serde(untagged)]
pub enum ServerFrame {
    Response { id: u64, value: Value },
    Failure { id: u64, error: RpcError },
    Event { event: Value },
}

impl ServerFrame {
    /// Parse an unambiguous response or event, preserving explicit null results.
    ///
    /// # Errors
    /// Returns a protocol error for malformed, ambiguous, or incomplete envelopes.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let frame: Value = serde_json::from_slice(bytes)
            .map_err(|error| ProtocolError::InvalidJson(error.into()))?;
        let Value::Object(mut object) = frame else {
            return Err(ProtocolError::ExpectedObject);
        };
        if let Some(event) = object.remove("event") {
            if ["id", "value", "error"]
                .iter()
                .any(|key| object.contains_key(*key))
            {
                return Err(ProtocolError::AmbiguousEvent);
            }
            return Ok(Self::Event { event });
        }
        let id = object
            .remove("id")
            .and_then(|value| value.as_u64())
            .ok_or(ProtocolError::MissingRequestId)?;
        match (object.remove("value"), object.remove("error")) {
            (Some(value), None) => Ok(Self::Response { id, value }),
            (None, Some(error)) => Ok(Self::Failure {
                id,
                error: serde_json::from_value(error)
                    .map_err(|error| ProtocolError::InvalidError(error.into()))?,
            }),
            _ => Err(ProtocolError::AmbiguousResponse),
        }
    }
}

/// Serialize and bound a complete frame before writing any bytes to the stream.
///
/// # Errors
/// Returns a protocol error for invalid serialization or a frame above 8 MiB.
pub fn encode_frame(frame: &impl Serialize) -> Result<Vec<u8>, ProtocolError> {
    let mut bytes = serde_json::to_vec(frame).map_err(ProtocolError::Serialization)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge {
            limit: MAX_FRAME_BYTES,
        });
    }
    bytes.push(b'\n');
    Ok(bytes)
}

/// Read one bounded, newline-terminated frame; EOF is valid only between frames.
///
/// # Errors
/// Returns an I/O error, or a protocol error for truncated or oversized frames.
pub async fn read_frame(
    reader: &mut (impl AsyncBufRead + Unpin),
) -> Result<Option<Vec<u8>>, ProtocolError> {
    let mut frame = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err(ProtocolError::TruncatedFrame)
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let count = newline.unwrap_or(available.len());
        if frame.len().saturating_add(count) > MAX_FRAME_BYTES {
            return Err(ProtocolError::FrameTooLarge {
                limit: MAX_FRAME_BYTES,
            });
        }
        frame.extend_from_slice(&available[..count]);
        reader.consume(count + usize::from(newline.is_some()));
        if newline.is_some() {
            return Ok(Some(frame));
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::io::BufReader;

    use super::*;

    #[test]
    fn preserves_null_results_and_rejects_ambiguous_envelopes() -> Result<(), ProtocolError> {
        for frame in [
            ServerFrame::Response {
                id: 1,
                value: Value::Null,
            },
            ServerFrame::Failure {
                id: 2,
                error: RpcError {
                    code: RpcErrorCode::RuntimeFailure,
                    message: "retry".into(),
                },
            },
            ServerFrame::Event {
                event: json!({"kind":"delta"}),
            },
        ] {
            let encoded = encode_frame(&frame)?;
            assert_eq!(encode_frame(&ServerFrame::decode(&encoded)?)?, encoded);
        }
        for invalid in [
            json!([]),
            json!({"value":true}),
            json!({"id":-1,"value":null}),
            json!({"id":1,"event":{}}),
            json!({"event":{},"error":{}}),
            json!({"id":1,"value":1,"error":{}}),
            json!({"id":1,"error":{"code":"runtime_failure"}}),
            json!({"id":1,"error":{"code":"unrecognized","message":"retry"}}),
            json!({"id":1,"error":{"code":"runtime_failure","message":"retry","private":"key"}}),
        ] {
            assert!(ServerFrame::decode(&encode_frame(&invalid)?).is_err());
        }
        Ok(())
    }

    #[test]
    fn invalid_request_errors_do_not_echo_payloads() {
        for input in [
            br#"{"id":1,"command":"secret-command!","payload":"private-key"}"#.as_slice(),
            br#"{"id":1,"payload":"private-key"}"#,
            br#"{"id":1,"command":"send","payload":"private-key""#,
            br#"{"id":"private-key","command":"send"}"#,
            br#"{"id":1,"command":"secret-command!"}"#,
        ] {
            let Err(error) = RequestEnvelope::decode(input) else {
                panic!("invalid request was accepted");
            };
            assert!(!error.to_string().contains("private-key"));
            assert!(!error.to_string().contains("secret-command"));
            assert!(!format!("{error:?}").contains("private-key"));
            assert!(!format!("{error:?}").contains("secret-command"));
        }
    }

    #[test]
    fn malformed_responses_expose_typed_reasons_and_redacted_sources() {
        for invalid in ["", "a-b", "x y"] {
            assert!(matches!(
                RequestEnvelope::new(1, invalid, Value::Null),
                Err(ProtocolError::InvalidCommand)
            ));
        }
        assert!(matches!(
            ServerFrame::decode(b"[]"),
            Err(ProtocolError::ExpectedObject)
        ));
        assert!(matches!(
            ServerFrame::decode(br#"{"value":true}"#),
            Err(ProtocolError::MissingRequestId)
        ));
        assert!(matches!(
            ServerFrame::decode(br#"{"event":{},"id":1}"#),
            Err(ProtocolError::AmbiguousEvent)
        ));
        assert!(matches!(
            ServerFrame::decode(br#"{"id":1,"value":null,"error":{}}"#),
            Err(ProtocolError::AmbiguousResponse)
        ));
        let Err(error) = ServerFrame::decode(
            br#"{"id":1,"error":{"code":"private-key","message":"private-key"}}"#,
        ) else {
            panic!("unknown error code was accepted");
        };
        assert!(matches!(&error, ProtocolError::InvalidError(_)));
        assert!(!format!("{error:?}").contains("private-key"));
        let source = std::error::Error::source(&error)
            .and_then(|source| source.downcast_ref::<JsonDecodeError>());
        assert!(source.is_some_and(|source| source.category == serde_json::error::Category::Data));
    }

    #[tokio::test]
    async fn bounds_encoding_and_decoding_and_rejects_partial_eof() -> Result<(), ProtocolError> {
        let exact = encode_frame(&"x".repeat(MAX_FRAME_BYTES - 2))?;
        assert_eq!(exact.len(), MAX_FRAME_BYTES + 1);
        assert!(matches!(
            encode_frame(&"x".repeat(MAX_FRAME_BYTES - 1)),
            Err(ProtocolError::FrameTooLarge {
                limit: MAX_FRAME_BYTES
            })
        ));
        assert_eq!(
            read_frame(&mut exact.as_slice())
                .await?
                .map(|value| value.len()),
            Some(MAX_FRAME_BYTES)
        );
        assert!(matches!(
            read_frame(&mut b"{\"id\":1}".as_slice()).await,
            Err(ProtocolError::TruncatedFrame)
        ));
        assert!(matches!(
            read_frame(&mut vec![b'x'; MAX_FRAME_BYTES + 1].as_slice()).await,
            Err(ProtocolError::FrameTooLarge {
                limit: MAX_FRAME_BYTES
            })
        ));
        let mut stream = BufReader::new(b"null\n{}\n".as_slice());
        assert_eq!(read_frame(&mut stream).await?, Some(b"null".to_vec()));
        assert_eq!(read_frame(&mut stream).await?, Some(b"{}".to_vec()));
        assert!(read_frame(&mut stream).await?.is_none());
        Ok(())
    }
}
