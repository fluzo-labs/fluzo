//! Chat Completions request construction and streaming response
//! assembly (PRD 45.2: SSE framing, fragmented tool-call arguments,
//! finish reasons, usage metadata and stream completion).
//!
//! The assembler is pure: JSON payload strings in, typed assembly
//! events out. A tool call only becomes visible after its complete
//! argument buffer has been received and JSON-validated, which is
//! what makes "validated tool calls are assembled before dispatch"
//! (issue #20 AC1) a property of this layer rather than a caller
//! convention.

use std::collections::BTreeMap;

use fluzo_core::telemetry::{GenAiUsage, ProviderUsageReport};
use serde_json::{Value, json};

/// Hard ceiling on one assembled response body (text plus tool
/// argument bytes). Sized generously above any legitimate
/// max-output configuration; a provider exceeding it is a typed
/// failure, not unbounded memory use.
pub const MAX_ASSEMBLED_BYTES: usize = 8 * 1024 * 1024;

/// Typed assembly failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChatError {
    /// A `data:` payload was not a valid Chat Completions chunk.
    MalformedFrame,
    /// A streamed tool call finished without a parsable JSON object
    /// for its arguments (or without a name/id where required).
    InvalidToolCall { index: u32, reason: String },
    /// The assembled response exceeded the hard ceiling.
    ResponseTooLarge,
}

/// A fully assembled, dispatch-eligible tool call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssembledToolCall {
    pub id: String,
    pub name: String,
    /// Validated JSON object text.
    pub arguments: String,
}

/// What one streamed chunk contributed, for callers that render
/// deltas live.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FeedOutcome {
    TextDelta(String),
    ToolCallDelta { index: u32 },
    Usage,
    Ignored,
}

/// The completed assembly of one streamed response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssembledResponse {
    pub id: Option<String>,
    pub model: Option<String>,
    pub text: String,
    pub tool_calls: Vec<AssembledToolCall>,
    pub finish_reasons: Vec<String>,
    /// Provider-reported usage. `None` means unknown usage and is
    /// never replaced by zeros.
    pub usage: Option<GenAiUsage>,
}

#[derive(Default)]
struct ToolCallBuilder {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

/// Incremental Chat Completions stream assembler.
#[derive(Default)]
pub struct ChatStreamAssembler {
    id: Option<String>,
    model: Option<String>,
    text: String,
    calls: BTreeMap<u32, ToolCallBuilder>,
    finish_reasons: Vec<String>,
    usage: Option<GenAiUsage>,
    assembled_bytes: usize,
}

impl ChatStreamAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one SSE `data:` payload (already de-framed).
    pub fn feed(&mut self, data: &str) -> Result<Vec<FeedOutcome>, ChatError> {
        let frame: Value = serde_json::from_str(data).map_err(|_| ChatError::MalformedFrame)?;
        if let Some(id) = frame.get("id").and_then(Value::as_str) {
            if self.id.is_none() {
                self.id = Some(id.to_owned());
            }
        }
        if let Some(model) = frame.get("model").and_then(Value::as_str) {
            if self.model.is_none() {
                self.model = Some(model.to_owned());
            }
        }
        let mut outcomes = Vec::new();
        let mut touched = false;

        if let Some(choices) = frame.get("choices").and_then(Value::as_array) {
            for choice in choices {
                touched = true;
                if let Some(delta) = choice.get("delta") {
                    if let Some(content) = delta.get("content").and_then(Value::as_str) {
                        self.charge(content.len())?;
                        self.text.push_str(content);
                        outcomes.push(FeedOutcome::TextDelta(content.to_owned()));
                    }
                    if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                        for call in calls {
                            let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
                            let builder = self.calls.entry(index).or_default();
                            self.charge(1)?;
                            if let Some(id) = call.get("id").and_then(Value::as_str) {
                                builder.id = Some(id.to_owned());
                            }
                            if let Some(function) = call.get("function") {
                                if let Some(name) = function.get("name").and_then(Value::as_str) {
                                    builder.name = Some(name.to_owned());
                                }
                                if let Some(fragment) =
                                    function.get("arguments").and_then(Value::as_str)
                                {
                                    self.charge(fragment.len())?;
                                    builder.arguments.push_str(fragment);
                                }
                            }
                            outcomes.push(FeedOutcome::ToolCallDelta { index });
                        }
                    }
                }
                if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                    self.finish_reasons.push(reason.to_owned());
                }
            }
        }

        if let Some(usage) = frame.get("usage").filter(Value::is_object) {
            // A usage-only final chunk carries no choices; it is
            // honored here and never treated as a stream error or a
            // duplicate of earlier content.
            touched = true;
            let report = parse_usage(usage);
            self.usage = Some(match self.usage.take() {
                Some(previous) => merge_usage(previous, report),
                None => usage_from_report(report),
            });
            outcomes.push(FeedOutcome::Usage);
        }

        if !touched {
            outcomes.push(FeedOutcome::Ignored);
        }
        Ok(outcomes)
    }

    /// Finalize. Every tool call's argument buffer must be complete
    /// and a valid JSON object, or the whole response fails typed
    /// (a fragment is never dispatchable).
    pub fn finish(self) -> Result<AssembledResponse, ChatError> {
        let mut tool_calls = Vec::with_capacity(self.calls.len());
        for (index, builder) in self.calls {
            let id = builder.id.ok_or_else(|| ChatError::InvalidToolCall {
                index,
                reason: "missing id".to_owned(),
            })?;
            let name = builder.name.ok_or_else(|| ChatError::InvalidToolCall {
                index,
                reason: "missing function name".to_owned(),
            })?;
            let arguments = if builder.arguments.trim().is_empty() {
                "{}".to_owned()
            } else {
                builder.arguments.clone()
            };
            let parsed: Value = serde_json::from_str(&arguments).map_err(|error| {
                ChatError::InvalidToolCall {
                    index,
                    reason: format!("arguments are not valid JSON: {error}"),
                }
            })?;
            if !parsed.is_object() {
                return Err(ChatError::InvalidToolCall {
                    index,
                    reason: "arguments must be a JSON object".to_owned(),
                });
            }
            tool_calls.push(AssembledToolCall {
                id,
                name,
                arguments,
            });
        }
        Ok(AssembledResponse {
            id: self.id,
            model: self.model,
            text: self.text,
            tool_calls,
            finish_reasons: self.finish_reasons,
            usage: self.usage,
        })
    }

    /// Usage known so far (partial streams keep whatever was reported
    /// before the break).
    pub fn usage_so_far(&self) -> Option<GenAiUsage> {
        self.usage
    }

    fn charge(&mut self, bytes: usize) -> Result<(), ChatError> {
        self.assembled_bytes = self.assembled_bytes.saturating_add(bytes);
        if self.assembled_bytes > MAX_ASSEMBLED_BYTES {
            return Err(ChatError::ResponseTooLarge);
        }
        Ok(())
    }
}

fn parse_usage(usage: &Value) -> ProviderUsageReport {
    let field = |name: &str| usage.get(name).and_then(Value::as_u64);
    let details = |section: &str, name: &str| {
        usage
            .get(section)
            .and_then(Value::as_object)
            .and_then(|map| map.get(name))
            .and_then(Value::as_u64)
    };
    ProviderUsageReport {
        input_tokens: field("prompt_tokens"),
        output_tokens: field("completion_tokens"),
        cache_read_input_tokens: details("prompt_tokens_details", "cached_tokens"),
        cache_write_input_tokens: None,
        reasoning_output_tokens: details("completion_tokens_details", "reasoning_tokens"),
        // A Chat Completions usage report is the request's total,
        // delivered once; not a running cumulative restatement.
        cumulative: false,
    }
}

fn usage_from_report(report: ProviderUsageReport) -> GenAiUsage {
    GenAiUsage {
        input_tokens: report.input_tokens,
        output_tokens: report.output_tokens,
        cache_read_input_tokens: report.cache_read_input_tokens,
        cache_write_input_tokens: report.cache_write_input_tokens,
        reasoning_output_tokens: report.reasoning_output_tokens,
    }
}

fn merge_usage(previous: GenAiUsage, report: ProviderUsageReport) -> GenAiUsage {
    let incoming = usage_from_report(report);
    GenAiUsage {
        input_tokens: previous.input_tokens.or(incoming.input_tokens),
        output_tokens: previous.output_tokens.or(incoming.output_tokens),
        cache_read_input_tokens: previous.cache_read_input_tokens.or(incoming.cache_read_input_tokens),
        cache_write_input_tokens: previous.cache_write_input_tokens.or(incoming.cache_write_input_tokens),
        reasoning_output_tokens: previous
            .reasoning_output_tokens
            .or(incoming.reasoning_output_tokens),
    }
}

/// Build a Chat Completions request body. Streaming always requests
/// usage metadata so the usage-only final chunk contract applies.
pub fn build_chat_request(
    model: &str,
    messages: Vec<Value>,
    tools: Option<Vec<Value>>,
    max_output_tokens: u64,
) -> Value {
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
        "max_tokens": max_output_tokens,
    });
    if let Some(tools) = tools {
        body["tools"] = Value::Array(tools);
    }
    body
}
