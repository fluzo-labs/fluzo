//! Pure Chat Completions assembly tests: fragmentation, usage-only
//! final chunks, unknown usage and the tool-call validation boundary
//! that makes AC1 ("validated tool calls are assembled before
//! dispatch") structural.

use fluzo_core::telemetry::ProviderUsageReport;
use serde_json::json;

use super::{ChatError, ChatStreamAssembler, FeedOutcome, build_chat_request};

fn feed_all(frames: &[serde_json::Value]) -> Result<super::AssembledResponse, ChatError> {
    let mut assembler = ChatStreamAssembler::new();
    for frame in frames {
        assembler.feed(&frame.to_string())?;
    }
    assembler.finish()
}

#[test]
fn text_deltas_accumulate_in_order() {
    let response = feed_all(&[
        json!({"id": "c1", "choices": [{"delta": {"content": "Hel"}}]}),
        json!({"choices": [{"delta": {"content": "lo"}}]}),
        json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
    ])
    .unwrap();
    assert_eq!(response.id.as_deref(), Some("c1"));
    assert_eq!(response.text, "Hello");
    assert_eq!(response.finish_reasons, vec!["stop".to_owned()]);
    assert!(response.tool_calls.is_empty());
    assert_eq!(response.usage, None, "unknown usage stays unknown");
}

#[test]
fn fragmented_tool_call_is_validated_at_finish() {
    let response = feed_all(&[
        json!({"choices": [{"delta": {"tool_calls": [{
            "index": 0, "id": "call_1",
            "function": {"name": "read_file", "arguments": "{\"path\""}
        }]}}]}),
        json!({"choices": [{"delta": {"tool_calls": [{
            "index": 0,
            "function": {"arguments": ": \"a.rs\"}"}
        }]}}]}),
        json!({"choices": [{"finish_reason": "tool_calls", "delta": {}}]}),
    ])
    .unwrap();
    assert_eq!(response.tool_calls.len(), 1);
    let call = &response.tool_calls[0];
    assert_eq!(call.id, "call_1");
    assert_eq!(call.name, "read_file");
    assert_eq!(call.arguments, "{\"path\": \"a.rs\"}");
}

#[test]
fn incomplete_tool_call_arguments_fail_typed() {
    let error = feed_all(&[
        json!({"choices": [{"delta": {"tool_calls": [{
            "index": 0, "id": "call_1",
            "function": {"name": "run", "arguments": "{\"cmd\": "}
        }]}}]}),
        json!({"choices": [{"finish_reason": "tool_calls", "delta": {}}]}),
    ])
    .unwrap_err();
    assert!(
        matches!(error, ChatError::InvalidToolCall { index: 0, .. }),
        "a truncated argument buffer is never dispatchable"
    );
}

#[test]
fn tool_call_missing_id_or_name_fails_typed() {
    let error = feed_all(&[json!({"choices": [{"delta": {"tool_calls": [{
        "index": 0,
        "function": {"arguments": "{}"}
    }]}}]})])
    .unwrap_err();
    assert_eq!(
        error,
        ChatError::InvalidToolCall {
            index: 0,
            reason: "missing id".to_owned()
        }
    );
}

#[test]
fn non_object_tool_arguments_fail_typed() {
    let error = feed_all(&[json!({"choices": [{"delta": {"tool_calls": [{
        "index": 1, "id": "c", "function": {"name": "f", "arguments": "[1,2]"}
    }]}}]})])
    .unwrap_err();
    assert!(
        matches!(error, ChatError::InvalidToolCall { index: 1, .. }),
        "arguments must be a JSON object"
    );
}

#[test]
fn empty_arguments_default_to_empty_object() {
    let response = feed_all(&[json!({"choices": [{"delta": {"tool_calls": [{
        "index": 0, "id": "c", "function": {"name": "f"}
    }]}}]})])
    .unwrap();
    assert_eq!(response.tool_calls[0].arguments, "{}");
}

#[test]
fn usage_only_final_chunk_is_honored() {
    let response = feed_all(&[
        json!({"choices": [{"delta": {"content": "hi"}}]}),
        json!({"usage": {
            "prompt_tokens": 120,
            "completion_tokens": 34,
            "prompt_tokens_details": {"cached_tokens": 40},
            "completion_tokens_details": {"reasoning_tokens": 6}
        }}),
    ])
    .unwrap();
    let usage = response.usage.expect("usage-only chunk is known usage");
    assert_eq!(usage.input_tokens, Some(120));
    assert_eq!(usage.output_tokens, Some(34));
    assert_eq!(usage.cache_read_input_tokens, Some(40));
    assert_eq!(usage.reasoning_output_tokens, Some(6));
}

#[test]
fn usage_outcome_carries_the_raw_report_for_the_normalizer() {
    let mut assembler = ChatStreamAssembler::new();
    let outcomes = assembler
        .feed(&json!({"usage": {"prompt_tokens": 5, "completion_tokens": 2}}).to_string())
        .unwrap();
    assert_eq!(
        outcomes,
        vec![FeedOutcome::Usage(ProviderUsageReport {
            input_tokens: Some(5),
            output_tokens: Some(2),
            cache_read_input_tokens: None,
            cache_write_input_tokens: None,
            reasoning_output_tokens: None,
            cumulative: false,
        })]
    );
}

#[test]
fn partial_usage_is_visible_before_finish() {
    let mut assembler = ChatStreamAssembler::new();
    assembler
        .feed(&json!({"usage": {"prompt_tokens": 9}}).to_string())
        .unwrap();
    let usage = assembler.usage_so_far().unwrap();
    assert_eq!(usage.input_tokens, Some(9));
    assert_eq!(usage.output_tokens, None, "unknown fields are not zeroed");
}

#[test]
fn malformed_frame_is_typed() {
    let mut assembler = ChatStreamAssembler::new();
    assert_eq!(
        assembler.feed("not json").err(),
        Some(ChatError::MalformedFrame)
    );
}

#[test]
fn frame_without_recognized_content_is_ignored() {
    let mut assembler = ChatStreamAssembler::new();
    let outcomes = assembler
        .feed(&json!({"object": "chat.completion.chunk"}).to_string())
        .unwrap();
    assert_eq!(outcomes, vec![FeedOutcome::Ignored]);
}

#[test]
fn assembled_response_size_ceiling_is_enforced() {
    let mut assembler = ChatStreamAssembler::new();
    let big = "x".repeat(1024 * 1024);
    let mut fed = 0usize;
    loop {
        let frame = json!({"choices": [{"delta": {"content": big}}]});
        match assembler.feed(&frame.to_string()) {
            Ok(_) => fed += big.len(),
            Err(error) => {
                assert_eq!(error, ChatError::ResponseTooLarge);
                break;
            }
        }
    }
    assert!(fed > 0);
    assert!(fed <= super::MAX_ASSEMBLED_BYTES);
}

#[test]
fn request_builder_always_streams_with_usage_and_max_tokens() {
    let body = build_chat_request(
        "coder",
        vec![json!({"role": "user", "content": "hi"})],
        None,
        4096,
    );
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["stream_options"]["include_usage"], json!(true));
    assert_eq!(body["max_tokens"], json!(4096));
    assert_eq!(body["model"], json!("coder"));
    assert!(
        body.get("tools").is_none(),
        "tools are only present when supplied"
    );

    let with_tools = build_chat_request(
        "coder",
        vec![json!({"role": "user", "content": "hi"})],
        Some(vec![json!({"type": "function"})]),
        512,
    );
    assert_eq!(with_tools["tools"].as_array().unwrap().len(), 1);
}
