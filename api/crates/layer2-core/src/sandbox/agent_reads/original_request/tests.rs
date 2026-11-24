use super::*;
use crate::sandbox::{FirstMessageRecord, MAX_FIRST_MESSAGE_BYTES};

fn record(text: &str) -> FirstMessageRecord {
    FirstMessageRecord::new("initial-task", text, "2026-11-20T00:00:00Z").unwrap()
}

#[test]
fn undispatched_task_preserves_exact_text_including_unicode_and_line_breaks() {
    let text = "  Original request\r\n第二行 🦀\n\tlast line  ";
    assert_eq!(
        project(Some(&record(text))).unwrap(),
        Some(AgentOriginalRequest {
            text: text.into(),
            turn_sequence: None,
        })
    );
}

#[test]
fn setup_unknown_purpose_and_missing_record_have_no_original_task() {
    let mut value = record("setup instruction");
    for purpose in [Some(FirstMessagePurpose::Setup), None] {
        value.purpose = purpose;
        assert_eq!(project(Some(&value)).unwrap(), None);
    }
    assert_eq!(project(None).unwrap(), None);
}

#[test]
fn corrupted_or_oversized_record_is_an_error_without_truncation() {
    let mut value = record("valid task");
    value.text = "different task".into();
    assert!(matches!(
        project(Some(&value)),
        Err(AgentReadError::Corrupt(_))
    ));
    value = record(&"x".repeat(MAX_FIRST_MESSAGE_BYTES));
    assert_eq!(
        project(Some(&value)).unwrap().unwrap().text.len(),
        MAX_FIRST_MESSAGE_BYTES
    );
    value.text.push('x');
    assert!(matches!(
        project(Some(&value)),
        Err(AgentReadError::Corrupt(_))
    ));
}

#[test]
fn dispatched_request_reports_the_durable_turn_without_changing_the_body() {
    let mut value = record("exact task");
    value
        .fence_dispatch(3, 2, "2026-11-20T00:01:00Z".into())
        .unwrap();
    assert_eq!(
        project(Some(&value)).unwrap(),
        Some(AgentOriginalRequest {
            text: "exact task".into(),
            turn_sequence: Some(3),
        })
    );
}
