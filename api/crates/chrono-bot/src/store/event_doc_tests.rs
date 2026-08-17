//! What a reader does with a document it did not write.
//!
//! Exercised over documents rather than over a live database: the behaviour
//! under test is the decision between "history from a newer writer" and
//! "damage", and that decision is made entirely on the shape of one document.

use layer2_core::{Event, EventKind, SessionId, StoredEventKind};
use mongodb::bson::{doc, Bson};

use super::{from_document, to_document};

fn event(seq: u64, kind: EventKind) -> Event {
    Event {
        session_id: SessionId("s-1".into()),
        seq,
        kind,
    }
}

#[test]
fn a_written_event_reads_back_as_the_kind_it_was() {
    let original = event(
        7,
        EventKind::TerminalResized {
            columns: 120,
            rows: 40,
        },
    );
    let stored = from_document(to_document(&original).expect("writes")).expect("reads");

    assert_eq!(stored.seq, 7);
    assert_eq!(stored.session_id, SessionId("s-1".into()));
    match stored.kind {
        StoredEventKind::Known(EventKind::TerminalResized { columns, rows }) => {
            assert_eq!((columns, rows), (120, 40));
        }
        other => panic!("expected the kind it was written as, got {other:?}"),
    }
}

#[test]
fn every_current_variant_survives_the_round_trip() {
    let kinds = vec![
        EventKind::SessionCreated,
        EventKind::Progress {
            stage: layer2_core::session::ProvisioningStage::Attaching,
        },
        EventKind::StatusChanged {
            status: layer2_core::SessionStatus::Ready,
            stop_reason: None,
            detail: None,
        },
        EventKind::TurnStarted,
        EventKind::TurnCompleted,
        EventKind::TerminalAttached,
        EventKind::TerminalDetached,
        EventKind::TerminalResized {
            columns: 80,
            rows: 24,
        },
        EventKind::ApprovalRequested {
            request_id: "req-1".into(),
            action: Some("run a command".into()),
            detail: None,
        },
        EventKind::MessageCommitted {
            content: "hello".into(),
        },
        EventKind::ToolCallStarted {
            call_id: "call-1".into(),
            title: "read a file".into(),
            kind: "read".into(),
        },
        EventKind::ToolCallUpdated {
            call_id: "call-1".into(),
            status: "completed".into(),
        },
        EventKind::UsageReported {
            input_tokens: 1234,
            output_tokens: 56,
            context_used: 1290,
        },
        EventKind::ConversationGap { frames: 7 },
        EventKind::ApprovalResolved {
            request_id: "req-1".into(),
            allowed: false,
        },
    ];

    for (index, kind) in kinds.into_iter().enumerate() {
        let written = to_document(&event(index as u64 + 1, kind.clone())).expect("writes");
        let stored = from_document(written).expect("reads");
        match stored.kind {
            StoredEventKind::Known(read_back) => assert_eq!(
                serde_json::to_value(&read_back).expect("serializes"),
                serde_json::to_value(&kind).expect("serializes"),
                "{kind:?} did not survive storage"
            ),
            StoredEventKind::Unknown { tag, .. } => {
                panic!("{kind:?} was read back as an unknown kind tagged {tag}")
            }
        }
    }
}

#[test]
fn a_kind_from_a_newer_writer_is_carried_back_rather_than_refused() {
    let stored = from_document(doc! {
        "session_id": "s-1",
        "seq": 42_i64,
        "kind": { "type": "a_kind_a_newer_control_plane_invented", "content": "hello" },
    })
    .expect("an unfamiliar kind is not a failure");

    assert_eq!(stored.seq, 42);
    match stored.kind {
        StoredEventKind::Unknown { tag, payload } => {
            assert_eq!(tag, "a_kind_a_newer_control_plane_invented");
            assert_eq!(
                payload["content"], "hello",
                "the payload is kept so an operator can see what was skipped"
            );
            assert!(
                payload.get("type").is_none(),
                "the discriminator is carried on its own, not twice"
            );
        }
        other => panic!("expected the unknown arm, got {other:?}"),
    }
}

#[test]
fn a_document_missing_its_sequence_is_still_damage() {
    let error = from_document(doc! {
        "session_id": "s-1",
        "kind": { "type": "session_created" },
    })
    .expect_err("a missing sequence is corruption, not vocabulary");
    assert!(
        matches!(error, layer2_core::store::StoreError::Malformed(_)),
        "expected Malformed, got {error:?}"
    );
}

#[test]
fn a_document_whose_sequence_has_the_wrong_type_is_still_damage() {
    let error = from_document(doc! {
        "session_id": "s-1",
        "seq": "seven",
        "kind": { "type": "session_created" },
    })
    .expect_err("a sequence that is not a number is corruption");
    assert!(
        matches!(error, layer2_core::store::StoreError::Malformed(_)),
        "expected Malformed, got {error:?}"
    );
}

#[test]
fn a_document_with_no_kind_is_still_damage() {
    let error = from_document(doc! { "session_id": "s-1", "seq": 1_i64 })
        .expect_err("an event with no kind is corruption");
    assert!(
        matches!(error, layer2_core::store::StoreError::Malformed(_)),
        "expected Malformed, got {error:?}"
    );
}

#[test]
fn a_broken_payload_under_a_kind_we_own_is_still_damage() {
    // The discriminator is ours, so the payload is validated strictly: this is
    // the boundary the tolerant path must not be allowed to swallow.
    let error = from_document(doc! {
        "session_id": "s-1",
        "seq": 1_i64,
        "kind": { "type": "terminal_resized", "columns": "wide", "rows": 24_i32 },
    })
    .expect_err("a payload we can name and cannot read is corruption");
    assert!(
        matches!(error, layer2_core::store::StoreError::Malformed(_)),
        "expected Malformed, got {error:?}"
    );
}

#[test]
fn a_negative_sequence_is_refused_rather_than_wrapped() {
    let error = from_document(doc! {
        "session_id": "s-1",
        "seq": -1_i64,
        "kind": { "type": "session_created" },
    })
    .expect_err("a negative sequence cannot be a position in the log");
    assert!(
        matches!(error, layer2_core::store::StoreError::Malformed(_)),
        "expected Malformed, got {error:?}"
    );
}

#[test]
fn a_kind_that_is_not_a_document_is_damage_rather_than_an_unknown_tag() {
    let error = from_document(doc! {
        "session_id": "s-1",
        "seq": 1_i64,
        "kind": Bson::String("session_created".into()),
    })
    .expect_err("the envelope must hold, whatever the vocabulary is");
    assert!(
        matches!(error, layer2_core::store::StoreError::Malformed(_)),
        "expected Malformed, got {error:?}"
    );
}
