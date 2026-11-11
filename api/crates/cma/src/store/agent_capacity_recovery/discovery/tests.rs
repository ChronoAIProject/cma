use super::*;
use crate::store::agent_capacity_recovery::records::test_support::{work, NOW};

fn row() -> Document {
    let work = work();
    doc! {"_id": work.request.key().unwrap(), "format_version": 2_i64,
    "agent_id": work.request.agent_id.as_str(),
    "work": bson::serialize_to_bson(&work).unwrap()}
}

#[test]
fn streams_have_independent_due_predicates_and_cursor_bindings() {
    for stream in [Stream::Admitted, Stream::ExpiredLease] {
        let (phase, field, _) = stream.fields();
        assert_eq!(
            filter(stream, None, NOW).unwrap(),
            doc! {"format_version": 2_i64, "work.phase": phase, field: {"$lte": NOW}}
        );
        let cursor = Cursor {
            stream,
            at: NOW.into(),
            key: work().request.key().unwrap(),
        };
        let query = filter(stream, Some(&cursor), NOW).unwrap();
        assert_eq!(
            query.get_array("$or").unwrap(),
            &vec![
                bson::Bson::Document(doc! {"format_version": 2_i64, "work.phase": phase,
                field: {"$gt": NOW, "$lte": NOW}}),
                bson::Bson::Document(doc! {"format_version": 2_i64, "work.phase": phase,
                field: {"$eq": NOW, "$lte": NOW}, "_id": {"$gt": &cursor.key}})
            ]
        );
        let other = if stream == Stream::Admitted {
            Stream::ExpiredLease
        } else {
            Stream::Admitted
        };
        assert!(filter(other, Some(&cursor), NOW).is_err());
    }
}

#[test]
fn keyset_refuses_noncanonical_time_and_invalid_keys() {
    for now in [
        "2026-11-11T00:00:00+00:00",
        "2026-11-11T00:00:00.0Z",
        "2026-02-29T00:00:00Z",
        "2026-11-31T00:00:00Z",
        "2026-11-11T24:00:00Z",
        "invalid",
    ] {
        assert!(filter(Stream::Admitted, None, now).is_err());
        let cursor = Cursor {
            stream: Stream::Admitted,
            at: now.into(),
            key: work().request.key().unwrap(),
        };
        assert!(filter(Stream::Admitted, Some(&cursor), NOW).is_err());
    }
    for key in [
        "bad".into(),
        format!("capacity-recovery:{}", "z".repeat(64)),
    ] {
        assert!(filter(
            Stream::Admitted,
            Some(&Cursor {
                stream: Stream::Admitted,
                at: NOW.into(),
                key
            }),
            NOW
        )
        .is_err());
    }
}

#[test]
fn mixed_precision_due_rows_cannot_enter_lexical_ordering() {
    // A fractional second sorts before the whole-second Z suffix, despite
    // representing a later instant. Reject it rather than returning it early.
    assert!("2026-11-11T00:00:00.1Z" < NOW);
    for timestamp in [
        "2026-11-11T00:00:00.1Z",
        "2026-11-11T00:00:00+00:00",
        "2026-02-29T00:00:00Z",
    ] {
        for stream in [Stream::Admitted, Stream::ExpiredLease] {
            let mut row = row();
            let work = row.get_document_mut("work").unwrap();
            let (phase, _, _) = stream.fields();
            work.insert("phase", phase);
            work.insert("ready_at", timestamp);
            work.insert("lease", doc! {"expires_at": timestamp});
            assert!(candidate(&row, stream, phase, NOW).is_err(), "{timestamp}");
        }
    }
}

#[test]
fn candidates_validate_format_identity_phase_revision_and_due_time() {
    let original = row();
    assert_eq!(
        candidate(&original, Stream::Admitted, "admitted", NOW)
            .unwrap()
            .0,
        work().request
    );
    for variant in 0..8 {
        let mut changed = original.clone();
        match variant {
            0 => {
                changed.insert("format_version", 1_i64);
            }
            1 => {
                changed.insert("format_version", 2_i32);
            }
            2 => {
                changed.insert("agent_id", "foreign-agent");
            }
            3 => {
                changed.insert("_id", "foreign-key");
            }
            _ => {
                let mut work = changed.get_document("work").unwrap().clone();
                match variant {
                    4 => {
                        work.insert("revision", 0_i64);
                    }
                    5 => {
                        work.insert("phase", "completed");
                    }
                    6 => {
                        work.insert("ready_at", "2026-11-11T00:01:00Z");
                    }
                    _ => {
                        work.insert("ready_at", "2026-11-11T00:00:00+00:00");
                    }
                }
                changed.insert("work", work);
            }
        }
        assert!(
            candidate(&changed, Stream::Admitted, "admitted", NOW).is_err(),
            "variant {variant}"
        );
    }
}

#[test]
fn expired_stream_uses_lease_expiry_instead_of_ready_time() {
    let mut row = row();
    let mut work = row.get_document("work").unwrap().clone();
    work.insert("phase", "attaching");
    work.insert("lease", doc! {"expires_at": "2026-11-11T00:01:00Z"});
    row.insert("work", work);
    assert!(candidate(&row, Stream::ExpiredLease, "attaching", NOW).is_err());
    let (_, cursor) = candidate(
        &row,
        Stream::ExpiredLease,
        "attaching",
        "2026-11-11T00:01:00Z",
    )
    .unwrap();
    assert_eq!(cursor.at, "2026-11-11T00:01:00Z");
    assert_eq!(cursor.stream, Stream::ExpiredLease);
}
