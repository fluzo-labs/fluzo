//! Incremental Server-Sent Events framing for provider streams.
//!
//! Pure framing only: bytes in, complete events out. The parser is
//! deliberately transport-agnostic and unit-testable against arbitrary
//! chunk-boundary splits, including multi-byte UTF-8 sequences split
//! across chunks and CR/LF/CRLF terminators mixed in one stream
//! (PRD 45.2: SSE framing is part of the tested contract).
//!
//! The buffer has a hard ceiling supplied by the caller: a hostile or
//! broken provider cannot make this parser grow without bound, and an
//! incomplete escape of that ceiling is a typed error, not a stall.

/// One framed SSE event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SseEvent {
    /// Joined `data:` payload for one event (multi-line data fields
    /// are joined with `\n` per the SSE specification).
    Data(String),
    /// The `data: [DONE]` completion sentinel.
    Done,
}

/// Framing failure. Everything is typed; nothing is silently dropped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SseError {
    /// The pending buffer exceeded the configured ceiling. The stream
    /// must be treated as failed; the ceiling is never raised.
    BufferExceeded,
    /// A framed payload was not valid UTF-8.
    InvalidUtf8,
}

/// Incremental SSE framing parser.
pub struct SseParser {
    buffer: Vec<u8>,
    ceiling: usize,
    done: bool,
}

impl SseParser {
    pub fn new(ceiling_bytes: usize) -> Self {
        Self {
            buffer: Vec::new(),
            ceiling: ceiling_bytes,
            done: false,
        }
    }

    /// Feed one transport chunk; returns every event that became
    /// complete with this chunk.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, SseError> {
        if self.done {
            // Traffic after the sentinel is a protocol violation the
            // caller must see as no more events; keep it simple: the
            // parser stops accepting.
            return Ok(Vec::new());
        }
        if self.buffer.len().saturating_add(bytes.len()) > self.ceiling {
            self.buffer.clear();
            return Err(SseError::BufferExceeded);
        }
        self.buffer.extend_from_slice(bytes);
        self.drain(false)
    }

    /// End of transport. A trailing event without its blank-line
    /// terminator is NOT dispatched (SSE dispatch semantics); it is
    /// surfaced as incomplete by returning nothing and letting the
    /// caller's stream-completion check fail loudly.
    pub fn finish(&mut self) -> Result<Vec<SseEvent>, SseError> {
        // A trailing CR held pending may still terminate one line;
        // normalize it and dispatch only if a full blank-line event
        // results (i.e. the stream ended with "\r\n\r" style edge is
        // still incomplete, so nothing dispatches).
        self.drain(true)
    }

    fn drain(&mut self, final_pass: bool) -> Result<Vec<SseEvent>, SseError> {
        let mut events = Vec::new();
        loop {
            let Some(separator) = find_event_separator(&self.buffer, final_pass) else {
                return Ok(events);
            };
            let raw: Vec<u8> = self.buffer.drain(..=separator.1).collect();
            let payload = &raw[..separator.0];
            match frame_event(payload)? {
                Frame::Nothing => {}
                Frame::Done => {
                    self.done = true;
                    events.push(SseEvent::Done);
                    return Ok(events);
                }
                Frame::Data(data) => events.push(SseEvent::Data(data)),
            }
        }
    }

    /// True once the `[DONE]` sentinel was seen.
    pub fn is_done(&self) -> bool {
        self.done
    }
}

enum Frame {
    Nothing,
    Data(String),
    Done,
}

/// Find the next event separator (blank line). Returns
/// `(payload_end, separator_last_index)`: the payload is everything
/// before the blank line (including the previous line's terminator,
/// which line splitting handles), and the separator spans the blank
/// line plus its terminator. Handles LF, CRLF and CR terminators.
/// A trailing lone `\r` at the buffer end is only treated as a
/// terminator on the final pass; mid-stream it may still be the
/// first byte of a CRLF split across chunks.
fn find_event_separator(buffer: &[u8], final_pass: bool) -> Option<(usize, usize)> {
    let mut line_start = 0usize;
    while line_start <= buffer.len() {
        let (line_end, term_len) = find_line_terminator(buffer, line_start, final_pass)?;
        if line_end == line_start {
            // Empty line: this is the event separator.
            return Some((line_start, line_start + term_len - 1));
        }
        line_start = line_end + term_len;
    }
    None
}

/// Find a line terminator starting at `from`. Returns
/// `(terminator_index, terminator_length)`. A trailing lone `\r` is
/// ambiguous mid-stream (it may precede a `\n` in the next chunk)
/// and yields `None` until resolved.
fn find_line_terminator(
    buffer: &[u8],
    from: usize,
    final_pass: bool,
) -> Option<(usize, usize)> {
    let mut index = from;
    while index < buffer.len() {
        match buffer[index] {
            b'\n' => return Some((index, 1)),
            b'\r' => {
                match buffer.get(index + 1) {
                    Some(b'\n') => return Some((index, 2)),
                    Some(_) => return Some((index, 1)),
                    None => {
                        if final_pass {
                            return Some((index, 1));
                        }
                        return None;
                    }
                }
            }
            _ => index += 1,
        }
    }
    None
}

fn frame_event(payload: &[u8]) -> Result<Frame, SseError> {
    let text = std::str::from_utf8(payload).map_err(|_| SseError::InvalidUtf8)?;
    let mut data_lines: Vec<&str> = Vec::new();
    for line in split_lines(text) {
        if line.is_empty() {
            continue;
        }
        if line.starts_with(':') {
            // SSE comment line: ignored.
            continue;
        }
        let Some((field, value)) = line.split_once(':') else {
            // Field name with no colon: treated as a field with empty
            // value per spec; only `data` matters here.
            continue;
        };
        if field == "data" {
            data_lines.push(value.strip_prefix(' ').unwrap_or(value));
        }
        // Other fields (event:, id:, retry:) are not part of the
        // Chat Completions contract and are ignored, not errors.
    }
    if data_lines.is_empty() {
        return Ok(Frame::Nothing);
    }
    let joined = data_lines.join("\n");
    if joined.trim() == "[DONE]" {
        return Ok(Frame::Done);
    }
    Ok(Frame::Data(joined))
}

/// Split on any of LF, CRLF or lone CR without allocating for the
/// common LF-only case beyond the iterator.
fn split_lines(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        for (index, byte) in rest.bytes().enumerate() {
            if byte == b'\n' {
                let line = &rest[..index];
                rest = &rest[index + 1..];
                return Some(line);
            }
            if byte == b'\r' {
                let line = &rest[..index];
                if rest.as_bytes().get(index + 1) == Some(&b'\n') {
                    rest = &rest[index + 2..];
                } else {
                    rest = &rest[index + 1..];
                }
                return Some(line);
            }
        }
        let line = rest;
        rest = "";
        Some(line)
    })
}
