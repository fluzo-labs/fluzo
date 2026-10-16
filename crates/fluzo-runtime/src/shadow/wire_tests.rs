use super::*;
use crate::shadow::wire::ShadowQuestion;

fn choice() -> ShadowQuestion {
    ShadowQuestion::Choice {
        prompt: "next action?".to_owned(),
        options: vec!["edit".to_owned(), "ask".to_owned(), "stop".to_owned()],
    }
}

fn decision_point() -> serde_json::Value {
    serde_json::json!({
        "task": "t1", "stage": "pre-edit", "state_version": 7,
        "state_fingerprint": "fp-7"
    })
}

#[test]
fn question_validation_accepts_wellformed_sets() {
    assert!(choice().validate().is_ok());
    assert!(
        ShadowQuestion::Noul {
            prompt: "is editing allowed?".to_owned()
        }
        .validate()
        .is_ok()
    );
    assert!(
        ShadowQuestion::Score {
            prompt: "rank".to_owned(),
            candidates: vec!["a".to_owned()],
        }
        .validate()
        .is_ok()
    );
}

#[test]
fn question_validation_rejects_malformed() {
    assert_eq!(
        ShadowQuestion::Choice {
            prompt: "  ".to_owned(),
            options: vec!["a".to_owned(), "b".to_owned()],
        }
        .validate(),
        Err(QuestionInvalid::EmptyPrompt)
    );
    assert_eq!(
        ShadowQuestion::Choice {
            prompt: "p".to_owned(),
            options: vec!["a".to_owned()],
        }
        .validate(),
        Err(QuestionInvalid::OptionCount {
            count: 1,
            min: 2,
            max: 8
        })
    );
    let too_many = (0..9).map(|i| format!("o{i}")).collect();
    assert_eq!(
        ShadowQuestion::Choice {
            prompt: "p".to_owned(),
            options: too_many,
        }
        .validate(),
        Err(QuestionInvalid::OptionCount {
            count: 9,
            min: 2,
            max: 8
        })
    );
    assert_eq!(
        ShadowQuestion::Choice {
            prompt: "p".to_owned(),
            options: vec!["a".to_owned(), "a".to_owned()],
        }
        .validate(),
        Err(QuestionInvalid::DuplicateOption)
    );
    assert_eq!(
        ShadowQuestion::Choice {
            prompt: "p".to_owned(),
            options: vec!["a".to_owned(), " ".to_owned()],
        }
        .validate(),
        Err(QuestionInvalid::EmptyOption)
    );
}

#[test]
fn request_json_carries_schema_and_decision_point() {
    let body = build_request_json("obs-1", &choice(), &decision_point());
    assert_eq!(
        body["schema_version"],
        serde_json::json!(SYSTEMONE_SCHEMA_VERSION)
    );
    assert_eq!(body["observation_id"], serde_json::json!("obs-1"));
    assert_eq!(
        body["decision_point"]["state_version"],
        serde_json::json!(7)
    );
    assert_eq!(
        body["decision_point"]["state_fingerprint"],
        serde_json::json!("fp-7")
    );
    assert_eq!(body["question"]["type"], serde_json::json!("choice"));
    assert_eq!(body["question"]["options"].as_array().unwrap().len(), 3);
}

fn answer_json(extra: serde_json::Value) -> Vec<u8> {
    let mut root = serde_json::json!({
        "schema_version": 1,
        "type": "choice",
        "answer": "edit",
        "probabilities": {"edit": 0.7, "ask": 0.2, "stop": 0.1},
        "confidence": 0.8,
        "action": {"recommended": "edit", "act_probability": 0.4}
    });
    if let Some(map) = extra.as_object() {
        root.as_object_mut().unwrap().extend(map.clone());
    }
    root.to_string().into_bytes()
}

#[test]
fn parses_valid_choice_answer() {
    let answer = parse_answer(&answer_json(serde_json::json!({})), &choice()).unwrap();
    let ShadowAnswer::Choice {
        answer: picked,
        probabilities,
        confidence,
        action,
    } = &answer
    else {
        panic!("expected choice answer");
    };
    assert_eq!(picked, "edit");
    assert_eq!(probabilities.len(), 3);
    assert!((confidence - 0.8).abs() < 1e-9);
    assert_eq!(action.recommended, "edit");
    assert!((action.act_probability - 0.4).abs() < 1e-9);
}

#[test]
fn rejects_unknown_or_mismatched_answer_types() {
    let unknown = serde_json::json!({"schema_version": 1, "type": "vibes"});
    assert_eq!(
        parse_answer(unknown.to_string().as_bytes(), &choice()),
        Err(AnswerRejection::UnknownAnswerType)
    );
    let mismatch = serde_json::json!({"schema_version": 1, "type": "noul"});
    assert_eq!(
        parse_answer(mismatch.to_string().as_bytes(), &choice()),
        Err(AnswerRejection::MismatchedAnswerType)
    );
}

#[test]
fn rejects_bad_schema_versions_and_malformed_json() {
    let mut bad_schema = answer_json(serde_json::json!({}));
    let text = String::from_utf8(bad_schema.clone())
        .unwrap()
        .replace("\"schema_version\":1", "\"schema_version\":2")
        .replace("\"schema_version\": 1", "\"schema_version\": 2");
    bad_schema = text.into_bytes();
    assert_eq!(
        parse_answer(&bad_schema, &choice()),
        Err(AnswerRejection::UnsupportedSchemaVersion)
    );
    assert_eq!(
        parse_answer(b"not json", &choice()),
        Err(AnswerRejection::MalformedJson)
    );
    assert_eq!(
        parse_answer(serde_json::json!([1, 2]).to_string().as_bytes(), &choice()),
        Err(AnswerRejection::MalformedJson)
    );
}

#[test]
fn rejects_out_of_set_answers_and_recommendations() {
    let out_of_set = answer_json(serde_json::json!({"answer": "delete_everything"}));
    assert_eq!(
        parse_answer(&out_of_set, &choice()),
        Err(AnswerRejection::ChoiceOutOfSet)
    );
    let bad_reco = answer_json(serde_json::json!({
        "action": {"recommended": "rm -rf", "act_probability": 0.9}
    }));
    assert_eq!(
        parse_answer(&bad_reco, &choice()),
        Err(AnswerRejection::ChoiceOutOfSet)
    );
    let bad_prob_key = answer_json(serde_json::json!({
        "probabilities": {"edit": 0.7, "ghost": 0.1}
    }));
    assert_eq!(
        parse_answer(&bad_prob_key, &choice()),
        Err(AnswerRejection::ChoiceOutOfSet)
    );
}

#[test]
fn rejects_invalid_probabilities_and_confidence() {
    let over_sum = answer_json(serde_json::json!({
        "probabilities": {"edit": 0.7, "ask": 0.5, "stop": 0.1}
    }));
    assert_eq!(
        parse_answer(&over_sum, &choice()),
        Err(AnswerRejection::InvalidProbability)
    );
    let negative = answer_json(serde_json::json!({
        "probabilities": {"edit": -0.1, "ask": 0.5, "stop": 0.4}
    }));
    assert_eq!(
        parse_answer(&negative, &choice()),
        Err(AnswerRejection::InvalidProbability)
    );
    let empty_probs = answer_json(serde_json::json!({"probabilities": {}}));
    assert_eq!(
        parse_answer(&empty_probs, &choice()),
        Err(AnswerRejection::InvalidProbability)
    );
    let bad_conf = answer_json(serde_json::json!({"confidence": 1.5}));
    assert_eq!(
        parse_answer(&bad_conf, &choice()),
        Err(AnswerRejection::InvalidConfidence)
    );
    let bad_act = answer_json(serde_json::json!({
        "action": {"recommended": "edit", "act_probability": 2.0}
    }));
    assert_eq!(
        parse_answer(&bad_act, &choice()),
        Err(AnswerRejection::InvalidActProbability)
    );
}

#[test]
fn rejects_missing_required_fields() {
    let no_action = br#"{"schema_version":1,"type":"choice","answer":"edit","probabilities":{"edit":0.5,"ask":0.2,"stop":0.1},"confidence":0.5}"#;
    assert_eq!(
        parse_answer(no_action, &choice()),
        Err(AnswerRejection::MissingField("action"))
    );
    let no_type = br#"{"schema_version":1,"confidence":0.5}"#;
    assert_eq!(
        parse_answer(no_type, &choice()),
        Err(AnswerRejection::MissingField("type"))
    );
}

#[test]
fn noul_requires_boolean_answer() {
    let noul = ShadowQuestion::Noul {
        prompt: "allowed?".to_owned(),
    };
    let good = br#"{"schema_version":1,"type":"noul","answer":true,"confidence":0.9,"action":{"recommended":"yes","act_probability":0.3}}"#;
    let answer = parse_answer(good, &noul).unwrap();
    assert!(matches!(answer, ShadowAnswer::Noul { answer: true, .. }));
    let bad = br#"{"schema_version":1,"type":"noul","answer":"yes","confidence":0.9,"action":{"recommended":"yes","act_probability":0.3}}"#;
    assert_eq!(
        parse_answer(bad, &noul),
        Err(AnswerRejection::InvalidAnswer)
    );
}

#[test]
fn score_requires_every_candidate_scored() {
    let score = ShadowQuestion::Score {
        prompt: "rank".to_owned(),
        candidates: vec!["a".to_owned(), "b".to_owned(), "c".to_owned()],
    };
    let partial = br#"{"schema_version":1,"type":"score","probabilities":{"a":0.5,"b":0.4},"confidence":0.7,"action":{"recommended":"a","act_probability":0.2}}"#;
    assert_eq!(
        parse_answer(partial, &score),
        Err(AnswerRejection::InvalidProbability)
    );
    let full = br#"{"schema_version":1,"type":"score","probabilities":{"a":0.5,"b":0.4,"c":0.1},"confidence":0.7,"action":{"recommended":"a","act_probability":0.2}}"#;
    assert!(parse_answer(full, &score).is_ok());
}

#[test]
fn oversized_response_is_rejected_before_parsing() {
    let huge = vec![b' '; MAX_PROMPT_BYTES * 4 + 1];
    assert_eq!(
        parse_answer(&huge, &choice()),
        Err(AnswerRejection::ResponseTooLarge)
    );
}
