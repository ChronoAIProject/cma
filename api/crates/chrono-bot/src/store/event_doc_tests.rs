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
        appended_at: String::new(),
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
            capability: None,
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
        EventKind::OwnerMessage {
            content: "hello back".into(),
        },
        EventKind::ToolCallStarted {
            call_id: "call-1".into(),
            title: "read a file".into(),
            kind: "read".into(),
            arguments: None,
        },
        EventKind::ToolCallUpdated {
            call_id: "call-1".into(),
            status: "completed".into(),
            result: None,
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
fn a_field_a_newer_writer_added_does_not_make_an_event_unreadable() {
    // `CLAUDE.md` §5: no deployment may interrupt a session its owner has not
    // stopped, and a deployment is a rolling one — a replica on the older
    // build reads events the newer build has already written.
    //
    // A tag this build has never heard of is already handled, and tested
    // above. This is the other direction, and it is quieter: a tag this build
    // *owns*, carrying a field it does not. It takes the strict path, so if
    // the vocabulary ever gained `deny_unknown_fields` every session's history
    // would become unreadable to the replica that had not been replaced yet —
    // and it would look like corruption rather than like a version skew.
    //
    // `capability` on `status_changed` is the field that occasioned this; the
    // property is about any field, and this test is here so adding the
    // attribute fails loudly rather than in production during a deploy.
    let read = from_document(doc! {
        "session_id": "s-1",
        "seq": 9_i64,
        "kind": {
            "type": "status_changed",
            "status": "terminated",
            "stop_reason": "error",
            "detail": "this session's runtime can no longer be what it is",
            "capability": "conversation",
            "something_a_later_build_added": "and this build has never heard of",
        },
    })
    .expect("a known tag with an unfamiliar field is still history, not damage");

    match read.kind {
        StoredEventKind::Known(EventKind::StatusChanged {
            capability, detail, ..
        }) => {
            assert_eq!(capability.as_deref(), Some("conversation"));
            assert!(detail.is_some(), "and everything it does know still reads");
        }
        other => panic!("expected a status change, got {other:?}"),
    }
}

#[test]
fn an_event_written_before_a_field_existed_still_reads() {
    // The same property from the other side of a deploy: the newer build
    // reading what the older one wrote. Every session in production has a log
    // full of these, and §5 makes losing them as severe as losing the data.
    let read = from_document(doc! {
        "session_id": "s-1",
        "seq": 4_i64,
        "kind": {
            "type": "status_changed",
            "status": "idle",
            "stop_reason": "user_terminated",
        },
    })
    .expect("an event from before the field existed is ordinary history");

    match read.kind {
        StoredEventKind::Known(EventKind::StatusChanged {
            capability, detail, ..
        }) => {
            assert!(capability.is_none());
            assert!(detail.is_none());
        }
        other => panic!("expected a status change, got {other:?}"),
    }
}

#[test]
fn the_append_time_round_trips() {
    let mut original = event(3, EventKind::SessionCreated);
    original.appended_at = "2026-08-19T04:00:00Z".into();
    let stored = from_document(to_document(&original).expect("writes")).expect("reads");
    assert_eq!(stored.appended_at, "2026-08-19T04:00:00Z");
}

#[test]
fn an_event_without_an_append_time_reads_back_empty() {
    // An event written before `appended_at` existed: ordinary history with no
    // time, not damage. §5 makes reading it back a hard requirement.
    let read = from_document(doc! {
        "session_id": "s-1",
        "seq": 4_i64,
        "kind": { "type": "session_created" },
    })
    .expect("a document without appended_at is ordinary history");
    assert!(read.appended_at.is_empty());
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
