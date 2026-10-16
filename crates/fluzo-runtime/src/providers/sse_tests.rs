//! Pure SSE framing tests: arbitrary byte-boundary splits of the same
//! logical stream must produce the same events, and hostile input must
//! fail typed instead of growing the buffer or stalling.

use super::{SseError, SseEvent, SseParser};

const CEILING: usize = 4096;

fn parse_all(chunks: &[&[u8]]) -> Result<Vec<SseEvent>, SseError> {
    let mut parser = SseParser::new(CEILING);
    let mut events = Vec::new();
    for chunk in chunks {
        events.extend(parser.push(chunk)?);
    }
    events.extend(parser.finish()?);
    Ok(events)
}

#[test]
fn single_event_in_one_chunk() {
    let events = parse_all(&[b"data: hello\n\n"]).unwrap();
    assert_eq!(events, vec![SseEvent::Data("hello".to_owned())]);
}

#[test]
fn byte_at_a_time_split_still_yields_one_event() {
    let stream = b"data: fragmented payload\n\n";
    let chunks: Vec<&[u8]> = stream.chunks(1).collect();
    let events = parse_all(&chunks).unwrap();
    assert_eq!(
        events,
        vec![SseEvent::Data("fragmented payload".to_owned())]
    );
}

#[test]
fn crlf_and_lone_cr_terminators_mixed() {
    let events = parse_all(&[b"data: a\r\ndata: b\r\r\ndata: c\n\n"]).unwrap();
    assert_eq!(
        events,
        vec![
            SseEvent::Data("a\nb".to_owned()),
            SseEvent::Data("c".to_owned())
        ]
    );
}

#[test]
fn cr_lf_pair_split_across_chunks_is_one_terminator() {
    // A trailing CR mid-stream is ambiguous: it may be the first byte
    // of a CRLF that arrives in the next chunk.
    let events = parse_all(&[b"data: x\r", b"\n\r\n"]).unwrap();
    assert_eq!(events, vec![SseEvent::Data("x".to_owned())]);
}

#[test]
fn multi_line_data_joined_with_newline() {
    let events = parse_all(&[b"data: one\ndata: two\n\n"]).unwrap();
    assert_eq!(events, vec![SseEvent::Data("one\ntwo".to_owned())]);
}

#[test]
fn leading_space_after_colon_is_stripped_once() {
    let events = parse_all(&[b"data:  spaced\n\n"]).unwrap();
    assert_eq!(events, vec![SseEvent::Data(" spaced".to_owned())]);
}

#[test]
fn comment_lines_are_ignored() {
    let events = parse_all(&[b": keep-alive\ndata: real\n\n"]).unwrap();
    assert_eq!(events, vec![SseEvent::Data("real".to_owned())]);
}

#[test]
fn non_data_fields_are_ignored_not_errors() {
    let events = parse_all(&[b"event: message\nid: 7\nretry: 100\ndata: kept\n\n"]).unwrap();
    assert_eq!(events, vec![SseEvent::Data("kept".to_owned())]);
}

#[test]
fn done_sentinel_terminates_the_stream() {
    let events = parse_all(&[b"data: a\n\ndata: [DONE]\n\ndata: after\n\n"]).unwrap();
    assert_eq!(events, vec![SseEvent::Data("a".to_owned()), SseEvent::Done]);
}

#[test]
fn traffic_after_done_is_not_dispatched() {
    let mut parser = SseParser::new(CEILING);
    parser.push(b"data: [DONE]\n\n").unwrap();
    assert!(parser.is_done());
    assert!(parser.push(b"data: more\n\n").unwrap().is_empty());
}

#[test]
fn buffer_ceiling_is_enforced_typed() {
    let mut parser = SseParser::new(16);
    let result = parser.push(b"data: this payload is definitely longer than sixteen bytes\n\n");
    assert_eq!(result.err(), Some(SseError::BufferExceeded));
    // The ceiling is never raised implicitly: a second push after the
    // cleared buffer still cannot exceed it with one oversized chunk.
    let result = parser.push(&[b'x'; 32]);
    assert_eq!(result.err(), Some(SseError::BufferExceeded));
}

#[test]
fn ceiling_counts_pending_bytes_across_chunks() {
    let mut parser = SseParser::new(20);
    // 15 pending bytes fit.
    parser.push(b"data: aaaaaaaaaaa\n").unwrap();
    // 15 more would exceed 20 while still unframed.
    assert_eq!(
        parser.push(b"data: bbbbbbbbbbb\n").err(),
        Some(SseError::BufferExceeded)
    );
}

#[test]
fn invalid_utf8_is_typed() {
    let result = parse_all(&[b"data: \xff\xfe\n\n"]);
    assert_eq!(result.err(), Some(SseError::InvalidUtf8));
}

#[test]
fn multibyte_utf8_split_across_chunks_survives() {
    let payload = "data: ñandú é\n\n";
    let bytes = payload.as_bytes();
    let chunks: Vec<&[u8]> = bytes.chunks(3).collect();
    let events = parse_all(&chunks).unwrap();
    assert_eq!(events, vec![SseEvent::Data("ñandú é".to_owned())]);
}

#[test]
fn unterminated_trailing_event_is_not_dispatched() {
    // SSE dispatch requires the blank-line terminator; a stream that
    // ends mid-event yields nothing rather than a partial event.
    let events = parse_all(&[b"data: complete\n\ndata: trailing"]).unwrap();
    assert_eq!(events, vec![SseEvent::Data("complete".to_owned())]);
}

#[test]
fn empty_payload_produces_no_event() {
    let events = parse_all(&[b"\n\n", b"\n\n"]).unwrap();
    assert!(events.is_empty());
}
