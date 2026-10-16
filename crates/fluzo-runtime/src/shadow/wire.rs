//! Versioned `/v1/systemone` wire contract (PRD 10.1).
//!
//! The schema is owned here and versioned explicitly: Laya is not a
//! Chat Completions provider and its public wire format is only known
//! from probes, so a corrected upstream schema must land as a
//! localized, versioned change in this file rather than leaking into
//! the coordinator, the adapter or any consumer type.
//!
//! Raw option probabilities, provider confidence and
//! `action.act_probability` are kept as three distinct fields. None
//! of them is an authorization signal (PRD 10.1); they are inert
//! data inside [`ShadowAnswer`].

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// Schema version emitted in every request and accepted in every
/// response. A response carrying any other version is rejected.
pub const SYSTEMONE_SCHEMA_VERSION: u32 = 1;

/// Inclusive bounds enforced on questions before dispatch.
pub const MIN_CHOICE_OPTIONS: usize = 2;
pub const MAX_CHOICE_OPTIONS: usize = 8;
pub const MIN_SCORE_CANDIDATES: usize = 1;
pub const MAX_SCORE_CANDIDATES: usize = 8;
pub const MAX_PROMPT_BYTES: usize = 8192;
pub const MAX_OPTION_BYTES: usize = 256;

/// Typed question accepted by the shadow contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShadowQuestion {
    /// Pick one of a small predefined set of next actions.
    Choice {
        prompt: String,
        options: Vec<String>,
    },
    /// Yes/no decision over a stated prohibition or permission.
    Noul { prompt: String },
    /// Rank a candidate set independently.
    Score {
        prompt: String,
        candidates: Vec<String>,
    },
}

impl ShadowQuestion {
    pub fn type_tag(&self) -> &'static str {
        match self {
            Self::Choice { .. } => "choice",
            Self::Noul { .. } => "noul",
            Self::Score { .. } => "score",
        }
    }

    /// Candidate action labels for `choice` and `score`; empty for
    /// `noul`.
    pub fn options(&self) -> &[String] {
        match self {
            Self::Choice { options, .. }
            | Self::Score {
                candidates: options,
                ..
            } => options,
            Self::Noul { .. } => &[],
        }
    }

    /// Validate the question before any request is built. A malformed
    /// question never reaches the transport.
    pub fn validate(&self) -> Result<(), QuestionInvalid> {
        let (prompt, options, min, max) = match self {
            Self::Choice { prompt, options } => {
                (prompt, options, MIN_CHOICE_OPTIONS, MAX_CHOICE_OPTIONS)
            }
            Self::Noul { prompt } => (prompt, &Vec::new(), 0, 0),
            Self::Score { prompt, candidates } => (
                prompt,
                candidates,
                MIN_SCORE_CANDIDATES,
                MAX_SCORE_CANDIDATES,
            ),
        };
        if prompt.trim().is_empty() {
            return Err(QuestionInvalid::EmptyPrompt);
        }
        if prompt.len() > MAX_PROMPT_BYTES {
            return Err(QuestionInvalid::PromptTooLarge);
        }
        if !options.is_empty() && (options.len() < min || options.len() > max) {
            return Err(QuestionInvalid::OptionCount {
                count: options.len(),
                min,
                max,
            });
        }
        let mut seen = std::collections::BTreeSet::new();
        for option in options {
            if option.trim().is_empty() {
                return Err(QuestionInvalid::EmptyOption);
            }
            if option.len() > MAX_OPTION_BYTES {
                return Err(QuestionInvalid::OptionTooLarge);
            }
            if !seen.insert(option) {
                return Err(QuestionInvalid::DuplicateOption);
            }
        }
        Ok(())
    }

    fn to_json(&self) -> Value {
        match self {
            Self::Choice { prompt, options } => json!({
                "type": "choice",
                "prompt": prompt,
                "options": options,
            }),
            Self::Noul { prompt } => json!({ "type": "noul", "prompt": prompt }),
            Self::Score { prompt, candidates } => json!({
                "type": "score",
                "prompt": prompt,
                "options": candidates,
            }),
        }
    }
}

/// Why a question is not dispatch-eligible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuestionInvalid {
    EmptyPrompt,
    PromptTooLarge,
    OptionCount {
        count: usize,
        min: usize,
        max: usize,
    },
    EmptyOption,
    OptionTooLarge,
    DuplicateOption,
}

impl QuestionInvalid {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::EmptyPrompt => "question_empty_prompt",
            Self::PromptTooLarge => "question_prompt_too_large",
            Self::OptionCount { .. } => "question_option_count",
            Self::EmptyOption => "question_empty_option",
            Self::OptionTooLarge => "question_option_too_large",
            Self::DuplicateOption => "question_duplicate_option",
        }
    }
}

/// Provider-side action recommendation. Distinct from the answer and
/// from confidence; inert data with no authority (PRD 10.1).
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderAction {
    pub recommended: String,
    pub act_probability: f64,
}

/// Validated typed answer. Construction is private: only
/// [`parse_answer`] produces one, so an instance is proof the
/// response passed every schema rule.
#[derive(Clone, Debug, PartialEq)]
pub enum ShadowAnswer {
    Choice {
        answer: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
        action: ProviderAction,
    },
    Noul {
        answer: bool,
        confidence: f64,
        action: ProviderAction,
    },
    Score {
        scores: BTreeMap<String, f64>,
        confidence: f64,
        action: ProviderAction,
    },
}

/// Typed rejection of a provider response. Every variant is a
/// protocol failure (counts toward the observer's pause threshold)
/// and none of them is retry-eligible.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnswerRejection {
    MalformedJson,
    ResponseTooLarge,
    UnsupportedSchemaVersion,
    UnknownAnswerType,
    MismatchedAnswerType,
    MissingField(&'static str),
    InvalidAnswer,
    InvalidProbability,
    ChoiceOutOfSet,
    InvalidConfidence,
    InvalidActProbability,
}

impl AnswerRejection {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::MalformedJson => "answer_malformed_json",
            Self::ResponseTooLarge => "answer_response_too_large",
            Self::UnsupportedSchemaVersion => "answer_unsupported_schema",
            Self::UnknownAnswerType => "answer_unknown_type",
            Self::MismatchedAnswerType => "answer_type_mismatch",
            Self::MissingField(_) => "answer_missing_field",
            Self::InvalidAnswer => "answer_invalid",
            Self::InvalidProbability => "answer_invalid_probability",
            Self::ChoiceOutOfSet => "answer_choice_out_of_set",
            Self::InvalidConfidence => "answer_invalid_confidence",
            Self::InvalidActProbability => "answer_invalid_act_probability",
        }
    }
}

/// Build the `/v1/systemone` request body. The decision point and
/// observed state reference travel with every request so the
/// observation is bound to the state before the corresponding action
/// (PRD 10.1), never to later outcomes.
pub fn build_request_json(
    observation_id: &str,
    question: &ShadowQuestion,
    decision_point: &Value,
) -> Value {
    json!({
        "schema_version": SYSTEMONE_SCHEMA_VERSION,
        "observation_id": observation_id,
        "decision_point": decision_point,
        "question": question.to_json(),
    })
}

/// Parse and validate one response body against the question that
/// produced it. The caller enforces the byte ceiling before calling
/// and passes the already-capped bytes; `ResponseTooLarge` is also
/// re-checked here so a caller cannot forget it.
pub fn parse_answer(
    bytes: &[u8],
    question: &ShadowQuestion,
) -> Result<ShadowAnswer, AnswerRejection> {
    if bytes.len() > MAX_PROMPT_BYTES * 4 {
        return Err(AnswerRejection::ResponseTooLarge);
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| AnswerRejection::MalformedJson)?;
    let root = value.as_object().ok_or(AnswerRejection::MalformedJson)?;

    let schema = root
        .get("schema_version")
        .and_then(Value::as_u64)
        .ok_or(AnswerRejection::MissingField("schema_version"))?;
    if schema != u64::from(SYSTEMONE_SCHEMA_VERSION) {
        return Err(AnswerRejection::UnsupportedSchemaVersion);
    }

    let tag = root
        .get("type")
        .and_then(Value::as_str)
        .ok_or(AnswerRejection::MissingField("type"))?;
    if !matches!(tag, "choice" | "noul" | "score") {
        return Err(AnswerRejection::UnknownAnswerType);
    }
    if tag != question.type_tag() {
        return Err(AnswerRejection::MismatchedAnswerType);
    }

    let confidence = read_unit(root, "confidence").ok_or(AnswerRejection::InvalidConfidence)?;
    let action = parse_action(root, question)?;

    match tag {
        "choice" => {
            let answer = root
                .get("answer")
                .and_then(Value::as_str)
                .ok_or(AnswerRejection::InvalidAnswer)?
                .to_owned();
            if !question.options().contains(&answer) {
                return Err(AnswerRejection::ChoiceOutOfSet);
            }
            let probabilities =
                read_probability_map(root.get("probabilities"), question.options())?;
            Ok(ShadowAnswer::Choice {
                answer,
                probabilities,
                confidence,
                action,
            })
        }
        "noul" => {
            let answer = root
                .get("answer")
                .and_then(Value::as_bool)
                .ok_or(AnswerRejection::InvalidAnswer)?;
            Ok(ShadowAnswer::Noul {
                answer,
                confidence,
                action,
            })
        }
        "score" => {
            let scores = read_probability_map(root.get("probabilities"), question.options())?;
            // Every candidate must be scored: a partial ranking is a
            // protocol violation, not a partial result.
            if scores.len() != question.options().len() {
                return Err(AnswerRejection::InvalidProbability);
            }
            Ok(ShadowAnswer::Score {
                scores,
                confidence,
                action,
            })
        }
        _ => Err(AnswerRejection::UnknownAnswerType),
    }
}

fn parse_action(
    root: &serde_json::Map<String, Value>,
    question: &ShadowQuestion,
) -> Result<ProviderAction, AnswerRejection> {
    let action = root
        .get("action")
        .and_then(Value::as_object)
        .ok_or(AnswerRejection::MissingField("action"))?;
    let recommended = action
        .get("recommended")
        .and_then(Value::as_str)
        .ok_or(AnswerRejection::MissingField("action.recommended"))?
        .to_owned();
    if !question.options().is_empty() && !question.options().contains(&recommended) {
        return Err(AnswerRejection::ChoiceOutOfSet);
    }
    let act_probability =
        read_unit(action, "act_probability").ok_or(AnswerRejection::InvalidActProbability)?;
    Ok(ProviderAction {
        recommended,
        act_probability,
    })
}

fn read_unit(map: &serde_json::Map<String, Value>, key: &str) -> Option<f64> {
    let raw = map.get(key)?.as_f64()?;
    if raw.is_finite() && (0.0..=1.0).contains(&raw) {
        Some(raw)
    } else {
        None
    }
}

fn read_probability_map(
    field: Option<&Value>,
    options: &[String],
) -> Result<BTreeMap<String, f64>, AnswerRejection> {
    let map = field
        .and_then(Value::as_object)
        .ok_or(AnswerRejection::InvalidProbability)?;
    let mut out = BTreeMap::new();
    let mut total = 0.0f64;
    for (key, value) in map {
        if !options.iter().any(|option| option == key) {
            return Err(AnswerRejection::ChoiceOutOfSet);
        }
        let Some(raw) = value.as_f64() else {
            return Err(AnswerRejection::InvalidProbability);
        };
        if !raw.is_finite() || !(0.0..=1.0).contains(&raw) {
            return Err(AnswerRejection::InvalidProbability);
        }
        total += raw;
        out.insert(key.clone(), raw);
    }
    if out.is_empty() || total > 1.0 + 1e-9 {
        return Err(AnswerRejection::InvalidProbability);
    }
    Ok(out)
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod wire_tests;
