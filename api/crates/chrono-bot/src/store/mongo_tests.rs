use super::*;
use layer2_core::{
    AgentOptions, KernelId, Rate, RateCardId, RecoveryState, Session, SessionId, SessionPricing,
    SessionRecord, SessionStatus, StopReason, SubjectId, TerminalGeometry,
};

fn record_with(pricing: Option<SessionPricing>) -> SessionRecord {
    SessionRecord::new(Session {
        id: SessionId("sesn_priced".into()),
        owner: SubjectId("subject-priced".into()),
        kernel: KernelId::new("example-runtime"),
        status: SessionStatus::Ready,
        recovery_state: RecoveryState::Normal,
        stop_reason: None,
        options: AgentOptions::default(),
        environment_profile: None,
        geometry: TerminalGeometry::default(),
        last_seq: 7,
        runtime_version: None,
        rotations: 0,
        rotated_at: None,
        capabilities: Default::default(),
        granted_services: None,
        granted_skills: None,
        pricing,
        created_at: "2026-08-23T04:00:00Z".into(),
    })
}

fn pricing() -> SessionPricing {
    SessionPricing {
        rate_card: RateCardId("rate_exact".into()),
        rate: Rate {
            cpu_micros_per_cpu_hour: 40_000,
            memory_micros_per_gib_hour: 5_000,
        },
        cpu_millis: 1_500,
        memory_mib: 3_072,
        micros_per_hour: 75_000,
        quoted_at: "2026-08-23T03:59:59Z".into(),
    }
}

#[test]
fn session_pricing_round_trips_exactly() {
    let expected = pricing();
    let stored = to_doc(&record_with(Some(expected.clone()))).expect("price can be stored");
    let read = from_doc(stored).expect("price can be read");

    assert_eq!(read.session.pricing, Some(expected));
}

#[test]
fn a_document_without_pricing_reads_as_unpriced() {
    let stored = to_doc(&record_with(Some(pricing()))).expect("price can be stored");
    let mut document = mongodb::bson::serialize_to_document(&stored).expect("doc serializes");
    assert!(document.remove("pricing").is_some());
    let old: SessionDoc = mongodb::bson::deserialize_from_bson(document.into()).expect("doc reads");

    assert_eq!(from_doc(old).expect("old doc reads").session.pricing, None);
}

#[test]
fn a_price_above_bson_i64_is_refused_instead_of_clamped() {
    let mut too_large = pricing();
    too_large.rate.cpu_micros_per_cpu_hour = i64::MAX as u64 + 1;

    let error = to_doc(&record_with(Some(too_large))).expect_err("overflow must fail");
    assert!(matches!(error, StoreError::Malformed(_)));
    assert!(error.to_string().contains("cannot be stored"));
}

#[test]
fn a_negative_stored_price_is_refused_instead_of_guessed() {
    let mut stored = to_doc(&record_with(Some(pricing()))).expect("price can be stored");
    stored
        .pricing
        .as_mut()
        .expect("price is present")
        .micros_per_hour = -1;

    let error = from_doc(stored).expect_err("negative price must fail");
    assert!(matches!(error, StoreError::Malformed(_)));
    assert!(error.to_string().contains("stored price of -1"));
}

#[test]
fn enums_round_trip_through_their_serde_representation() {
    // The stored string must be the same one the interfaces emit, or a value
    // would mean different things depending on which layer wrote it.
    assert_eq!(serde_plain_string(&SessionStatus::Ready).unwrap(), "ready");
    assert_eq!(
        serde_plain_string(&RecoveryState::Recovering).unwrap(),
        "recovering"
    );
    assert_eq!(
        serde_plain_string(&StopReason::RequiresAction).unwrap(),
        "requires_action"
    );

    let s: SessionStatus = parse_plain("terminated").unwrap();
    assert_eq!(s, SessionStatus::Terminated);
    let r: RecoveryState = parse_plain("degraded").unwrap();
    assert_eq!(r, RecoveryState::Degraded);
}

#[test]
fn wanting_runtime_statuses_use_the_core_serialization() {
    let statuses = SessionStatus::WANTING_RUNTIME
        .iter()
        .map(serde_plain_string)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(statuses, ["provisioning", "ready", "running", "rotating"]);
}

#[test]
fn an_unknown_stored_value_is_rejected_rather_than_guessed() {
    assert!(parse_plain::<SessionStatus>("not-a-status").is_err());
}

/// Build the failure the database returns for a duplicate sequence.
///
/// Deserialised from the server's own shape rather than assembled, because
/// that shape is the contract being read: a driver that renamed a field would
/// break this the same way it breaks production.
fn insert_failure(errors: serde_json::Value) -> mongodb::error::Error {
    let failed: mongodb::error::InsertManyError =
        serde_json::from_value(serde_json::json!({ "writeErrors": errors }))
            .expect("the driver's own shape");
    mongodb::error::ErrorKind::InsertMany(failed).into()
}

#[test]
fn a_duplicate_sequence_is_named_rather_than_reported_as_an_outage() {
    // The production failure this exists for. Reported as `Unavailable`, it
    // read as a storage problem and the batch was dropped; the turn had run
    // and no interface could say what it did.
    let error = insert_failure(serde_json::json!([{
        "index": 0,
        "code": 11000,
        "errmsg": "E11000 duplicate key error collection: chrono-bot.session_events \
                   index: session_id_1_seq_1"
    }]));

    assert!(matches!(
        sequence_taken(&error, 4),
        Some(StoreError::SequenceTaken {
            written: 0,
            total: 4
        })
    ));
}

#[test]
fn the_count_is_what_the_database_accepted_before_it_stopped() {
    // Inserts are ordered, so the first failing position is the number already
    // written. A caller re-numbering from zero would write those twice.
    let error = insert_failure(serde_json::json!([
        { "index": 2, "code": 11000, "errmsg": "dup" },
        { "index": 3, "code": 11000, "errmsg": "dup" }
    ]));

    assert!(matches!(
        sequence_taken(&error, 5),
        Some(StoreError::SequenceTaken {
            written: 2,
            total: 5
        })
    ));
}

#[test]
fn any_other_failure_is_left_alone() {
    let other = insert_failure(serde_json::json!([{
        "index": 0, "code": 121, "errmsg": "document failed validation"
    }]));
    assert!(sequence_taken(&other, 1).is_none());

    let unreachable: mongodb::error::Error =
        mongodb::error::ErrorKind::Io(std::sync::Arc::new(std::io::Error::other("gone"))).into();
    assert!(sequence_taken(&unreachable, 1).is_none());
}
