use super::*;
use crate::shadow::wire::{AnswerRejection, ShadowQuestion};

fn request() -> ShadowRequest {
    ShadowRequest {
        observation_id: "obs-1".to_owned(),
        decision_point: DecisionPoint {
            task: "task-a".to_owned(),
            stage: "pre-tool".to_owned(),
            state_version: 3,
            state_fingerprint: "fp".to_owned(),
        },
        question: ShadowQuestion::Choice {
            prompt: "next?".to_owned(),
            options: vec!["a".to_owned(), "b".to_owned()],
        },
        model: "laya-mini".to_owned(),
        input_estimate_tokens: 100,
        remaining_task_seconds: 60,
        state_size_bytes: 4096,
        language: Some("en".to_owned()),
        truncation: None,
    }
}

#[test]
fn base_record_carries_provenance() {
    let record = request().base_record();
    assert_eq!(record.observation_id, "obs-1");
    assert_eq!(record.question_type, "choice");
    assert_eq!(record.schema_version, wire::SYSTEMONE_SCHEMA_VERSION);
    assert_eq!(record.candidate_actions, vec!["a", "b"]);
    assert_eq!(record.state_size_bytes, 4096);
    assert_eq!(record.language.as_deref(), Some("en"));
    assert_eq!(record.model, "laya-mini");
    assert_eq!(record.answer, None);
    assert_eq!(record.actual_action, None);
}

#[test]
fn decision_point_key_is_stable_and_distinguishing() {
    let a = request().decision_point;
    let mut b = a.clone();
    assert_eq!(a.key(), b.key());
    b.state_version += 1;
    assert_ne!(a.key(), b.key());
    b.state_version = a.state_version;
    b.state_fingerprint = "other".to_owned();
    assert_ne!(a.key(), b.key());
}

#[test]
fn status_codes_are_machine_readable() {
    assert_eq!(ObservationStatus::Completed.code(), "completed");
    assert_eq!(
        ObservationStatus::Rejected {
            reason: AnswerRejection::ChoiceOutOfSet
        }
        .code(),
        "answer_choice_out_of_set"
    );
    assert_eq!(
        ObservationStatus::Timeout {
            stage: "request".to_owned()
        }
        .code(),
        "timeout_request"
    );
    assert_eq!(
        ObservationStatus::Cancelled {
            stage: "queued_dropped".to_owned()
        }
        .code(),
        "cancelled_queued_dropped"
    );
    assert_eq!(
        ObservationStatus::Skipped {
            reason: "no_shadow_allowance".to_owned()
        }
        .code(),
        "skipped_no_shadow_allowance"
    );
}

#[test]
fn only_transport_protocol_and_timeout_count_as_failures() {
    assert!(!ObservationStatus::Completed.counts_as_failure());
    assert!(
        ObservationStatus::Rejected {
            reason: AnswerRejection::MalformedJson
        }
        .counts_as_failure()
    );
    assert!(
        ObservationStatus::Timeout {
            stage: "request".to_owned()
        }
        .counts_as_failure()
    );
    assert!(
        ObservationStatus::Failed {
            code: "transport".to_owned()
        }
        .counts_as_failure()
    );
    assert!(
        ObservationStatus::Failed {
            code: "response_too_large".to_owned()
        }
        .counts_as_failure()
    );
    // Consent, admission, cancellation and skips never feed the
    // automatic pause.
    assert!(
        !ObservationStatus::Failed {
            code: "consent_denied".to_owned()
        }
        .counts_as_failure()
    );
    assert!(
        !ObservationStatus::Failed {
            code: "admission_denied:unknown_pool".to_owned()
        }
        .counts_as_failure()
    );
    assert!(
        !ObservationStatus::Cancelled {
            stage: "after_dispatch".to_owned()
        }
        .counts_as_failure()
    );
    assert!(
        !ObservationStatus::Skipped {
            reason: "disabled".to_owned()
        }
        .counts_as_failure()
    );
}
