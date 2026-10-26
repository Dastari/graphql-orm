//! Bounded server-sent-event decoding shared by HTTP provider adapters.

use crate::ProviderError;

/// Bounded `text/event-stream` frame decoder shared by provider adapters.
pub(super) struct SseDecoder {
    buffer: Vec<u8>,
    maximum_event_bytes: usize,
}

impl SseDecoder {
    /// Creates a decoder that rejects any single pending event above the bound.
    pub(super) fn new(maximum_event_bytes: usize) -> Self {
        Self {
            buffer: Vec::new(),
            maximum_event_bytes,
        }
    }

    /// Appends bytes and returns every complete `data:` payload. `[DONE]` and
    /// comment-only frames yield nothing.
    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, ProviderError> {
        self.buffer.extend_from_slice(bytes);
        if self.buffer.len() > self.maximum_event_bytes {
            return Err(ProviderError::Rejected);
        }
        let mut payloads = Vec::new();
        while let Some((position, delimiter_length)) = find_sse_delimiter(&self.buffer) {
            let frame = self.buffer.drain(..position).collect::<Vec<_>>();
            self.buffer.drain(..delimiter_length);
            if let Some(payload) = decode_sse_frame(&frame)? {
                payloads.push(payload);
            }
        }
        Ok(payloads)
    }

    /// Fails unless only whitespace remains after the stream ends.
    pub(super) fn finish(&self) -> Result<(), ProviderError> {
        if self.buffer.iter().all(u8::is_ascii_whitespace) {
            Ok(())
        } else {
            Err(ProviderError::Unavailable)
        }
    }
}

fn find_sse_delimiter(bytes: &[u8]) -> Option<(usize, usize)> {
    bytes
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|position| (position, 2))
        .or_else(|| {
            bytes
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| (position, 4))
        })
}

fn decode_sse_frame(frame: &[u8]) -> Result<Option<String>, ProviderError> {
    let frame = std::str::from_utf8(frame).map_err(|_| ProviderError::Rejected)?;
    let mut data = String::new();
    for line in frame.lines() {
        if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    if data.is_empty() || data == "[DONE]" {
        Ok(None)
    } else {
        Ok(Some(data))
    }
}
