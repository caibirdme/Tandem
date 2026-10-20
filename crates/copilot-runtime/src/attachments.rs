//! Validated message attachments shared by local and remote sessions.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use github_copilot_sdk::Attachment;
use serde::{Deserialize, Serialize};

use crate::ProviderProtocol;

pub const MAX_ATTACHMENTS: usize = 10;
pub const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_TOTAL_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;
const MAX_NAME_BYTES: usize = 255;
const MAX_MIME_BYTES: usize = 127;
const MAX_ENCODED_BYTES: usize = MAX_ATTACHMENT_BYTES.div_ceil(3) * 4;

/// An uploaded file, independent of filesystem paths on either host.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageAttachment {
    pub name: String,
    pub mime_type: String,
    /// Standard padded base64, without a data URL prefix.
    pub data: String,
}

impl std::fmt::Debug for MessageAttachment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MessageAttachment")
            .field("name", &self.name)
            .field("mime_type", &self.mime_type)
            .field("encoded_bytes", &self.data.len())
            .finish()
    }
}

impl MessageAttachment {
    /// Encodes file bytes for transport. Call `validate_attachments` before use.
    #[must_use]
    pub fn from_bytes(name: impl Into<String>, mime_type: impl Into<String>, bytes: &[u8]) -> Self {
        Self {
            name: name.into(),
            mime_type: mime_type.into(),
            data: STANDARD.encode(bytes),
        }
    }

    pub(crate) fn to_sdk(&self) -> Attachment {
        Attachment::Blob {
            data: self.data.clone(),
            // The pinned runtime silently skips unrecognized blob MIME types.
            // Validation ensures every other format contains readable UTF-8.
            mime_type: binary_mime_type(&self.name, &self.mime_type)
                .unwrap_or("text/plain")
                .into(),
            display_name: Some(self.name.clone()),
        }
    }
}

/// Attachment failures retain structured context without including file data.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum AttachmentError {
    #[error("attach at most {limit} files to one message")]
    TooMany { limit: usize },
    #[error("attachment {index} needs a nonempty filename")]
    EmptyName { index: usize },
    #[error("attachment {index} filename must not contain paths or control characters")]
    InvalidName { index: usize },
    #[error("attachment {index} filename exceeds {limit} UTF-8 bytes")]
    NameTooLong { index: usize, limit: usize },
    #[error("attachment {index} needs a valid MIME type such as application/octet-stream")]
    InvalidMimeType { index: usize },
    #[error("attachment {index} is not valid standard base64")]
    InvalidBase64 {
        index: usize,
        #[source]
        source: base64::DecodeError,
    },
    #[error("attachment {index} exceeds the {limit} byte file limit")]
    FileTooLarge { index: usize, limit: usize },
    #[error("attachments exceed the {limit} byte combined limit")]
    TotalTooLarge { limit: usize },
    #[error(
        "attachment {index} uses an unsupported binary format; use PNG, JPEG, GIF, WebP, PDF, or UTF-8 text"
    )]
    UnsupportedFormat { index: usize },
    #[error(
        "attachment {index} image format is not supported; convert it to PNG, JPEG, GIF, or WebP"
    )]
    UnsupportedImageFormat { index: usize },
    #[error("attachment {index} is not UTF-8 text; save it as UTF-8 before attaching it")]
    InvalidTextEncoding {
        index: usize,
        #[source]
        source: std::str::Utf8Error,
    },
    #[error(
        "PDF attachments require an OpenAI Responses or Anthropic provider; select one of these protocols or attach UTF-8 text instead"
    )]
    PdfUnsupportedByProvider,
}

/// Validates names, MIME types, encoding, count, and decoded sizes.
///
/// Returns decoded bytes in attachment order for storage or other native use.
///
/// # Errors
/// Returns a typed error before accepting invalid metadata or oversized data.
pub fn validate_attachments(
    attachments: &[MessageAttachment],
) -> Result<Vec<Vec<u8>>, AttachmentError> {
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(AttachmentError::TooMany {
            limit: MAX_ATTACHMENTS,
        });
    }
    let mut decoded = Vec::with_capacity(attachments.len());
    let mut total_bytes = 0;
    for (offset, attachment) in attachments.iter().enumerate() {
        let index = offset + 1;
        validate_metadata(attachment, index)?;
        if attachment.data.len() > MAX_ENCODED_BYTES {
            return Err(AttachmentError::FileTooLarge {
                index,
                limit: MAX_ATTACHMENT_BYTES,
            });
        }
        let bytes = STANDARD
            .decode(&attachment.data)
            .map_err(|source| AttachmentError::InvalidBase64 { index, source })?;
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(AttachmentError::FileTooLarge {
                index,
                limit: MAX_ATTACHMENT_BYTES,
            });
        }
        total_bytes += bytes.len();
        if total_bytes > MAX_TOTAL_ATTACHMENT_BYTES {
            return Err(AttachmentError::TotalTooLarge {
                limit: MAX_TOTAL_ATTACHMENT_BYTES,
            });
        }
        validate_format(attachment, &bytes, index)?;
        decoded.push(bytes);
    }
    Ok(decoded)
}

/// Checks whether the selected protocol can deliver this attachment's contents.
///
/// This metadata-only check also applies when changing providers for a saved
/// conversation, so earlier attachments cannot silently disappear from context.
///
/// # Errors
/// Rejects PDF content on the pinned runtime's `OpenAI` Completions transport.
pub fn validate_attachment_protocol(
    name: &str,
    mime_type: &str,
    protocol: ProviderProtocol,
) -> Result<(), AttachmentError> {
    if protocol == ProviderProtocol::OpenaiCompletions
        && binary_mime_type(name, mime_type) == Some("application/pdf")
    {
        return Err(AttachmentError::PdfUnsupportedByProvider);
    }
    Ok(())
}

fn binary_mime_type(name: &str, mime_type: &str) -> Option<&'static str> {
    let mime = mime_type.to_ascii_lowercase();
    match mime.as_str() {
        "image/png" => Some("image/png"),
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/gif" => Some("image/gif"),
        "image/webp" => Some("image/webp"),
        "application/pdf" => Some("application/pdf"),
        "application/octet-stream" => {
            let (_, extension) = name.rsplit_once('.')?;
            match extension.to_ascii_lowercase().as_str() {
                "png" => Some("image/png"),
                "jpg" | "jpeg" => Some("image/jpeg"),
                "gif" => Some("image/gif"),
                "webp" => Some("image/webp"),
                "pdf" => Some("application/pdf"),
                _ => None,
            }
        }
        _ => None,
    }
}

fn validate_format(
    attachment: &MessageAttachment,
    bytes: &[u8],
    index: usize,
) -> Result<(), AttachmentError> {
    if binary_mime_type(&attachment.name, &attachment.mime_type).is_some() {
        return Ok(());
    }
    let mime = attachment.mime_type.to_ascii_lowercase();
    if mime.starts_with("image/") && mime != "image/svg+xml" {
        return Err(AttachmentError::UnsupportedImageFormat { index });
    }
    let text = std::str::from_utf8(bytes).map_err(|source| {
        if mime.starts_with("text/") {
            AttachmentError::InvalidTextEncoding { index, source }
        } else {
            AttachmentError::UnsupportedFormat { index }
        }
    })?;
    if text.contains('\0') {
        return Err(AttachmentError::UnsupportedFormat { index });
    }
    Ok(())
}

fn validate_metadata(attachment: &MessageAttachment, index: usize) -> Result<(), AttachmentError> {
    if attachment.name.trim().is_empty() {
        return Err(AttachmentError::EmptyName { index });
    }
    if attachment.name.len() > MAX_NAME_BYTES {
        return Err(AttachmentError::NameTooLong {
            index,
            limit: MAX_NAME_BYTES,
        });
    }
    if matches!(attachment.name.as_str(), "." | "..")
        || attachment
            .name
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err(AttachmentError::InvalidName { index });
    }
    let valid_mime = attachment.mime_type.len() <= MAX_MIME_BYTES
        && attachment
            .mime_type
            .split_once('/')
            .is_some_and(|(kind, subtype)| mime_token(kind) && mime_token(subtype));
    if !valid_mime {
        return Err(AttachmentError::InvalidMimeType { index });
    }
    Ok(())
}

fn mime_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_files_round_trip_without_paths_or_data_urls() -> Result<(), Box<dyn std::error::Error>>
    {
        let bytes = [0, 1, 128, 255];
        let files = [
            MessageAttachment::from_bytes("截图.png", "image/png", &bytes),
            MessageAttachment::from_bytes("notes.txt", "text/plain", b"hello"),
            MessageAttachment::from_bytes("empty", "application/octet-stream", b""),
        ];
        assert_eq!(
            validate_attachments(&files)?,
            [bytes.to_vec(), b"hello".to_vec(), Vec::new()]
        );
        let serialized = serde_json::to_value(&files[0])?;
        assert_eq!(serialized["mimeType"], "image/png");
        assert_eq!(serialized["data"], "AAGA/w==");
        let sdk = serde_json::to_value(files[0].to_sdk())?;
        assert_eq!(sdk["type"], "blob");
        assert_eq!(sdk["displayName"], "截图.png");
        assert_eq!(sdk["data"], serialized["data"]);
        assert!(!format!("{:?}", files[0]).contains("AAGA/w=="));
        Ok(())
    }

    #[test]
    fn invalid_metadata_and_encoding_are_rejected() {
        type Case = (
            &'static str,
            &'static str,
            &'static str,
            fn(&AttachmentError) -> bool,
        );
        let cases: [Case; 9] = [
            (" ", "text/plain", "", |error| {
                matches!(error, AttachmentError::EmptyName { .. })
            }),
            ("../file", "text/plain", "", |error| {
                matches!(error, AttachmentError::InvalidName { .. })
            }),
            ("folder\\file", "text/plain", "", |error| {
                matches!(error, AttachmentError::InvalidName { .. })
            }),
            ("file\nname", "text/plain", "", |error| {
                matches!(error, AttachmentError::InvalidName { .. })
            }),
            ("file", "text", "", |error| {
                matches!(error, AttachmentError::InvalidMimeType { .. })
            }),
            ("file", "text/plain; charset=utf-8", "", |error| {
                matches!(error, AttachmentError::InvalidMimeType { .. })
            }),
            ("file", "text/plain/more", "", |error| {
                matches!(error, AttachmentError::InvalidMimeType { .. })
            }),
            (
                "file",
                "text/plain",
                "data:text/plain;base64,YQ==",
                |error| matches!(error, AttachmentError::InvalidBase64 { .. }),
            ),
            ("file", "text/plain", "YR==", |error| {
                matches!(error, AttachmentError::InvalidBase64 { .. })
            }),
        ];
        for (name, mime_type, data, expected) in cases {
            let file = MessageAttachment {
                name: name.into(),
                mime_type: mime_type.into(),
                data: data.into(),
            };
            let Err(error) = validate_attachments(&[file]) else {
                panic!("invalid attachment was accepted");
            };
            assert!(expected(&error), "{error:?}");
        }
        assert!(matches!(
            validate_attachments(&[MessageAttachment::from_bytes(
                "x".repeat(MAX_NAME_BYTES + 1),
                "text/plain",
                b""
            )]),
            Err(AttachmentError::NameTooLong { .. })
        ));
    }

    #[test]
    fn attachment_limits_accept_exact_boundaries_and_reject_overflow() {
        let small = MessageAttachment::from_bytes("small", "application/octet-stream", b"x");
        assert!(validate_attachments(&vec![small.clone(); MAX_ATTACHMENTS]).is_ok());
        assert!(matches!(
            validate_attachments(&vec![small.clone(); MAX_ATTACHMENTS + 1]),
            Err(AttachmentError::TooMany { .. })
        ));
        let exact = MessageAttachment::from_bytes(
            "large",
            "application/octet-stream",
            &vec![b'a'; MAX_ATTACHMENT_BYTES],
        );
        assert!(validate_attachments(&[exact.clone(), exact.clone()]).is_ok());
        assert!(matches!(
            validate_attachments(&[exact.clone(), exact, small]),
            Err(AttachmentError::TotalTooLarge { .. })
        ));
        let oversized = MessageAttachment::from_bytes(
            "large",
            "application/octet-stream",
            &vec![b'a'; MAX_ATTACHMENT_BYTES + 1],
        );
        assert!(matches!(
            validate_attachments(&[oversized]),
            Err(AttachmentError::FileTooLarge { .. })
        ));
    }

    #[test]
    fn recognized_files_and_generic_utf8_reach_the_sdk_with_supported_mime_types()
    -> Result<(), Box<dyn std::error::Error>> {
        for (name, declared, expected, bytes) in [
            (
                "photo.PNG",
                "application/octet-stream",
                "image/png",
                b"\x89PNG".as_slice(),
            ),
            (
                "photo.jpg",
                "application/octet-stream",
                "image/jpeg",
                b"\xff\xd8".as_slice(),
            ),
            (
                "report.pdf",
                "application/octet-stream",
                "application/pdf",
                b"%PDF".as_slice(),
            ),
            (
                "notes.txt",
                "application/octet-stream",
                "text/plain",
                b"notes".as_slice(),
            ),
            (
                "source.rs",
                "application/octet-stream",
                "text/plain",
                b"fn main() {}".as_slice(),
            ),
            (
                "Dockerfile",
                "application/octet-stream",
                "text/plain",
                b"FROM scratch".as_slice(),
            ),
            (
                "data.json",
                "application/json",
                "text/plain",
                b"{\"key\":1}".as_slice(),
            ),
            (
                "drawing.svg",
                "image/svg+xml",
                "text/plain",
                b"<svg/>".as_slice(),
            ),
            (
                "unknown.data",
                "application/unknown",
                "text/plain",
                b"plain text".as_slice(),
            ),
        ] {
            let attachment = MessageAttachment::from_bytes(name, declared, bytes);
            assert_eq!(
                validate_attachments(std::slice::from_ref(&attachment))?,
                [bytes.to_vec()]
            );
            let sdk = serde_json::to_value(attachment.to_sdk())?;
            assert_eq!(sdk["mimeType"], expected, "{name}");
            assert_eq!(sdk["data"], attachment.data);
            assert_eq!(attachment.mime_type, declared);
        }
        Ok(())
    }

    #[test]
    fn unsupported_binary_and_non_utf8_text_fail_before_submission() {
        for (name, mime, bytes) in [
            ("archive.zip", "application/zip", b"PK\x03\x04\0".as_slice()),
            (
                "unknown.bin",
                "application/octet-stream",
                b"\xff\xfe".as_slice(),
            ),
            (
                "opaque.data",
                "application/octet-stream",
                b"abc\0def".as_slice(),
            ),
        ] {
            assert!(matches!(
                validate_attachments(&[MessageAttachment::from_bytes(name, mime, bytes)]),
                Err(AttachmentError::UnsupportedFormat { .. })
            ));
        }
        assert!(matches!(
            validate_attachments(&[MessageAttachment::from_bytes(
                "utf16.txt",
                "text/plain",
                b"\xff\xfeA\0"
            )]),
            Err(AttachmentError::InvalidTextEncoding { .. })
        ));
        for mime in ["image/bmp", "image/avif", "image/tiff", "image/heic"] {
            assert!(matches!(
                validate_attachments(&[MessageAttachment::from_bytes(
                    "image",
                    mime,
                    b"binary format"
                )]),
                Err(AttachmentError::UnsupportedImageFormat { .. })
            ));
        }
    }

    #[test]
    fn pdfs_require_a_transport_that_preserves_their_contents() -> Result<(), AttachmentError> {
        for protocol in [
            ProviderProtocol::OpenaiCompletions,
            ProviderProtocol::OpenaiResponses,
            ProviderProtocol::Anthropic,
        ] {
            for (name, mime) in [
                ("document.pdf", "application/pdf"),
                ("DOCUMENT.PDF", "application/octet-stream"),
                ("upload", "APPLICATION/PDF"),
            ] {
                let result = validate_attachment_protocol(name, mime, protocol);
                if protocol == ProviderProtocol::OpenaiCompletions {
                    assert_eq!(result, Err(AttachmentError::PdfUnsupportedByProvider));
                } else {
                    result?;
                }
            }
            validate_attachment_protocol("notes.txt", "text/plain", protocol)?;
            validate_attachment_protocol("image.png", "image/png", protocol)?;
        }
        Ok(())
    }
}
