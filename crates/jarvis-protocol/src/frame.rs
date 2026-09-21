use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest accepted control frame, including its length prefix.
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

const LENGTH_PREFIX_BYTES: usize = 4;
const MIN_PAYLOAD_BYTES: usize = 2;

/// Stable frame-level failures for the local control protocol.
#[derive(Debug, Error)]
pub enum FrameError {
    /// The peer declared a frame larger than the protocol allows.
    #[error("declared frame size {size} exceeds the {maximum}-byte limit")]
    TooLarge {
        /// Declared size in bytes.
        size: usize,
        /// Enforced maximum.
        maximum: usize,
    },
    /// The frame carried no JSON payload.
    #[error("frame payload is empty")]
    Empty,
    /// The length prefix did not match the bytes actually delivered.
    #[error("frame length prefix does not match the received payload")]
    LengthMismatch,
    /// The payload was not valid JSON for the expected type.
    #[error("frame payload is not a valid protocol message")]
    Malformed,
    /// The stream ended in the middle of a frame.
    #[error("stream ended before the frame was complete")]
    Truncated,
    /// The underlying transport failed.
    #[error("transport failure while {operation} a protocol frame")]
    Io {
        /// Bounded operation name.
        operation: &'static str,
        /// The transport error.
        #[source]
        source: std::io::Error,
    },
}

/// Encodes a message as a length-prefixed JSON frame.
///
/// # Errors
///
/// Returns [`FrameError::TooLarge`] when the encoded payload exceeds the
/// protocol limit, or [`FrameError::Malformed`] when serialization fails.
pub fn encode_frame<T: serde::Serialize>(message: &T) -> Result<Vec<u8>, FrameError> {
    let payload = serde_json::to_vec(message).map_err(|_| FrameError::Malformed)?;
    let total = payload.len() + LENGTH_PREFIX_BYTES;
    if total > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge {
            size: total,
            maximum: MAX_FRAME_BYTES,
        });
    }
    let size = u32::try_from(payload.len()).map_err(|_| FrameError::TooLarge {
        size: payload.len(),
        maximum: MAX_FRAME_BYTES,
    })?;
    let mut encoded = Vec::with_capacity(total);
    encoded.extend_from_slice(&size.to_be_bytes());
    encoded.extend_from_slice(&payload);
    Ok(encoded)
}

/// Decodes a complete length-prefixed JSON frame.
///
/// # Errors
///
/// Returns [`FrameError`] for a truncated prefix, an oversized or mismatched
/// length prefix, an empty payload, or a payload that is not valid JSON.
pub fn decode_frame<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, FrameError> {
    if bytes.len() < LENGTH_PREFIX_BYTES {
        return Err(FrameError::Truncated);
    }
    let declared = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let payload = &bytes[LENGTH_PREFIX_BYTES..];
    if declared != payload.len() {
        return Err(FrameError::LengthMismatch);
    }
    if declared < MIN_PAYLOAD_BYTES {
        return Err(FrameError::Empty);
    }
    if declared + LENGTH_PREFIX_BYTES > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge {
            size: declared + LENGTH_PREFIX_BYTES,
            maximum: MAX_FRAME_BYTES,
        });
    }
    serde_json::from_slice(payload).map_err(|_| FrameError::Malformed)
}

/// Reads one frame, returning `None` on a clean close at a frame boundary.
///
/// # Errors
///
/// Returns [`FrameError`] on an oversized, truncated, or malformed frame, or on
/// a transport failure.
pub async fn read_frame<R, T>(reader: &mut R) -> Result<Option<T>, FrameError>
where
    R: AsyncRead + Unpin,
    T: serde::de::DeserializeOwned,
{
    let mut prefix = [0_u8; LENGTH_PREFIX_BYTES];
    let mut filled = 0;
    while filled < LENGTH_PREFIX_BYTES {
        let read = reader
            .read(&mut prefix[filled..])
            .await
            .map_err(|source| FrameError::Io {
                operation: "read",
                source,
            })?;
        if read == 0 {
            return if filled == 0 {
                Ok(None)
            } else {
                Err(FrameError::Truncated)
            };
        }
        filled += read;
    }

    let declared = u32::from_be_bytes(prefix) as usize;
    if declared + LENGTH_PREFIX_BYTES > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge {
            size: declared + LENGTH_PREFIX_BYTES,
            maximum: MAX_FRAME_BYTES,
        });
    }
    if declared < MIN_PAYLOAD_BYTES {
        return Err(FrameError::Empty);
    }

    let mut payload = vec![0_u8; declared];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(|source| match source.kind() {
            std::io::ErrorKind::UnexpectedEof => FrameError::Truncated,
            _ => FrameError::Io {
                operation: "read",
                source,
            },
        })?;
    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(|_| FrameError::Malformed)
}

/// Writes one encoded frame and flushes it.
///
/// # Errors
///
/// Returns [`FrameError`] when encoding or the transport write fails.
pub async fn write_frame<W, T>(writer: &mut W, message: &T) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
    T: serde::Serialize,
{
    let encoded = encode_frame(message)?;
    writer
        .write_all(&encoded)
        .await
        .map_err(|source| FrameError::Io {
            operation: "write",
            source,
        })?;
    writer.flush().await.map_err(|source| FrameError::Io {
        operation: "flush",
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_through_the_length_prefix() {
        let encoded = encode_frame(&"hello").unwrap_or_else(|error| panic!("encode: {error}"));
        let decoded: String =
            decode_frame(&encoded).unwrap_or_else(|error| panic!("decode: {error}"));
        assert_eq!(decoded, "hello");
    }

    #[test]
    fn encoding_rejects_an_oversized_payload() {
        let oversized = "x".repeat(MAX_FRAME_BYTES);
        assert!(matches!(
            encode_frame(&oversized),
            Err(FrameError::TooLarge { .. })
        ));
    }

    #[test]
    fn decoding_rejects_a_mismatched_length_prefix() {
        let mut encoded = encode_frame(&"hello").unwrap_or_else(|error| panic!("encode: {error}"));
        encoded.push(b'!');
        assert!(matches!(
            decode_frame::<String>(&encoded),
            Err(FrameError::LengthMismatch)
        ));
    }

    #[test]
    fn decoding_rejects_malformed_empty_and_truncated_payloads() {
        let mut malformed = 5_u32.to_be_bytes().to_vec();
        malformed.extend_from_slice(b"nope!");
        assert!(matches!(
            decode_frame::<String>(&malformed),
            Err(FrameError::Malformed)
        ));

        assert!(matches!(
            decode_frame::<String>(&[0, 0]),
            Err(FrameError::Truncated)
        ));
        assert!(matches!(
            decode_frame::<String>(&0_u32.to_be_bytes()),
            Err(FrameError::Empty)
        ));
    }

    #[tokio::test]
    async fn streaming_codec_reads_a_frame_then_reports_a_clean_close() {
        let (mut writer, mut reader) = tokio::io::duplex(1024 * 128);
        write_frame(&mut writer, &vec![1_u8, 2, 3])
            .await
            .unwrap_or_else(|error| panic!("write: {error}"));
        drop(writer);

        let first: Vec<u8> = read_frame(&mut reader)
            .await
            .unwrap_or_else(|error| panic!("read: {error}"))
            .unwrap_or_else(|| panic!("expected one frame"));
        assert_eq!(first, vec![1, 2, 3]);
        assert_eq!(
            read_frame::<_, Vec<u8>>(&mut reader)
                .await
                .unwrap_or_else(|error| panic!("read: {error}")),
            None
        );
    }

    #[tokio::test]
    async fn streaming_codec_rejects_an_oversized_prefix_before_allocating() {
        let (mut writer, mut reader) = tokio::io::duplex(64);
        let declared = u32::try_from(MAX_FRAME_BYTES + 1).unwrap_or(u32::MAX);
        writer
            .write_all(&declared.to_be_bytes())
            .await
            .unwrap_or_else(|error| panic!("write prefix: {error}"));

        assert!(matches!(
            read_frame::<_, Vec<u8>>(&mut reader).await,
            Err(FrameError::TooLarge { .. })
        ));
    }

    #[tokio::test]
    async fn streaming_codec_rejects_a_truncated_frame() {
        let (mut writer, mut reader) = tokio::io::duplex(64);
        writer
            .write_all(&2_u32.to_be_bytes())
            .await
            .unwrap_or_else(|error| panic!("write prefix: {error}"));
        drop(writer);

        assert!(matches!(
            read_frame::<_, Vec<u8>>(&mut reader).await,
            Err(FrameError::Truncated)
        ));
    }
}
