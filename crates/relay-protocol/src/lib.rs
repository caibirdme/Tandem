//! The newline-delimited wire contract shared by SSH clients and stdio servers.
//!
//! Payloads may contain credentials. Parsing failures describe the contract and
//! never include input bytes; these envelopes deliberately do not implement Debug.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// Framing failures are independent of SSH, the desktop, and Agent execution.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("Invalid RPC frame: {0}")]
    InvalidFrame(String),
    #[error("RPC I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Maximum JSON bytes in a frame, excluding its terminating newline.
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

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
            .map_err(|_| ProtocolError::InvalidFrame("invalid RPC request envelope".into()))?;
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
        return Err(ProtocolError::InvalidFrame(
            "invalid RPC command name".into(),
        ));
    }
    Ok(())
}

/// A remote operation failed while the underlying connection remained usable.
#[derive(Serialize)]
pub struct RpcError {
    pub code: String,
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
        let frame: Value = serde_json::from_slice(bytes).map_err(|_| {
            ProtocolError::InvalidFrame("invalid JSON; stdout must contain only RPC frames".into())
        })?;
        let Value::Object(mut object) = frame else {
            return Err(ProtocolError::InvalidFrame(
                "RPC frame must be an object".into(),
            ));
        };
        if let Some(event) = object.remove("event") {
            if ["id", "value", "error"]
                .iter()
                .any(|key| object.contains_key(*key))
            {
                return Err(ProtocolError::InvalidFrame(
                    "event frame cannot also be a response".into(),
                ));
            }
            return Ok(Self::Event { event });
        }
        let id = object
            .remove("id")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| {
                ProtocolError::InvalidFrame("response is missing a numeric request ID".into())
            })?;
        match (object.remove("value"), object.remove("error")) {
            (Some(value), None) => Ok(Self::Response { id, value }),
            (None, Some(error)) => match (
                error.get("code").and_then(Value::as_str),
                error.get("message").and_then(Value::as_str),
            ) {
                (Some(code), Some(message)) => Ok(Self::Failure {
                    id,
                    error: RpcError {
                        code: code.into(),
                        message: message.into(),
                    },
                }),
                _ => Err(ProtocolError::InvalidFrame(
                    "error response is missing code or message".into(),
                )),
            },
            _ => Err(ProtocolError::InvalidFrame(
                "response must contain either value or error".into(),
            )),
        }
    }
}

/// Serialize and bound a complete frame before writing any bytes to the stream.
///
/// # Errors
/// Returns a protocol error for invalid serialization or a frame above 8 MiB.
pub fn encode_frame(frame: &impl Serialize) -> Result<Vec<u8>, ProtocolError> {
    let mut bytes = serde_json::to_vec(frame)
        .map_err(|_| ProtocolError::InvalidFrame("could not serialize RPC frame".into()))?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::InvalidFrame(
            "RPC frame exceeded the 8 MiB limit".into(),
        ));
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
                Err(ProtocolError::InvalidFrame(
                    "connection ended in the middle of an RPC frame".into(),
                ))
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let count = newline.unwrap_or(available.len());
        if frame.len().saturating_add(count) > MAX_FRAME_BYTES {
            return Err(ProtocolError::InvalidFrame(
                "RPC frame exceeded the 8 MiB limit".into(),
            ));
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
                    code: "failed".into(),
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
            json!({"id":1,"error":{"code":"failed"}}),
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
        ] {
            let Err(error) = RequestEnvelope::decode(input) else {
                panic!("invalid request was accepted");
            };
            assert!(!error.to_string().contains("private-key"));
            assert!(!error.to_string().contains("secret-command"));
        }
    }

    #[tokio::test]
    async fn bounds_encoding_and_decoding_and_rejects_partial_eof() -> Result<(), ProtocolError> {
        let exact = encode_frame(&"x".repeat(MAX_FRAME_BYTES - 2))?;
        assert_eq!(exact.len(), MAX_FRAME_BYTES + 1);
        assert!(encode_frame(&"x".repeat(MAX_FRAME_BYTES - 1)).is_err());
        assert_eq!(
            read_frame(&mut exact.as_slice())
                .await?
                .map(|value| value.len()),
            Some(MAX_FRAME_BYTES)
        );
        for mut bytes in [
            b"{\"id\":1}".as_slice(),
            vec![b'x'; MAX_FRAME_BYTES + 1].as_slice(),
        ] {
            assert!(read_frame(&mut bytes).await.is_err());
        }
        let mut stream = BufReader::new(b"null\n{}\n".as_slice());
        assert_eq!(read_frame(&mut stream).await?, Some(b"null".to_vec()));
        assert_eq!(read_frame(&mut stream).await?, Some(b"{}".to_vec()));
        assert!(read_frame(&mut stream).await?.is_none());
        Ok(())
    }
}
