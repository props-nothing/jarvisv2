//! Server-Sent Events decoding for the streaming path.
//!
//! Only the subset the provider uses is implemented: `data:` lines carrying JSON,
//! separated by blank lines, ending with `data: [DONE]`. Comment lines and unknown
//! fields are ignored.
//!
//! The decoder works on a byte buffer rather than on `String` lines on purpose. A
//! TCP chunk boundary can fall in the middle of a multi-byte character, and decoding
//! each chunk independently would replace the split character with a replacement
//! character, corrupting the answer exactly at an arbitrary point.

/// The sentinel a provider sends instead of a final data object.
pub(super) const DONE_SENTINEL: &str = "[DONE]";

/// Maximum bytes of unprocessed buffer before the stream is rejected.
///
/// A server that never emits an event separator would otherwise grow the buffer
/// without bound.
pub(super) const MAX_PENDING_BYTES: usize = 1024 * 1024;

/// The result of pushing bytes into the decoder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum SseStep {
    /// Nothing complete yet.
    Incomplete,
    /// A complete event whose `data` payload was this text.
    Data(String),
    /// The provider sent the done sentinel.
    Done,
    /// The buffer exceeded its bound without producing an event.
    Overflow,
}

/// An incremental SSE decoder.
#[derive(Default)]
pub(super) struct SseDecoder {
    pending: Vec<u8>,
}

impl SseDecoder {
    /// Creates an empty decoder.
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Appends bytes and returns the next complete event, if any.
    ///
    /// Returns at most one event per call so the caller's loop stays simple and a
    /// single chunk containing several events is drained across iterations.
    pub(super) fn push(&mut self, chunk: &[u8]) -> SseStep {
        self.pending.extend_from_slice(chunk);

        // An event is terminated by a blank line, in either line-ending convention.
        while let Some((end, separator_len)) = find_separator(&self.pending) {
            let block: Vec<u8> = self.pending.drain(..end).collect();
            self.pending.drain(..separator_len);

            if let Some(step) = interpret_block(&block) {
                return step;
            }
            // The block carried no data (a comment or a keep-alive) so keep looking.
        }

        if self.pending.len() > MAX_PENDING_BYTES {
            return SseStep::Overflow;
        }
        SseStep::Incomplete
    }
}

/// Finds the first blank-line separator and its byte length.
fn find_separator(buffer: &[u8]) -> Option<(usize, usize)> {
    for index in 0..buffer.len() {
        // "\r\n\r\n"
        if buffer[index..].starts_with(b"\r\n\r\n") {
            return Some((index, 4));
        }
        // "\n\n"
        if buffer[index..].starts_with(b"\n\n") {
            return Some((index, 2));
        }
    }
    None
}

/// Interprets one event block, or `None` when it carries no data.
fn interpret_block(block: &[u8]) -> Option<SseStep> {
    for line in block.split(|byte| *byte == b'\n') {
        let line = strip_carriage_return(line);
        let Some(rest) = line.strip_prefix(b"data:") else {
            // `event:`, `id:`, `retry:`, and comments are not used by the adapter.
            // Ignoring them is correct because adding fields is a compatible change.
            continue;
        };
        let value = rest.strip_prefix(b" ").unwrap_or(rest);
        let text = String::from_utf8_lossy(value).into_owned();
        if text.trim() == DONE_SENTINEL {
            return Some(SseStep::Done);
        }
        return Some(SseStep::Data(text));
    }
    None
}

/// Removes a single trailing carriage return.
fn strip_carriage_return(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_multibyte_character_split_across_chunks_is_not_corrupted() {
        // This is why the decoder buffers bytes: decoding each chunk alone would
        // produce a replacement character mid-answer, at a position that depends on
        // network timing.
        let payload = "data: {\"text\":\"é\"}\n\n";
        let bytes = payload.as_bytes();
        let split = bytes
            .iter()
            .position(|byte| *byte >= 0x80)
            .unwrap_or_else(|| panic!("fixture must contain a multibyte character"));

        let mut decoder = SseDecoder::new();
        assert_eq!(decoder.push(&bytes[..split]), SseStep::Incomplete);
        let step = decoder.push(&bytes[split..]);

        let SseStep::Data(text) = step else {
            panic!("expected a data event, got {step:?}");
        };
        assert!(
            text.contains('é'),
            "the split character must survive: {text}"
        );
        assert!(
            !text.contains('\u{fffd}'),
            "no replacement character expected"
        );
    }

    #[test]
    fn one_chunk_containing_several_events_is_drained_incrementally() {
        let mut decoder = SseDecoder::new();
        let step = decoder.push(b"data: one\n\ndata: two\n\ndata: three\n\n");
        assert_eq!(step, SseStep::Data("one".to_owned()));
        assert_eq!(decoder.push(b""), SseStep::Data("two".to_owned()));
        assert_eq!(decoder.push(b""), SseStep::Data("three".to_owned()));
    }

    #[test]
    fn the_done_sentinel_is_recognized() {
        let mut decoder = SseDecoder::new();
        assert_eq!(decoder.push(b"data: [DONE]\n\n"), SseStep::Done);
    }

    #[test]
    fn comments_and_keep_alives_do_not_produce_events() {
        let mut decoder = SseDecoder::new();
        assert_eq!(
            decoder.push(b": keep-alive\n\nevent: ping\n\n"),
            SseStep::Incomplete,
            "a block with no data line must not yield an event"
        );
        assert_eq!(
            decoder.push(b"data: real\n\n"),
            SseStep::Data("real".to_owned())
        );
    }

    #[test]
    fn carriage_return_line_endings_are_accepted() {
        let mut decoder = SseDecoder::new();
        assert_eq!(
            decoder.push(b"data: crlf\r\n\r\n"),
            SseStep::Data("crlf".to_owned())
        );
    }

    #[test]
    fn a_server_that_never_separates_events_is_bounded() {
        let mut decoder = SseDecoder::new();
        let filler = vec![b'x'; 64 * 1024];
        let mut result = SseStep::Incomplete;
        for _ in 0..32 {
            result = decoder.push(&filler);
            if result == SseStep::Overflow {
                break;
            }
        }
        assert_eq!(
            result,
            SseStep::Overflow,
            "unbounded buffering must be rejected rather than growing without limit"
        );
    }
}
