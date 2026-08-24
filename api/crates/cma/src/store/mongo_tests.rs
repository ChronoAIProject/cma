use super::*;
use layer2_core::{
    AgentOptions, Event, EventKind, KernelId, Rate, RateCardId, RecoveryState, Session, SessionId,
    SessionPricing, SessionRecord, SessionStatus, StopReason, SubjectId, TerminalGeometry,
};
use std::io::Write;
use std::sync::{Arc, Mutex as StdMutex};

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

fn event(session: &str, seq: u64) -> Event {
    Event {
        session_id: SessionId(session.into()),
        seq,
        kind: EventKind::SessionCreated,
        appended_at: "2026-08-24T00:00:00Z".into(),
    }
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<StdMutex<Vec<u8>>>);

struct LogWriter(Arc<StdMutex<Vec<u8>>>);

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("log buffer is not poisoned")
            .extend(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(Arc::clone(&self.0))
    }
}

#[test]
fn a_contiguous_batch_passes_the_pre_insert_check() {
    validate_contiguous_append(
        &[event("sesn_ordered", 4), event("sesn_ordered", 5)],
        Some(3),
    )
    .expect("the durable tail is followed exactly");
}

#[test]
fn an_out_of_order_batch_is_typed_and_logged_before_insertion() {
    let logs = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(logs.clone())
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);

    let error = validate_contiguous_append(&[event("sesn_gap", 5)], Some(3))
        .expect_err("sequence four is not readable");

    assert!(matches!(
        error,
        StoreError::NonContiguousAppend {
            expected: 4,
            got: 5
        }
    ));
    let bytes = logs.0.lock().expect("log buffer").clone();
    let output = String::from_utf8(bytes).expect("utf-8 logs");
    assert!(output.contains("sesn_gap"), "session id missing: {output}");
    assert!(
        output.contains("expected=4"),
        "expected seq missing: {output}"
    );
    assert!(output.contains("got=5"), "received seq missing: {output}");
    assert!(
        output.contains("refused a non-contiguous event append"),
        "refusal missing: {output}"
    );
}

#[test]
fn a_gap_inside_one_batch_is_refused_too() {
    let error = validate_contiguous_append(
        &[event("sesn_internal_gap", 4), event("sesn_internal_gap", 6)],
        Some(3),
    )
    .expect_err("sequence five is absent from the batch");

    assert!(matches!(
        error,
        StoreError::NonContiguousAppend {
            expected: 5,
            got: 6
        }
    ));
}

#[test]
fn a_lower_sequence_is_taken_not_a_forward_gap() {
    let error = validate_contiguous_append(&[event("sesn_lower", 3)], Some(3))
        .expect_err("the durable tail already owns sequence three");

    assert!(matches!(
        error,
        StoreError::SequenceTaken {
            expected: 4,
            got: 3
        }
    ));
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
fn a_document_without_revision_reads_as_revision_zero() {
    let mut record = record_with(None);
    record.revision = 7;
    let stored = to_doc(&record).expect("record can be stored");
    let mut document = mongodb::bson::serialize_to_document(&stored).expect("doc serializes");
    assert!(document.remove("revision").is_some());

    let legacy: SessionDoc =
        mongodb::bson::deserialize_from_bson(document.into()).expect("legacy doc reads");

    assert_eq!(from_doc(legacy).expect("legacy record decodes").revision, 0);
}

#[test]
fn revision_zero_cas_atomically_matches_explicit_and_legacy_zero() {
    assert_eq!(
        session_revision_filter("sesn_legacy", 0),
        doc! {
            "_id": "sesn_legacy",
            "$or": [
                { "revision": 0_i64 },
                { "revision": { "$exists": false } },
            ],
        }
    );
}

#[test]
fn positive_revision_cas_never_matches_a_missing_revision() {
    assert_eq!(
        session_revision_filter("sesn_revised", 4),
        doc! { "_id": "sesn_revised", "revision": 4_i64 }
    );
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
fn a_terminal_mongo_record_cannot_transition_back_to_live() {
    let id = SessionId("sesn_terminal".into());
    assert!(matches!(
        validate_status_transition(&id, "terminated", "ready"),
        Err(StoreError::SessionTerminated { id: refused }) if refused == id
    ));
    validate_status_transition(&id, "terminated", "terminated")
        .expect("terminal cleanup remains writable");
}

#[test]
fn an_empty_record_only_transaction_is_valid_input() {
    validate_transaction_input("write-empty", &record_with(None), &[], false)
        .expect("an empty event batch still updates the record and writes a receipt");
}

#[test]
fn a_receipt_belongs_to_exactly_one_session_and_carries_a_valid_revision() {
    let receipt = SessionWriteDoc {
        id: "write-owned".into(),
        session_id: "sesn_owner".into(),
        revision: 7,
    };
    assert_eq!(
        receipt_revision(&receipt, "write-owned", &SessionId("sesn_owner".into())).unwrap(),
        7
    );
    assert!(receipt_revision(&receipt, "write-owned", &SessionId("sesn_other".into())).is_err());

    let negative = SessionWriteDoc {
        revision: -1,
        ..receipt
    };
    assert!(receipt_revision(&negative, "write-owned", &SessionId("sesn_owner".into())).is_err());
}

#[test]
fn revision_increment_is_checked_before_bson_storage() {
    assert_eq!(next_stored_revision(0).unwrap(), (1, 1));
    assert!(next_stored_revision(i64::MAX as u64).is_err());
    assert!(next_stored_revision(u64::MAX).is_err());
}

#[test]
fn transactional_updates_explicitly_disable_upsert() {
    assert_eq!(session_update_options().upsert, Some(false));
}

#[test]
fn a_session_tail_above_bson_i64_is_refused() {
    let mut record = record_with(None);
    record.session.last_seq = i64::MAX as u64 + 1;
    assert!(matches!(to_doc(&record), Err(StoreError::Malformed(_))));
}

#[test]
fn an_event_cursor_past_bson_i64_is_already_past_all_storable_events() {
    assert_eq!(stored_event_cursor(i64::MAX as u64), Some(i64::MAX));
    assert_eq!(stored_event_cursor(i64::MAX as u64 + 1), None);
}

#[test]
fn a_legacy_record_tail_is_a_sequence_floor_without_authorizing_new_holes() {
    assert_eq!(durable_sequence_floor(Some(7), Some(9)).unwrap(), 9);
    assert_eq!(durable_sequence_floor(Some(11), Some(9)).unwrap(), 11);
    assert!(durable_sequence_floor(Some(7), Some(-1)).is_err());

    let id = SessionId("sesn_legacy_tail".into());
    assert_eq!(resolved_transaction_tail(&id, 9, 9, &[]).unwrap(), 9);
    assert_eq!(
        resolved_transaction_tail(&id, 11, 9, &[]).unwrap(),
        11,
        "a record-only update preserves events already ahead of its snapshot"
    );
    assert!(resolved_transaction_tail(&id, 9, 10, &[]).is_err());
    assert_eq!(
        resolved_transaction_tail(&id, 9, 10, &[event("sesn_legacy_tail", 10)]).unwrap(),
        10
    );
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
fn reserved_capacity_pipeline_matches_and_sums_only_durable_runtime_promises() {
    let pipeline = reserved_capacity_pipeline().expect("the core statuses serialize");
    assert_eq!(
        pipeline,
        vec![
            doc! {
                "$match": {
                    "status": {
                        "$in": ["provisioning", "ready", "running", "rotating"]
                    }
                }
            },
            doc! {
                "$group": {
                    "_id": null,
                    "sessions": { "$sum": 1_i64 },
                    "unsized_sessions": {
                        "$sum": {
                            "$cond": [
                                { "$eq": [{ "$ifNull": ["$options.size", null] }, null] },
                                1_i64,
                                0_i64,
                            ]
                        }
                    },
                    "cpu_millis": {
                        "$sum": { "$ifNull": ["$options.size.cpu_millis", 0_i64] }
                    },
                    "memory_mib": {
                        "$sum": { "$ifNull": ["$options.size.memory_mib", 0_i64] }
                    },
                }
            },
        ]
    );
}

#[test]
fn reservation_query_has_a_status_index() {
    let index = reservation_status_index();
    assert_eq!(index.keys, doc! { "status": 1 });
    assert_eq!(
        index.options.and_then(|options| options.name),
        Some("status_1".into())
    );
}

#[tokio::test]
async fn readiness_ping_accepts_a_database_reply() {
    bounded_ping(
        async { Ok(doc! { "ok": 1_i32 }) },
        Duration::from_millis(10),
    )
    .await
    .expect("a database reply is healthy");
}

#[tokio::test]
async fn readiness_ping_returns_at_its_deadline_without_cancelling_the_driver_future() {
    let (release, wait_for_release) = tokio::sync::oneshot::channel();
    let completed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let completed_by_ping = std::sync::Arc::clone(&completed);
    let error = bounded_ping(
        async move {
            wait_for_release.await.expect("test releases the ping");
            completed_by_ping.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(doc! { "ok": 1_i32 })
        },
        Duration::from_millis(10),
    )
    .await
    .expect_err("a hung database must fail readiness");

    assert!(matches!(error, StoreError::Unavailable(_)));
    assert!(error.to_string().contains("timed out after 10 ms"));

    release
        .send(())
        .expect("the timed-out ping task remains alive");
    tokio::time::timeout(Duration::from_secs(1), async {
        while !completed.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the detached ping is still polled to completion");
}

#[test]
fn an_empty_reservation_aggregate_is_the_zero_ledger() {
    assert_eq!(
        reserved_from_optional_doc(None).unwrap(),
        Reserved {
            sessions: 0,
            unsized_sessions: 0,
            cpu_millis: 0,
            memory_mib: 0,
        }
    );
}

#[test]
fn a_reservation_aggregate_is_converted_without_losing_units() {
    assert_eq!(
        reserved_from_doc(ReservedCapacityDoc {
            sessions: 4,
            unsized_sessions: 2,
            cpu_millis: 3_500,
            memory_mib: 7_168,
        })
        .unwrap(),
        Reserved {
            sessions: 4,
            unsized_sessions: 2,
            cpu_millis: 3_500,
            memory_mib: 7_168,
        }
    );
    assert!(reserved_from_doc(ReservedCapacityDoc {
        sessions: -1,
        unsized_sessions: 0,
        cpu_millis: 0,
        memory_mib: 0,
    })
    .is_err());
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

    assert!(sequence_taken(&error));
}

#[test]
fn any_duplicate_position_is_recognized_after_an_atomic_abort() {
    let error = insert_failure(serde_json::json!([
        { "index": 2, "code": 11000, "errmsg": "dup" },
        { "index": 3, "code": 11000, "errmsg": "dup" }
    ]));

    assert!(sequence_taken(&error));
}

#[test]
fn any_other_failure_is_left_alone() {
    let other = insert_failure(serde_json::json!([{
        "index": 0, "code": 121, "errmsg": "document failed validation"
    }]));
    assert!(!sequence_taken(&other));

    let unreachable: mongodb::error::Error =
        mongodb::error::ErrorKind::Io(std::sync::Arc::new(std::io::Error::other("gone"))).into();
    assert!(!sequence_taken(&unreachable));
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_real_legacy_document_passes_its_first_revision_zero_cas() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database = format!("chrono_legacy_revision_{}_{}", std::process::id(), nonce);
    let store = MongoSessionStore::open(&client, &database)
        .await
        .expect("test store opens");

    let original = record_with(None);
    let stored = to_doc(&original).expect("record encodes");
    let mut legacy = mongodb::bson::serialize_to_document(&stored).expect("record serializes");
    assert!(legacy.remove("revision").is_some());
    store
        .sessions
        .clone_with_type::<Document>()
        .insert_one(legacy)
        .await
        .expect("legacy document inserts");

    let mut update = store
        .get(&original.session.id)
        .await
        .expect("legacy record reads")
        .expect("legacy record exists");
    assert_eq!(update.revision, 0);
    update.context_ref = Some("first-cas-succeeded".into());
    let revision = store
        .commit_session_idempotent("legacy-first-cas", &update, &[])
        .await
        .expect("missing revision atomically compares as zero");

    assert_eq!(revision, 1);
    let durable = store
        .get(&original.session.id)
        .await
        .expect("updated record reads")
        .expect("updated record exists");
    assert_eq!(durable.revision, 1);
    assert_eq!(durable.context_ref.as_deref(), Some("first-cas-succeeded"));

    client
        .database(&database)
        .drop()
        .await
        .expect("temporary database drops");
}
