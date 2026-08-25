use super::*;
use layer2_core::{
    AgentOptions, Event, EventKind, KernelId, PageCursor, PageRequest, Rate, RateCardId,
    RecoveryState, Session, SessionId, SessionListQuery, SessionPricing, SessionRecord,
    SessionStatus, StopReason, SubjectId, TerminalGeometry,
};
use std::io::Write;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex as StdMutex,
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
fn a_rotation_claim_round_trips_exactly() {
    let mut expected = record_with(None);
    expected.rotation_claim = Some("rotn_durable_owner".into());

    let stored = to_doc(&expected).expect("rotation claim can be stored");
    let read = from_doc(stored).expect("rotation claim can be read");

    assert_eq!(read.rotation_claim, expected.rotation_claim);
}

#[test]
fn a_document_without_a_rotation_claim_reads_as_unclaimed() {
    let mut current = record_with(None);
    current.rotation_claim = Some("rotn_not_present_in_legacy".into());
    let stored = to_doc(&current).expect("record can be stored");
    let mut document = mongodb::bson::serialize_to_document(&stored).expect("doc serializes");
    assert!(document.remove("rotation_claim").is_some());
    let legacy: SessionDoc =
        mongodb::bson::deserialize_from_bson(document.into()).expect("legacy doc reads");

    assert_eq!(
        from_doc(legacy)
            .expect("legacy record decodes")
            .rotation_claim,
        None
    );
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
    assert!(document.remove(WRITE_PROTOCOL_FIELD).is_some());

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
        write_protocol: WRITE_PROTOCOL,
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
            tombstone_lookup_stage(),
            doc! {
                "$match": { "_cma_tombstones.0": { "$exists": false } }
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
fn transaction_topology_accepts_replica_sets_and_mongos_only() {
    validate_transaction_topology_reply(&doc! { "setName": "rs0" })
        .expect("a replica set supports transactions");
    validate_transaction_topology_reply(&doc! { "msg": "isdbgrid" })
        .expect("mongos supports transactions");

    let standalone = validate_transaction_topology_reply(&doc! { "isWritablePrimary": true })
        .expect_err("a standalone cannot commit this store's transactions");
    assert!(matches!(standalone, StoreError::Unavailable(_)));
    assert!(standalone.to_string().contains("replica set or mongos"));
}

#[test]
fn namespace_cutover_fences_before_rename_and_recovers_only_a_proven_late_spill() {
    assert_eq!(
        next_namespace_action(&NamespacePair::default(), "old", "new").unwrap(),
        NamespaceAction::CreateAuthoritative
    );
    assert_eq!(
        next_namespace_action(
            &NamespacePair {
                legacy: Some(NamespaceKind::Collection),
                authoritative: None,
                ..NamespacePair::default()
            },
            "old",
            "new",
        )
        .unwrap(),
        NamespaceAction::FenceLegacy
    );
    assert_eq!(
        next_namespace_action(
            &NamespacePair {
                legacy: Some(NamespaceKind::Collection),
                legacy_fenced: true,
                ..NamespacePair::default()
            },
            "old",
            "new",
        )
        .unwrap(),
        NamespaceAction::RenameLegacy
    );
    assert_eq!(
        next_namespace_action(
            &NamespacePair {
                legacy: None,
                authoritative: Some(NamespaceKind::Collection),
                ..NamespacePair::default()
            },
            "old",
            "new",
        )
        .unwrap(),
        NamespaceAction::FenceAuthoritative
    );
    assert_eq!(
        next_namespace_action(
            &NamespacePair {
                authoritative: Some(NamespaceKind::Collection),
                authoritative_fenced: true,
                ..NamespacePair::default()
            },
            "old",
            "new",
        )
        .unwrap(),
        NamespaceAction::CreateLegacyView
    );
    assert_eq!(
        next_namespace_action(
            &NamespacePair {
                legacy: Some(NamespaceKind::View {
                    source: Some("new".into()),
                    pipeline_is_empty: true,
                }),
                authoritative: Some(NamespaceKind::Collection),
                authoritative_fenced: true,
                ..NamespacePair::default()
            },
            "old",
            "new",
        )
        .unwrap(),
        NamespaceAction::Ready
    );

    let unproven_split_brain = next_namespace_action(
        &NamespacePair {
            legacy: Some(NamespaceKind::Collection),
            authoritative: Some(NamespaceKind::Collection),
            ..NamespacePair::default()
        },
        "old",
        "new",
    )
    .expect_err("two writable histories must never be merged or guessed between");
    assert!(unproven_split_brain.to_string().contains("ambiguous"));

    assert_eq!(
        next_namespace_action(
            &NamespacePair {
                legacy: Some(NamespaceKind::Collection),
                authoritative: Some(NamespaceKind::Collection),
                authoritative_fenced: true,
                ..NamespacePair::default()
            },
            LEGACY_EVENTS,
            EVENTS,
        )
        .unwrap(),
        NamespaceAction::FenceLateLegacy
    );
    assert_eq!(
        next_namespace_action(
            &NamespacePair {
                legacy: Some(NamespaceKind::Collection),
                authoritative: Some(NamespaceKind::Collection),
                legacy_fenced: true,
                authoritative_fenced: true,
                ..NamespacePair::default()
            },
            LEGACY_EVENTS,
            EVENTS,
        )
        .unwrap(),
        NamespaceAction::MergeLateLegacy
    );

    let late_session_spill = next_namespace_action(
        &NamespacePair {
            legacy: Some(NamespaceKind::Collection),
            authoritative: Some(NamespaceKind::Collection),
            legacy_fenced: true,
            authoritative_fenced: true,
            ..NamespacePair::default()
        },
        LEGACY_SESSIONS,
        SESSIONS,
    )
    .expect_err("a session spill is ambiguous even when both collections are fenced");
    assert!(late_session_spill.to_string().contains("refusing to merge"));

    assert!(next_namespace_action(
        &NamespacePair {
            legacy: Some(NamespaceKind::View {
                source: Some("somewhere_else".into()),
                pipeline_is_empty: true,
            }),
            authoritative: Some(NamespaceKind::Collection),
            authoritative_fenced: true,
            ..NamespacePair::default()
        },
        "old",
        "new",
    )
    .is_err());
}

#[test]
fn cutover_delete_owner_lookup_uses_only_the_current_fenced_session_collection() {
    let fenced_legacy = NamespacePair {
        legacy: Some(NamespaceKind::Collection),
        legacy_fenced: true,
        ..NamespacePair::default()
    };
    assert_eq!(
        cutover_session_owner_namespace(&fenced_legacy).unwrap(),
        Some(LEGACY_SESSIONS)
    );

    let fenced_authoritative = NamespacePair {
        authoritative: Some(NamespaceKind::Collection),
        authoritative_fenced: true,
        ..NamespacePair::default()
    };
    assert_eq!(
        cutover_session_owner_namespace(&fenced_authoritative).unwrap(),
        Some(SESSIONS)
    );

    let ready = NamespacePair {
        legacy: Some(NamespaceKind::View {
            source: Some(SESSIONS.into()),
            pipeline_is_empty: true,
        }),
        authoritative: Some(NamespaceKind::Collection),
        authoritative_fenced: true,
        ..NamespacePair::default()
    };
    assert_eq!(
        cutover_session_owner_namespace(&ready).unwrap(),
        Some(SESSIONS)
    );
    assert_eq!(
        cutover_session_owner_namespace(&NamespacePair::default()).unwrap(),
        None
    );

    let unfenced = NamespacePair {
        authoritative: Some(NamespaceKind::Collection),
        ..NamespacePair::default()
    };
    assert!(cutover_session_owner_namespace(&unfenced).is_err());
}

#[test]
fn cutover_delete_owner_lookup_retries_a_rename_between_selection_and_read() {
    let uuid = test_collection_uuid(41);
    let before = NamespacePair {
        legacy: Some(NamespaceKind::Collection),
        legacy_fenced: true,
        legacy_uuid: Some(uuid.clone()),
        ..NamespacePair::default()
    };
    let after = NamespacePair {
        authoritative: Some(NamespaceKind::Collection),
        authoritative_fenced: true,
        authoritative_uuid: Some(uuid),
        ..NamespacePair::default()
    };

    assert_eq!(
        resolve_cutover_session_owner_lookup(&before, &after, false).unwrap(),
        CutoverOwnerLookup::Retry
    );
    assert_eq!(
        resolve_cutover_session_owner_lookup(&after, &after, true).unwrap(),
        CutoverOwnerLookup::Present
    );
    assert_eq!(
        resolve_cutover_session_owner_lookup(&after, &after, false).unwrap(),
        CutoverOwnerLookup::Absent
    );
}

#[test]
fn cutover_delete_owner_lookup_rejects_ambiguity_appearing_during_read() {
    let before = NamespacePair {
        authoritative: Some(NamespaceKind::Collection),
        authoritative_fenced: true,
        authoritative_uuid: Some(test_collection_uuid(42)),
        ..NamespacePair::default()
    };
    let ambiguous_after = NamespacePair {
        legacy: Some(NamespaceKind::Collection),
        authoritative: Some(NamespaceKind::Collection),
        authoritative_fenced: true,
        legacy_uuid: Some(test_collection_uuid(43)),
        authoritative_uuid: Some(test_collection_uuid(42)),
        ..NamespacePair::default()
    };

    let error = resolve_cutover_session_owner_lookup(&before, &ambiguous_after, false)
        .expect_err("a recreated legacy collection must block tombstone reconciliation");
    assert!(error.to_string().contains("ambiguous"));

    let replaced_after = NamespacePair {
        legacy_uuid: Some(test_collection_uuid(44)),
        ..NamespacePair {
            legacy: Some(NamespaceKind::Collection),
            legacy_fenced: true,
            legacy_uuid: Some(test_collection_uuid(43)),
            ..NamespacePair::default()
        }
    };
    let replaced_before = NamespacePair {
        legacy: Some(NamespaceKind::Collection),
        legacy_fenced: true,
        legacy_uuid: Some(test_collection_uuid(43)),
        ..NamespacePair::default()
    };
    assert_eq!(
        resolve_cutover_session_owner_lookup(&replaced_before, &replaced_after, false).unwrap(),
        CutoverOwnerLookup::Retry
    );
}

#[test]
fn namespace_cutover_requires_an_exact_operator_gate_until_both_views_are_complete() {
    assert_eq!(
        namespace_cutover_authorization(
            &NamespacePair::default(),
            &NamespacePair::default(),
            false,
            None,
        )
        .unwrap(),
        NamespaceCutoverAuthorization::RecordNewDatabase
    );

    let writable_sessions = NamespacePair {
        legacy: Some(NamespaceKind::Collection),
        ..NamespacePair::default()
    };
    let writable_events = NamespacePair {
        legacy: Some(NamespaceKind::Collection),
        ..NamespacePair::default()
    };
    let error = namespace_cutover_authorization(&writable_sessions, &writable_events, false, None)
        .expect_err("default startup must not authorize a legacy namespace mutation");
    assert!(error
        .to_string()
        .contains("stop every pre-cutover CMA replica"));
    assert!(error
        .to_string()
        .contains("CHRONO_MONGODB_LEGACY_CUTOVER=drained"));
    assert_eq!(
        namespace_cutover_authorization(&writable_sessions, &writable_events, true, None).unwrap(),
        NamespaceCutoverAuthorization::RecordOperatorDrain
    );
    let in_progress = NamespaceCutoverMarker::InProgress {
        origin: "operator-drained".into(),
    };
    assert_eq!(
        namespace_cutover_authorization(
            &writable_sessions,
            &writable_events,
            false,
            Some(&in_progress),
        )
        .unwrap(),
        NamespaceCutoverAuthorization::Proven
    );

    let ready = |authoritative: &str, uuid| NamespacePair {
        legacy: Some(NamespaceKind::View {
            source: Some(authoritative.into()),
            pipeline_is_empty: true,
        }),
        authoritative: Some(NamespaceKind::Collection),
        authoritative_marked: true,
        authoritative_fenced: true,
        authoritative_uuid: Some(uuid),
        ..NamespacePair::default()
    };
    let sessions = ready(SESSIONS, test_collection_uuid(1));
    let events = ready(EVENTS, test_collection_uuid(2));
    let collection_ids = BTreeMap::from([
        (SESSIONS.to_owned(), test_collection_uuid(1)),
        (EVENTS.to_owned(), test_collection_uuid(2)),
    ]);
    assert_eq!(
        namespace_cutover_authorization(&sessions, &events, false, None).unwrap(),
        NamespaceCutoverAuthorization::RecordCompletion {
            origin: NAMESPACE_CUTOVER_OBSERVED_ORIGIN.into(),
            collection_ids: collection_ids.clone(),
        }
    );
    assert_eq!(
        namespace_cutover_authorization(&sessions, &events, false, Some(&in_progress)).unwrap(),
        NamespaceCutoverAuthorization::RecordCompletion {
            origin: "operator-drained".into(),
            collection_ids: collection_ids.clone(),
        }
    );
    let completed = NamespaceCutoverMarker::Complete {
        origin: "operator-drained".into(),
        collection_ids,
    };
    assert_eq!(
        namespace_cutover_authorization(&sessions, &events, false, Some(&completed)).unwrap(),
        NamespaceCutoverAuthorization::Complete,
    );
}

#[test]
fn durable_new_database_provenance_resumes_every_partial_bootstrap_state() {
    let ready = |authoritative: &str, uuid| NamespacePair {
        legacy: Some(NamespaceKind::View {
            source: Some(authoritative.into()),
            pipeline_is_empty: true,
        }),
        authoritative: Some(NamespaceKind::Collection),
        authoritative_marked: true,
        authoritative_fenced: true,
        authoritative_uuid: Some(uuid),
        ..NamespacePair::default()
    };
    let ready_sessions = ready(SESSIONS, test_collection_uuid(1));
    let ready_events = ready(EVENTS, test_collection_uuid(2));
    let in_progress = NamespaceCutoverMarker::InProgress {
        origin: "new-database".into(),
    };
    let event_states = [
        NamespacePair {
            authoritative: Some(NamespaceKind::Collection),
            ..NamespacePair::default()
        },
        NamespacePair {
            authoritative: Some(NamespaceKind::Collection),
            authoritative_marked: true,
            ..NamespacePair::default()
        },
        NamespacePair {
            authoritative: Some(NamespaceKind::Collection),
            authoritative_marked: true,
            authoritative_fenced: true,
            ..NamespacePair::default()
        },
        ready_events.clone(),
    ];
    let session_states = [
        NamespacePair::default(),
        NamespacePair {
            authoritative: Some(NamespaceKind::Collection),
            ..NamespacePair::default()
        },
        NamespacePair {
            authoritative: Some(NamespaceKind::Collection),
            authoritative_marked: true,
            authoritative_fenced: true,
            ..NamespacePair::default()
        },
        ready_sessions.clone(),
    ];

    for events in &event_states {
        let without_proof =
            namespace_cutover_authorization(&NamespacePair::default(), events, false, None);
        assert!(
            without_proof.is_err(),
            "unproven partial event state: {events:?}"
        );
        assert_eq!(
            namespace_cutover_authorization(
                &NamespacePair::default(),
                events,
                false,
                Some(&in_progress),
            )
            .unwrap(),
            NamespaceCutoverAuthorization::Proven
        );
    }
    for sessions in &session_states[1..3] {
        assert!(namespace_cutover_authorization(sessions, &ready_events, false, None).is_err());
        assert_eq!(
            namespace_cutover_authorization(sessions, &ready_events, false, Some(&in_progress),)
                .unwrap(),
            NamespaceCutoverAuthorization::Proven
        );
    }

    assert!(matches!(
        namespace_cutover_authorization(&ready_sessions, &ready_events, false, Some(&in_progress),)
            .unwrap(),
        NamespaceCutoverAuthorization::RecordCompletion { .. }
    ));
}

#[test]
fn namespace_cutover_marker_accepts_only_supported_immutable_provenance() {
    let in_progress = doc! {
        "_id": NAMESPACE_CUTOVER_ID,
        "protocol": NAMESPACE_CUTOVER_PROTOCOL,
        "origin": "new-database",
        "phase": "in-progress",
    };
    assert_eq!(
        validate_namespace_cutover_marker(in_progress).unwrap(),
        NamespaceCutoverMarker::InProgress {
            origin: "new-database".into(),
        }
    );
    let complete = NamespaceCutoverMarker::Complete {
        origin: "operator-drained".into(),
        collection_ids: BTreeMap::from([
            (SESSIONS.to_owned(), test_collection_uuid(1)),
            (EVENTS.to_owned(), test_collection_uuid(2)),
        ]),
    };
    assert_eq!(
        validate_namespace_cutover_marker(namespace_cutover_marker_document(&complete).unwrap())
            .unwrap(),
        complete
    );
    for invalid in [
        doc! { "_id": NAMESPACE_CUTOVER_ID, "protocol": 2_i32, "origin": "new-database", "phase": "in-progress" },
        doc! { "_id": NAMESPACE_CUTOVER_ID, "protocol": NAMESPACE_CUTOVER_PROTOCOL, "origin": "assumed", "phase": "in-progress" },
        doc! { "_id": NAMESPACE_CUTOVER_ID, "protocol": NAMESPACE_CUTOVER_PROTOCOL, "origin": NAMESPACE_CUTOVER_OBSERVED_ORIGIN, "phase": "in-progress" },
        doc! { "_id": NAMESPACE_CUTOVER_ID, "protocol": NAMESPACE_CUTOVER_PROTOCOL, "origin": "new-database", "phase": "complete" },
        doc! { "_id": NAMESPACE_CUTOVER_ID, "protocol": NAMESPACE_CUTOVER_PROTOCOL, "origin": "new-database", "phase": "in-progress", "extra": true },
    ] {
        assert!(validate_namespace_cutover_marker(invalid).is_err());
    }
}

#[test]
fn completed_namespace_provenance_rejects_loss_and_replacement() {
    let ready = |authoritative: &str, uuid| NamespacePair {
        legacy: Some(NamespaceKind::View {
            source: Some(authoritative.into()),
            pipeline_is_empty: true,
        }),
        authoritative: Some(NamespaceKind::Collection),
        authoritative_marked: true,
        authoritative_fenced: true,
        authoritative_uuid: Some(uuid),
        ..NamespacePair::default()
    };
    let marker = NamespaceCutoverMarker::Complete {
        origin: "new-database".into(),
        collection_ids: BTreeMap::from([
            (SESSIONS.to_owned(), test_collection_uuid(1)),
            (EVENTS.to_owned(), test_collection_uuid(2)),
        ]),
    };

    let loss = namespace_cutover_authorization(
        &NamespacePair::default(),
        &NamespacePair::default(),
        false,
        Some(&marker),
    )
    .expect_err("completed provenance must not authorize empty-store recreation");
    assert!(loss.to_string().contains("no longer complete"));

    let replacement = namespace_cutover_authorization(
        &ready(SESSIONS, test_collection_uuid(3)),
        &ready(EVENTS, test_collection_uuid(2)),
        false,
        Some(&marker),
    )
    .expect_err("completed provenance must bind the authoritative UUIDs");
    assert!(replacement.to_string().contains("UUID no longer matches"));
}

#[test]
fn protocol_validators_require_the_current_marker_and_session_revision() {
    let sessions = session_protocol_validator();
    let schema = sessions
        .get_document("$jsonSchema")
        .expect("sessions use JSON schema validation");
    let required = schema
        .get_array("required")
        .expect("required fields are explicit");
    assert!(required.contains(&Bson::String(WRITE_PROTOCOL_FIELD.into())));
    assert!(required.contains(&Bson::String("revision".into())));
    assert_eq!(
        schema
            .get_document("properties")
            .unwrap()
            .get_document("revision")
            .unwrap()
            .get_str("bsonType")
            .unwrap(),
        "long"
    );

    for validator in [
        event_protocol_validator(),
        write_protocol_validator(),
        tombstone_protocol_validator(),
    ] {
        let schema = validator.get_document("$jsonSchema").unwrap();
        assert!(schema
            .get_array("required")
            .unwrap()
            .contains(&Bson::String(WRITE_PROTOCOL_FIELD.into())));
        assert_eq!(
            schema
                .get_document("properties")
                .unwrap()
                .get_document(WRITE_PROTOCOL_FIELD)
                .unwrap()
                .get_array("enum")
                .unwrap(),
            &[Bson::Int32(WRITE_PROTOCOL)]
        );
    }
}

fn test_collection_uuid(value: u8) -> Binary {
    Binary {
        subtype: mongodb::bson::spec::BinarySubtype::Uuid,
        bytes: vec![value; 16],
    }
}

fn exact_collection_contract(collection: &str, uuid: Binary) -> CollectionContract {
    CollectionContract {
        kind: NamespaceKind::Collection,
        validator: expected_validator(collection),
        validation_level: Some(ValidationLevel::Strict),
        validation_action: Some(ValidationAction::Error),
        uuid: Some(uuid),
    }
}

#[test]
fn validator_comparison_ignores_document_key_order_but_not_array_order_or_types() {
    let reordered = doc! {
        "$jsonSchema": {
            "properties": {
                "revision": { "bsonType": "long" },
                WRITE_PROTOCOL_FIELD: {
                    "enum": [WRITE_PROTOCOL],
                    "bsonType": "int",
                },
            },
            "required": [WRITE_PROTOCOL_FIELD, "revision"],
            "bsonType": "object",
        }
    };
    assert!(documents_equivalent(
        &session_protocol_validator(),
        &reordered
    ));
    let uuid = test_collection_uuid(0);
    let mut contract = exact_collection_contract(SESSIONS, uuid.clone());
    contract.validator = Some(reordered);
    validate_collection_contract(SESSIONS, &contract, Some(&uuid))
        .expect("document ordering does not weaken an exact validator");

    let reversed_array = doc! {
        "$jsonSchema": {
            "properties": {
                WRITE_PROTOCOL_FIELD: {
                    "bsonType": "int",
                    "enum": [WRITE_PROTOCOL],
                },
                "revision": { "bsonType": "long" },
            },
            "required": ["revision", WRITE_PROTOCOL_FIELD],
            "bsonType": "object",
        }
    };
    assert!(!documents_equivalent(
        &session_protocol_validator(),
        &reversed_array
    ));

    let wrong_bson_type = doc! {
        "$jsonSchema": {
            "bsonType": "object",
            "required": [WRITE_PROTOCOL_FIELD, "revision"],
            "properties": {
                WRITE_PROTOCOL_FIELD: {
                    "bsonType": "int",
                    "enum": [i64::from(WRITE_PROTOCOL)],
                },
                "revision": { "bsonType": "long" },
            },
        }
    };
    assert!(!documents_equivalent(
        &session_protocol_validator(),
        &wrong_bson_type
    ));
}

#[test]
fn validator_contract_rejects_missing_and_additional_clauses() {
    let uuid = test_collection_uuid(1);
    let mut missing = session_protocol_validator();
    missing
        .get_document_mut("$jsonSchema")
        .unwrap()
        .remove("required");
    let mut contract = exact_collection_contract(SESSIONS, uuid.clone());
    contract.validator = Some(missing);
    assert!(validate_collection_contract(SESSIONS, &contract, Some(&uuid)).is_err());

    let mut additional = session_protocol_validator();
    additional
        .get_document_mut("$jsonSchema")
        .unwrap()
        .insert("additionalProperties", false);
    contract.validator = Some(additional);
    assert!(validate_collection_contract(SESSIONS, &contract, Some(&uuid)).is_err());
}

#[test]
fn validator_contract_rejects_level_and_action_weakening() {
    let uuid = test_collection_uuid(2);
    let mut contract = exact_collection_contract(EVENTS, uuid.clone());
    contract.validation_level = Some(ValidationLevel::Moderate);
    assert!(validate_collection_contract(EVENTS, &contract, Some(&uuid)).is_err());

    contract = exact_collection_contract(EVENTS, uuid.clone());
    contract.validation_action = Some(ValidationAction::Warn);
    assert!(validate_collection_contract(EVENTS, &contract, Some(&uuid)).is_err());
}

#[test]
fn collection_uuid_replacement_is_rejected_even_with_the_exact_contract() {
    let original = test_collection_uuid(3);
    let replacement = test_collection_uuid(4);
    let contract = exact_collection_contract(WRITES, replacement);
    let error = validate_collection_contract(WRITES, &contract, Some(&original))
        .expect_err("a recreated authoritative collection is not the startup collection");
    assert!(error.to_string().contains("replaced after startup"));
}

fn required_event_index() -> RequiredIndex {
    required_indexes()
        .into_iter()
        .find(|required| required.collection == EVENTS)
        .expect("the event sequence index is required")
}

#[test]
fn exact_required_index_is_accepted_but_a_missing_one_is_rejected() {
    let required = required_event_index();
    let exact = required_index_model(&required);
    validate_required_index(&required, &[exact]).expect("the exact index is ready");
    assert!(validate_required_index(&required, &[]).is_err());
}

#[test]
fn event_sequence_index_cannot_be_changed_to_nonunique() {
    let required = required_event_index();
    let mut nonunique = required_index_model(&required);
    nonunique.options.as_mut().unwrap().unique = Some(false);
    let error = validate_required_index(&required, &[nonunique])
        .expect_err("duplicate sequence positions would become possible");
    assert!(error.to_string().contains("exact key and constraint"));
}

#[test]
fn required_indexes_reject_sparse_partial_hidden_ttl_and_collated_variants() {
    let required = required_event_index();

    let mut sparse = required_index_model(&required);
    sparse.options.as_mut().unwrap().sparse = Some(true);
    let mut partial = required_index_model(&required);
    partial.options.as_mut().unwrap().partial_filter_expression =
        Some(doc! { "seq": { "$gt": 0 } });
    let mut hidden = required_index_model(&required);
    hidden.options.as_mut().unwrap().hidden = Some(true);
    let mut ttl = required_index_model(&required);
    ttl.options.as_mut().unwrap().expire_after = Some(Duration::from_secs(60));
    let mut collated = required_index_model(&required);
    collated.options = Some(
        mongodb::bson::deserialize_from_document(doc! {
            "name": required.name,
            "unique": true,
            "collation": { "locale": "en" },
        })
        .expect("the driver's index options decode a collation"),
    );

    for weakened in [sparse, partial, hidden, ttl, collated] {
        assert!(validate_required_index(&required, &[weakened]).is_err());
    }
}

#[test]
fn required_index_name_and_compound_key_order_are_exact() {
    let required = required_event_index();
    let mut wrong_name = required_index_model(&required);
    wrong_name.options.as_mut().unwrap().name = Some("events_by_position".into());
    assert!(validate_required_index(&required, &[wrong_name]).is_err());

    let mut wrong_order = required_index_model(&required);
    wrong_order.keys = doc! { "seq": 1, "session_id": 1 };
    assert!(validate_required_index(&required, &[wrong_order]).is_err());
}

#[test]
fn namespace_mutation_authorization_requires_a_transaction_qualification_token() {
    let qualification = TransactionQualified;
    assert_eq!(
        qualification.authorize(NamespaceAction::RenameLegacy),
        NamespaceAction::RenameLegacy
    );
}

#[test]
fn current_session_documents_carry_the_write_protocol() {
    let stored = to_doc(&record_with(None)).expect("record encodes");
    let document = mongodb::bson::serialize_to_document(&stored).expect("record serializes");
    assert_eq!(
        document.get_i32(WRITE_PROTOCOL_FIELD).unwrap(),
        WRITE_PROTOCOL
    );
}

#[test]
fn read_aggregations_hide_tombstoned_sessions() {
    let query = SessionListQuery::new(PageRequest::new(5, None), None);
    let listed = session_page_pipeline(doc! { "owner": "subject" }, &query).unwrap();
    assert_eq!(listed[0], doc! { "$match": { "owner": "subject" } });
    assert_eq!(listed[1], doc! { "$sort": { "created_at": -1, "_id": -1 } });
    assert_eq!(listed[2], tombstone_lookup_stage());
    assert_eq!(
        listed[3],
        doc! { "$match": { "_cma_tombstones.0": { "$exists": false } } }
    );
    assert_eq!(listed[4], doc! { "$project": { "_cma_tombstones": 0 } });
    assert_eq!(listed[5], doc! { "$limit": 6_i64 });

    let counted = visible_count_pipeline(doc! { "status": "ready" });
    assert_eq!(counted[1], tombstone_lookup_stage());
    assert_eq!(
        counted.last().unwrap(),
        &doc! { "$group": { "_id": null, "count": { "$sum": 1_i64 } } }
    );
}

fn explain_cursor_stats(explain: &Document) -> &Document {
    explain
        .get_array("stages")
        .expect("aggregate explain carries stages")
        .iter()
        .filter_map(Bson::as_document)
        .find_map(|stage| stage.get_document("$cursor").ok())
        .and_then(|cursor| cursor.get_document("executionStats").ok())
        .expect("aggregate explain carries cursor execution statistics")
}

fn explain_count(stats: &Document, field: &str) -> i64 {
    match stats.get(field) {
        Some(Bson::Int32(value)) => i64::from(*value),
        Some(Bson::Int64(value)) => *value,
        other => panic!("execution statistic {field} is an integer, got {other:?}"),
    }
}

fn contains_string_field(value: &Bson, field: &str, expected: &str) -> bool {
    match value {
        Bson::Document(document) => document.iter().any(|(name, value)| {
            (name == field && value.as_str() == Some(expected))
                || contains_string_field(value, field, expected)
        }),
        Bson::Array(values) => values
            .iter()
            .any(|value| contains_string_field(value, field, expected)),
        _ => false,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at MongoDB"]
async fn real_session_page_uses_the_compound_index_before_tombstone_filtering() {
    const MATCHES: usize = 5_000;
    const EXAMINED_CEILING: i64 = 64;

    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name MongoDB");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database_name = format!("chrono_page_explain_{}_{}", std::process::id(), nonce);
    let database = client.database(&database_name);
    let sessions = database.collection::<Document>(SESSIONS);

    database
        .create_collection(TOMBSTONES)
        .await
        .expect("test tombstone collection creates");
    sessions
        .insert_many((0..MATCHES).map(|number| {
            doc! {
                "_id": format!("sesn_{number:05}"),
                "owner": "subject-page-explain",
                "status": "running",
                "created_at": "2026-08-25T00:00:00Z",
            }
        }))
        .await
        .expect("test sessions seed");

    let required = required_indexes()
        .into_iter()
        .find(|required| required.name == "owner_1_created_at_-1__id_-1")
        .expect("owner pagination index is required");
    sessions
        .create_index(required_index_model(&required))
        .await
        .expect("owner pagination index creates");

    let query = SessionListQuery::new(PageRequest::new(5, None), None);
    let pipeline = session_page_pipeline(doc! { "owner": "subject-page-explain" }, &query)
        .expect("page pipeline builds");
    let explain = database
        .run_command(doc! {
            "explain": {
                "aggregate": SESSIONS,
                "pipeline": pipeline,
                "cursor": {},
            },
            "verbosity": "executionStats",
        })
        .await
        .expect("page aggregation explains");

    let stats = explain_cursor_stats(&explain);
    let keys = explain_count(stats, "totalKeysExamined");
    let documents = explain_count(stats, "totalDocsExamined");
    let uses_index =
        contains_string_field(&Bson::Document(explain.clone()), "indexName", required.name);

    database.drop().await.expect("temporary database drops");

    assert!(uses_index, "page explain did not use {}", required.name);
    assert!(
        keys <= EXAMINED_CEILING,
        "five-row page examined {keys} keys from {MATCHES} matches"
    );
    assert!(
        documents <= EXAMINED_CEILING,
        "five-row page examined {documents} documents from {MATCHES} matches"
    );
}

#[test]
fn raw_append_touches_the_session_without_lowering_a_legacy_tail() {
    assert_eq!(
        appended_event_session_update(&doc! { "last_seq": 9_i64 }, 7),
        doc! {
            "$set": {
                "_cma_write_protocol": WRITE_PROTOCOL,
                "revision": 0_i64,
            },
            "$max": { "last_seq": 7_i64 },
        }
    );
    assert_eq!(
        appended_event_session_update(&doc! { "last_seq": 4_i64, "revision": 3_i64 }, 5),
        doc! {
            "$set": { "_cma_write_protocol": WRITE_PROTOCOL },
            "$max": { "last_seq": 5_i64 },
        }
    );
}

#[test]
fn post_fence_event_validation_detects_internal_and_initial_gaps() {
    let mut sequence = StoredEventSequence::default();
    sequence
        .observe(&doc! { "session_id": "s-1", "seq": 1_i64 })
        .unwrap();
    sequence
        .observe(&doc! { "session_id": "s-1", "seq": 2_i64 })
        .unwrap();
    sequence
        .observe(&doc! { "session_id": "s-2", "seq": 1_i64 })
        .unwrap();

    let internal = sequence
        .observe(&doc! { "session_id": "s-2", "seq": 3_i64 })
        .expect_err("sequence two is missing");
    assert!(internal.to_string().contains("expected 2, found 3"));

    let mut initial = StoredEventSequence::default();
    assert!(initial
        .observe(&doc! { "session_id": "s-3", "seq": 2_i64 })
        .is_err());
    assert!(StoredEventSequence::default()
        .observe(&doc! { "session_id": "s-4", "seq": -1_i64 })
        .is_err());
    assert!(StoredEventSequence::default()
        .observe(&doc! { "session_id": "s-5", "seq": "one" })
        .is_err());
}

#[test]
fn history_snapshots_reject_tail_regression_and_untombstoned_deletion() {
    let previous = HistorySnapshot {
        tombstones: HashSet::new(),
        session_tails: BTreeMap::from([("s-1".into(), 3)]),
        event_tails: BTreeMap::from([("s-1".into(), 3)]),
    };

    let lower_record = HistorySnapshot {
        session_tails: BTreeMap::from([("s-1".into(), 2)]),
        ..previous.clone()
    };
    assert!(validate_history_progress(&previous, &lower_record).is_err());

    let lower_events = HistorySnapshot {
        event_tails: BTreeMap::from([("s-1".into(), 2)]),
        ..previous.clone()
    };
    assert!(validate_history_progress(&previous, &lower_events).is_err());

    assert!(validate_history_progress(&previous, &HistorySnapshot::default()).is_err());
}

#[test]
fn history_snapshots_allow_only_tombstoned_deletion_and_never_tombstone_loss() {
    let previous = HistorySnapshot {
        tombstones: HashSet::from(["already-deleted".into()]),
        session_tails: BTreeMap::from([("s-1".into(), 3)]),
        event_tails: BTreeMap::from([("s-1".into(), 3)]),
    };
    let deleted = HistorySnapshot {
        tombstones: HashSet::from(["already-deleted".into(), "s-1".into()]),
        ..HistorySnapshot::default()
    };
    validate_history_progress(&previous, &deleted)
        .expect("a permanent tombstone authorizes removing the record and history");

    let lost_tombstone = HistorySnapshot {
        tombstones: HashSet::from(["s-1".into()]),
        ..HistorySnapshot::default()
    };
    assert!(validate_history_progress(&previous, &lost_tombstone).is_err());
}

#[test]
fn a_held_legacy_upsert_waits_for_its_late_event_then_resumes_from_the_event_spill() {
    let id = "sesn_held_flush".to_owned();
    let sessions = BTreeMap::from([(id.clone(), 2)]);

    let mut first = event_doc::to_document(&event(&id, 1)).unwrap();
    first.remove(WRITE_PROTOCOL_FIELD);
    let mut late = event_doc::to_document(&event(&id, 2)).unwrap();
    late.remove(WRITE_PROTOCOL_FIELD);

    let event_tails = |documents: Vec<Document>| {
        let mut positions = BTreeMap::new();
        for document in documents {
            let event = normalize_cutover_event_document(document).unwrap();
            positions.insert((event.session_id, event.seq), event.document);
        }
        let mut sequence = StoredEventSequence::default();
        let mut tails = BTreeMap::new();
        for document in positions.into_values() {
            let (session_id, seq) = sequence.observe(&document).unwrap();
            tails.insert(session_id, seq);
        }
        tails
    };

    let protected_before_append = event_tails(vec![first.clone()]);
    let waiting = validate_legacy_writer_drain(&sessions, &protected_before_append)
        .expect_err("the advertised second event has not reached protected history");
    assert!(waiting.to_string().contains("currently ends at 1"));

    let protected_with_spill = event_tails(vec![first, late.clone()]);
    validate_legacy_writer_drain(&sessions, &protected_with_spill)
        .expect("the held flush resumes once its late event is captured");
    let normalized = normalize_cutover_event_document(late).unwrap();
    assert_eq!(
        normalized.document.get_i32(WRITE_PROTOCOL_FIELD).unwrap(),
        WRITE_PROTOCOL
    );
    validate_same_cutover_event(&normalized.document, &normalized.document)
        .expect("replaying the same late event is idempotent");

    validate_legacy_writer_drain(
        &BTreeMap::from([(id.clone(), 1)]),
        &BTreeMap::from([(id, 1)]),
    )
    .expect("a record-only legacy update does not invent an event to wait for");
}

#[test]
fn events_first_cutover_finishes_a_held_legacy_delete_with_a_tombstone() {
    let id = "sesn_held_delete".to_owned();
    let sessions = BTreeMap::from([(id.clone(), 1)]);
    let events = BTreeMap::from([(id.clone(), 1)]);
    validate_legacy_pre_cutover_owners(&sessions, &events, &HashSet::new())
        .expect("the delete has not started at preflight");

    let sessions_after_first_delete = BTreeMap::new();
    assert!(validate_legacy_pre_cutover_owners(
        &sessions_after_first_delete,
        &events,
        &HashSet::new(),
    )
    .is_err());
    assert!(cutover_delete_requires_reconciliation(false, false));
    assert!(!cutover_delete_requires_reconciliation(true, false));

    let before = HistorySnapshot {
        tombstones: HashSet::new(),
        session_tails: sessions,
        event_tails: events,
    };
    let reconciled = HistorySnapshot {
        tombstones: HashSet::from([id]),
        ..HistorySnapshot::default()
    };
    validate_history_progress(&before, &reconciled)
        .expect("transactional reconciliation preserves the legacy delete as a tombstone");
}

#[test]
fn cached_audit_failure_keeps_readiness_closed_until_a_successful_reaudit() {
    let checked_at = Instant::now();
    let mut audit = IntegrityAudit::default();
    audit.record_success(checked_at, HistorySnapshot::default());
    audit.record_failure(
        checked_at + Duration::from_secs(1),
        StoreError::Malformed("first audit failure".into()),
    );
    audit.record_failure(
        checked_at + Duration::from_secs(2),
        StoreError::Malformed("later audit failure".into()),
    );

    let cached = audit
        .cached_result(checked_at + Duration::from_secs(3))
        .expect("the failed audit is still fresh")
        .expect_err("readiness remains closed");
    assert!(cached.to_string().contains("first audit failure"));

    audit.record_success(
        checked_at + Duration::from_secs(4),
        HistorySnapshot::default(),
    );
    audit
        .cached_result(checked_at + Duration::from_secs(5))
        .expect("the successful re-audit is cached")
        .expect("a successful re-audit reopens readiness");
}

#[test]
fn post_fence_session_validation_allows_legacy_revision_zero_but_not_bad_tails() {
    let stored = to_doc(&record_with(None)).unwrap();
    let mut legacy = mongodb::bson::serialize_to_document(&stored).unwrap();
    legacy.remove("revision");
    legacy.remove(WRITE_PROTOCOL_FIELD);
    assert_eq!(validate_stored_session_document(&legacy).unwrap().1, 7);

    legacy.insert("last_seq", -1_i64);
    assert!(validate_stored_session_document(&legacy).is_err());
    legacy.insert("last_seq", 7_i64);
    legacy.insert("revision", 1_i32);
    assert!(validate_stored_session_document(&legacy).is_err());
}

#[test]
fn reservation_query_has_a_status_index() {
    let required = required_indexes()
        .into_iter()
        .find(|required| required.name == "status_1")
        .expect("the reservation status index is required");
    let index = required_index_model(&required);
    assert_eq!(index.keys, doc! { "status": 1 });
    assert_eq!(
        index.options.and_then(|options| options.name),
        Some("status_1".into())
    );
}

#[tokio::test]
async fn readiness_probe_accepts_a_healthy_result() {
    bounded_readiness_probe(
        Arc::new(tokio::sync::Semaphore::new(1)),
        async { Ok(()) },
        Duration::from_millis(10),
    )
    .await
    .expect("a completed probe is healthy");
}

#[tokio::test(start_paused = true)]
async fn readiness_stages_share_one_overall_deadline() {
    let permit = Arc::new(tokio::sync::Semaphore::new(1));
    let stages = Arc::new(AtomicUsize::new(0));
    let stages_in_probe = Arc::clone(&stages);
    let started_at = tokio::time::Instant::now();

    let error = bounded_readiness_probe(
        Arc::clone(&permit),
        async move {
            stages_in_probe.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(1_250)).await;
            stages_in_probe.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(1_250)).await;
            stages_in_probe.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
        Duration::from_secs(2),
    )
    .await
    .expect_err("the second stage exceeds the one overall budget");

    assert!(error.to_string().contains("timed out after 2000 ms"));
    assert_eq!(
        tokio::time::Instant::now() - started_at,
        Duration::from_secs(2)
    );
    assert_eq!(stages.load(Ordering::SeqCst), 2);
    assert_eq!(permit.available_permits(), 0);

    tokio::time::advance(Duration::from_millis(500)).await;
    tokio::task::yield_now().await;
    assert_eq!(stages.load(Ordering::SeqCst), 3);
    assert_eq!(permit.available_permits(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_timed_out_readiness_probe_stays_single_flight_until_its_driver_work_finishes() {
    let permit = Arc::new(tokio::sync::Semaphore::new(1));
    let invocations = Arc::new(AtomicUsize::new(0));
    let pending = Arc::new(AtomicUsize::new(0));
    let (started, wait_for_started) = tokio::sync::oneshot::channel();
    let (release, wait_for_release) = tokio::sync::oneshot::channel();
    let (finished, wait_for_finished) = tokio::sync::oneshot::channel();
    let first_invocations = Arc::clone(&invocations);
    let first_pending = Arc::clone(&pending);
    let first_permit = Arc::clone(&permit);
    let first = tokio::spawn(async move {
        bounded_readiness_probe(
            first_permit,
            async move {
                first_invocations.fetch_add(1, Ordering::SeqCst);
                first_pending.fetch_add(1, Ordering::SeqCst);
                started.send(()).expect("the test receives probe start");
                wait_for_release.await.expect("test releases the probe");
                first_pending.fetch_sub(1, Ordering::SeqCst);
                finished
                    .send(())
                    .expect("the test receives probe completion");
                Ok(())
            },
            Duration::from_secs(2),
        )
        .await
    });

    wait_for_started.await.expect("the first probe starts");
    let error = first
        .await
        .expect("the readiness caller task does not fail")
        .expect_err("the held driver work reaches readiness's deadline");

    assert!(matches!(error, StoreError::Unavailable(_)));
    assert!(error.to_string().contains("timed out after 2000 ms"));
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
    assert_eq!(pending.load(Ordering::SeqCst), 1);
    assert_eq!(permit.available_permits(), 0);

    for _ in 0..8 {
        let candidate_invocations = Arc::clone(&invocations);
        let candidate = bounded_readiness_probe(
            Arc::clone(&permit),
            async move {
                candidate_invocations.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            Duration::from_secs(2),
        )
        .await
        .expect_err("a held probe closes readiness without queueing");
        assert!(candidate.to_string().contains("already in progress"));
    }

    assert_eq!(invocations.load(Ordering::SeqCst), 1);
    assert_eq!(pending.load(Ordering::SeqCst), 1);

    release
        .send(())
        .expect("the timed-out probe remains alive with its permit");
    wait_for_finished
        .await
        .expect("the detached probe is still polled to completion");
    tokio::task::yield_now().await;
    assert_eq!(pending.load(Ordering::SeqCst), 0);
    assert_eq!(permit.available_permits(), 1);

    let recovered_invocations = Arc::clone(&invocations);
    bounded_readiness_probe(
        Arc::clone(&permit),
        async move {
            recovered_invocations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
        Duration::from_secs(2),
    )
    .await
    .expect("readiness recovers after the detached work finishes");
    assert_eq!(invocations.load(Ordering::SeqCst), 2);
}

#[test]
fn session_pages_fetch_one_sentinel_row_and_never_exceed_the_store_bound() {
    let five = SessionListQuery::new(PageRequest::new(5, None), None);
    let oversized = SessionListQuery::new(PageRequest::new(u32::MAX, None), None);

    assert_eq!(session_page_fetch_limit(&five), 6);
    assert_eq!(oversized.page.limit(), 200);
    assert_eq!(session_page_fetch_limit(&oversized), 201);
}

#[test]
fn session_page_filter_pushes_status_and_both_cursor_keys_into_mongo() {
    let query = SessionListQuery::new(
        PageRequest::new(
            50,
            Some(PageCursor::new("2026-08-24T10:11:12Z", "sesn_tied")),
        ),
        Some(SessionStatus::Running),
    );

    assert_eq!(
        session_page_filter(doc! { "owner": "subject-a" }, &query).unwrap(),
        doc! {
            "owner": "subject-a",
            "status": "running",
            "$or": [
                { "created_at": { "$lt": "2026-08-24T10:11:12Z" } },
                {
                    "created_at": "2026-08-24T10:11:12Z",
                    "_id": { "$lt": "sesn_tied" },
                },
            ],
        }
    );
    assert_eq!(session_page_sort(), doc! { "created_at": -1, "_id": -1 });
}

#[test]
fn session_page_indexes_cover_owned_and_admin_queries_with_status() {
    for (name, keys) in [
        (
            "owner_1_created_at_-1__id_-1",
            doc! { "owner": 1, "created_at": -1, "_id": -1 },
        ),
        (
            "owner_1_status_1_created_at_-1__id_-1",
            doc! { "owner": 1, "status": 1, "created_at": -1, "_id": -1 },
        ),
        ("created_at_-1__id_-1", doc! { "created_at": -1, "_id": -1 }),
        (
            "status_1_created_at_-1__id_-1",
            doc! { "status": 1, "created_at": -1, "_id": -1 },
        ),
    ] {
        let required = required_indexes()
            .into_iter()
            .find(|required| required.collection == SESSIONS && required.name == name)
            .unwrap_or_else(|| panic!("missing required pagination index {name}"));
        assert_eq!(required.keys, keys);
        assert!(!required.unique);
    }
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

async fn collection_contract_snapshot(
    database: &Database,
    names: &[&str],
) -> BTreeMap<String, CollectionContract> {
    let mut cursor = database
        .list_collections()
        .filter(doc! { "name": { "$in": names.to_vec() } })
        .await
        .expect("test namespace specifications list");
    let mut contracts = BTreeMap::new();
    while let Some(specification) = cursor
        .try_next()
        .await
        .expect("test namespace specification reads")
    {
        contracts.insert(
            specification.name.clone(),
            collection_contract(&specification),
        );
    }
    contracts
}

async fn sorted_collection_documents(database: &Database, collection: &str) -> Vec<Document> {
    let mut cursor = database
        .collection::<Document>(collection)
        .find(doc! {})
        .sort(doc! { "_id": 1 })
        .await
        .expect("test collection reads");
    let mut documents = Vec::new();
    while let Some(document) = cursor.try_next().await.expect("test document reads") {
        documents.push(document);
    }
    documents
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_default_cutover_refusal_does_not_mutate_writable_legacy_namespaces() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database_name = format!("chrono_cutover_gate_{}_{}", std::process::id(), nonce);
    let database = client.database(&database_name);

    let mut record = record_with(None);
    record.session.id = SessionId("sesn_cutover_gate".into());
    record.session.last_seq = 1;
    let mut legacy_session =
        mongodb::bson::serialize_to_document(&to_doc(&record).unwrap()).unwrap();
    legacy_session.remove("revision");
    legacy_session.remove(WRITE_PROTOCOL_FIELD);
    database
        .collection::<Document>(LEGACY_SESSIONS)
        .insert_one(legacy_session)
        .await
        .unwrap();
    let mut legacy_event = event_doc::to_document(&event("sesn_cutover_gate", 1)).unwrap();
    legacy_event.remove(WRITE_PROTOCOL_FIELD);
    database
        .collection::<Document>(LEGACY_EVENTS)
        .insert_one(legacy_event)
        .await
        .unwrap();

    let mut names_before = database.list_collection_names().await.unwrap();
    names_before.sort();
    let contracts_before =
        collection_contract_snapshot(&database, &[LEGACY_SESSIONS, LEGACY_EVENTS]).await;
    let sessions_before = sorted_collection_documents(&database, LEGACY_SESSIONS).await;
    let events_before = sorted_collection_documents(&database, LEGACY_EVENTS).await;

    let error = match MongoSessionStore::open(&client, &database_name, false).await {
        Ok(_) => panic!("writable legacy collections require the explicit offline cutover"),
        Err(error) => error,
    };
    assert!(error
        .to_string()
        .contains("CHRONO_MONGODB_LEGACY_CUTOVER=drained"));

    let mut names_after = database.list_collection_names().await.unwrap();
    names_after.sort();
    assert_eq!(names_after, names_before);
    assert_eq!(
        collection_contract_snapshot(&database, &[LEGACY_SESSIONS, LEGACY_EVENTS]).await,
        contracts_before
    );
    assert_eq!(
        sorted_collection_documents(&database, LEGACY_SESSIONS).await,
        sessions_before
    );
    assert_eq!(
        sorted_collection_documents(&database, LEGACY_EVENTS).await,
        events_before
    );
    for absent in [NAMESPACE_CUTOVERS, TOMBSTONES, SESSIONS, EVENTS] {
        assert!(!names_after.iter().any(|name| name == absent));
    }

    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_marker_backed_partial_new_database_bootstrap_resumes_without_an_opt_in() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database_name = format!("chrono_bootstrap_resume_{}_{}", std::process::id(), nonce);
    let database = client.database(&database_name);
    database
        .collection::<Document>(NAMESPACE_CUTOVERS)
        .insert_one(doc! {
            "_id": NAMESPACE_CUTOVER_ID,
            "protocol": NAMESPACE_CUTOVER_PROTOCOL,
            "origin": "new-database",
            "phase": "in-progress",
        })
        .await
        .unwrap();
    database.create_collection(EVENTS).await.unwrap();

    let store = MongoSessionStore::open(&client, &database_name, false)
        .await
        .expect("durable new-database provenance resumes the partial bootstrap");
    for (legacy, authoritative) in [(LEGACY_SESSIONS, SESSIONS), (LEGACY_EVENTS, EVENTS)] {
        let pair = store.namespace_pair(legacy, authoritative).await.unwrap();
        assert_eq!(
            next_namespace_action(&pair, legacy, authoritative).unwrap(),
            NamespaceAction::Ready
        );
    }
    MongoSessionStore::open(&client, &database_name, false)
        .await
        .expect("completed views remain restartable without an opt-in");
    assert!(matches!(
        store.namespace_cutover_marker().await.unwrap(),
        Some(NamespaceCutoverMarker::Complete { .. })
    ));

    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_cutover_resume_after_session_rename_preserves_owner_and_history() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database_name = format!(
        "chrono_session_rename_resume_{}_{}",
        std::process::id(),
        nonce
    );
    let database = client.database(&database_name);
    let id = SessionId("sesn_session_rename_resume".into());

    database
        .collection::<Document>(NAMESPACE_CUTOVERS)
        .insert_one(doc! {
            "_id": NAMESPACE_CUTOVER_ID,
            "protocol": NAMESPACE_CUTOVER_PROTOCOL,
            "origin": "operator-drained",
            "phase": "in-progress",
        })
        .await
        .unwrap();

    let mut record = record_with(None);
    record.session.id = id.clone();
    record.session.last_seq = 1;
    let mut legacy_session =
        mongodb::bson::serialize_to_document(&to_doc(&record).unwrap()).unwrap();
    legacy_session.remove("revision");
    legacy_session.remove(WRITE_PROTOCOL_FIELD);
    database
        .collection::<Document>(SESSIONS)
        .insert_one(legacy_session)
        .await
        .unwrap();

    let mut legacy_event = event_doc::to_document(&event(&id.0, 1)).unwrap();
    legacy_event.remove(WRITE_PROTOCOL_FIELD);
    database
        .collection::<Document>(EVENTS)
        .insert_one(legacy_event)
        .await
        .unwrap();
    for authoritative in [SESSIONS, EVENTS] {
        database
            .run_command(doc! {
                "collMod": authoritative,
                "validator": expected_validator(authoritative).unwrap(),
                "validationLevel": "strict",
                "validationAction": "error",
            })
            .await
            .unwrap();
    }
    database
        .create_collection(LEGACY_EVENTS)
        .view_on(EVENTS)
        .pipeline(Vec::<Document>::new())
        .await
        .unwrap();

    let store = MongoSessionStore::open(&client, &database_name, false)
        .await
        .expect("cutover resumes after the session rename without inferring a delete");
    assert!(store.get(&id).await.unwrap().is_some());
    assert_eq!(store.event_tail(&id).await.unwrap(), 1);
    assert!(!store.is_tombstoned(&id).await.unwrap());
    assert_eq!(
        next_namespace_action(
            &store
                .namespace_pair(LEGACY_SESSIONS, SESSIONS)
                .await
                .unwrap(),
            LEGACY_SESSIONS,
            SESSIONS,
        )
        .unwrap(),
        NamespaceAction::Ready
    );
    assert_eq!(
        database
            .collection::<Document>(SESSIONS)
            .count_documents(doc! { "_id": &id.0 })
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        database
            .collection::<Document>(EVENTS)
            .count_documents(doc! { "session_id": &id.0 })
            .await
            .unwrap(),
        1
    );

    database.drop().await.unwrap();
}

async fn drop_session_namespace_pairs(database: &Database) {
    for collection in [LEGACY_SESSIONS, LEGACY_EVENTS, SESSIONS, EVENTS] {
        database
            .collection::<Document>(collection)
            .drop()
            .await
            .unwrap_or_else(|error| panic!("drop test namespace {collection}: {error}"));
    }
}

async fn create_fenced_session_namespace_pairs(database: &Database) {
    for authoritative in [SESSIONS, EVENTS] {
        database.create_collection(authoritative).await.unwrap();
        database
            .run_command(doc! {
                "collMod": authoritative,
                "validator": expected_validator(authoritative).unwrap(),
                "validationLevel": "strict",
                "validationAction": "error",
            })
            .await
            .unwrap();
    }
    for (legacy, authoritative) in [(LEGACY_SESSIONS, SESSIONS), (LEGACY_EVENTS, EVENTS)] {
        database
            .create_collection(legacy)
            .view_on(authoritative)
            .pipeline(Vec::<Document>::new())
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_completed_cutover_provenance_rejects_namespace_loss_and_replacement() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database_name = format!(
        "chrono_completed_cutover_guard_{}_{}",
        std::process::id(),
        nonce
    );
    let database = client.database(&database_name);

    let store = MongoSessionStore::open(&client, &database_name, false)
        .await
        .expect("a fresh store completes its namespace provenance");
    let marker_before = database
        .collection::<Document>(NAMESPACE_CUTOVERS)
        .find_one(doc! { "_id": NAMESPACE_CUTOVER_ID })
        .await
        .unwrap()
        .expect("completed provenance is durable");
    let completed = validate_namespace_cutover_marker(marker_before.clone()).unwrap();
    assert!(matches!(completed, NamespaceCutoverMarker::Complete { .. }));
    drop(store);

    drop_session_namespace_pairs(&database).await;
    create_fenced_session_namespace_pairs(&database).await;
    let replacement_error = match MongoSessionStore::open(&client, &database_name, false).await {
        Ok(_) => panic!("replacement collections must not inherit completed provenance"),
        Err(error) => error,
    };
    assert!(replacement_error
        .to_string()
        .contains("UUID no longer matches"));
    assert_eq!(
        database
            .collection::<Document>(NAMESPACE_CUTOVERS)
            .find_one(doc! { "_id": NAMESPACE_CUTOVER_ID })
            .await
            .unwrap()
            .unwrap(),
        marker_before
    );

    drop_session_namespace_pairs(&database).await;
    let mut names_before = database.list_collection_names().await.unwrap();
    names_before.sort();
    let loss_error = match MongoSessionStore::open(&client, &database_name, false).await {
        Ok(_) => panic!("lost completed namespaces must not become a fresh empty store"),
        Err(error) => error,
    };
    assert!(loss_error.to_string().contains("no longer complete"));
    let mut names_after = database.list_collection_names().await.unwrap();
    names_after.sort();
    assert_eq!(names_after, names_before);
    assert_eq!(
        database
            .collection::<Document>(NAMESPACE_CUTOVERS)
            .find_one(doc! { "_id": NAMESPACE_CUTOVER_ID })
            .await
            .unwrap()
            .unwrap(),
        marker_before
    );

    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn the_real_store_refuses_a_forward_gap_without_committing_it() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database = format!("chrono_gap_refusal_{}_{}", std::process::id(), nonce);
    let store = MongoSessionStore::open(&client, &database, false)
        .await
        .expect("test store opens");
    let id = SessionId("sesn_real_gap".into());
    let mut seed = record_with(None);
    seed.session.id = id.clone();
    seed.session.last_seq = 0;
    store
        .create_session_idempotent("seed-real-gap", &seed, &[])
        .await
        .expect("live session is seeded");

    let error = store
        .append_events(&[event(&id.0, 2)])
        .await
        .expect_err("sequence one is not durable");

    assert!(matches!(
        error,
        StoreError::NonContiguousAppend {
            expected: 1,
            got: 2
        }
    ));
    assert!(store.events_since(&id, 0).await.unwrap().is_empty());
    client
        .database(&database)
        .drop()
        .await
        .expect("temporary database drops");
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

    let mut original = record_with(None);
    original.session.last_seq = 0;
    let stored = to_doc(&original).expect("record encodes");
    let mut legacy = mongodb::bson::serialize_to_document(&stored).expect("record serializes");
    assert!(legacy.remove("revision").is_some());
    assert!(legacy.remove(WRITE_PROTOCOL_FIELD).is_some());
    client
        .database(&database)
        .collection::<Document>(LEGACY_SESSIONS)
        .insert_one(legacy)
        .await
        .expect("legacy document is seeded before the writer fence");
    let store = MongoSessionStore::open(&client, &database, true)
        .await
        .expect("test store opens and fences the legacy namespace");

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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_STANDALONE_URI pointing at a standalone MongoDB"]
async fn a_real_standalone_is_rejected_before_the_store_reports_ready() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_STANDALONE_URI")
        .expect("CHRONO_TEST_MONGODB_STANDALONE_URI must name a standalone MongoDB");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database = format!("chrono_standalone_refusal_{}_{}", std::process::id(), nonce);

    let error = match MongoSessionStore::open(&client, &database, false).await {
        Ok(_) => panic!("a standalone cannot support session transactions"),
        Err(error) => error,
    };

    assert!(matches!(error, StoreError::Unavailable(_)));
    assert!(error.to_string().contains("replica set or mongos"));
    assert!(client
        .database(&database)
        .list_collection_names()
        .await
        .expect("standalone namespace list succeeds")
        .is_empty());
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_legacy_names_are_readable_views_but_refuse_every_old_mutation() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database = format!("chrono_legacy_view_fence_{}_{}", std::process::id(), nonce);
    let db = client.database(&database);

    let mut original = record_with(None);
    original.session.id = SessionId("sesn_view_fence".into());
    original.session.last_seq = 1;
    let mut legacy_session = mongodb::bson::serialize_to_document(&to_doc(&original).unwrap())
        .expect("record serializes");
    legacy_session.remove("revision");
    legacy_session.remove(WRITE_PROTOCOL_FIELD);
    db.collection::<Document>(LEGACY_SESSIONS)
        .insert_one(legacy_session)
        .await
        .expect("legacy session is seeded");
    let mut legacy_event = event_doc::to_document(&event("sesn_view_fence", 1)).unwrap();
    legacy_event.remove(WRITE_PROTOCOL_FIELD);
    db.collection::<Document>(LEGACY_EVENTS)
        .insert_one(legacy_event)
        .await
        .expect("legacy event is seeded");

    let store = MongoSessionStore::open(&client, &database, true)
        .await
        .expect("cutover succeeds");
    let legacy_sessions = db.collection::<Document>(LEGACY_SESSIONS);
    let legacy_events = db.collection::<Document>(LEGACY_EVENTS);
    assert!(legacy_sessions
        .find_one(doc! { "_id": "sesn_view_fence" })
        .await
        .expect("legacy view reads")
        .is_some());
    assert!(legacy_events
        .find_one(doc! { "session_id": "sesn_view_fence", "seq": 1_i64 })
        .await
        .expect("legacy event view reads")
        .is_some());

    let mut advanced = store
        .get(&original.session.id)
        .await
        .unwrap()
        .expect("migrated session reads");
    advanced.context_ref = Some("revision-one-survives".into());
    assert_eq!(
        store
            .commit_session_idempotent("advance-before-old-replace", &advanced, &[])
            .await
            .unwrap(),
        1
    );
    let mut old_replacement =
        mongodb::bson::serialize_to_document(&to_doc(&original).unwrap()).unwrap();
    old_replacement.remove("revision");
    old_replacement.remove(WRITE_PROTOCOL_FIELD);

    assert!(legacy_sessions
        .insert_one(doc! { "_id": "old-insert" })
        .await
        .is_err());
    assert!(legacy_sessions
        .replace_one(doc! { "_id": "sesn_view_fence" }, old_replacement)
        .with_options(ReplaceOptions::builder().upsert(true).build())
        .await
        .is_err());
    assert!(legacy_sessions
        .delete_one(doc! { "_id": "sesn_view_fence" })
        .await
        .is_err());
    assert!(legacy_events
        .insert_one(doc! {
            "session_id": "sesn_view_fence",
            "seq": 2_i64,
            "kind": { "type": "session_created" },
        })
        .await
        .is_err());

    let durable = store.get(&original.session.id).await.unwrap().unwrap();
    assert_eq!(durable.revision, 1);
    assert_eq!(
        durable.context_ref.as_deref(),
        Some("revision-one-survives")
    );
    assert_eq!(store.event_tail(&original.session.id).await.unwrap(), 1);
    client
        .database(&database)
        .drop()
        .await
        .expect("temporary database drops");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_real_tombstoned_session_cannot_be_recreated_or_appended_to() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database = format!("chrono_tombstone_{}_{}", std::process::id(), nonce);
    let store = MongoSessionStore::open(&client, &database, false)
        .await
        .expect("test store opens");
    let mut record = record_with(None);
    record.session.id = SessionId("sesn_tombstoned".into());
    record.session.last_seq = 0;
    store
        .create_session_idempotent("create-before-delete", &record, &[])
        .await
        .expect("session creates");
    store
        .delete(&record.session.id)
        .await
        .expect("session deletes");

    let create = store
        .create_session_idempotent("create-after-delete", &record, &[])
        .await
        .expect_err("tombstone blocks create");
    assert!(matches!(create, StoreError::SessionDeleted { .. }));
    assert!(matches!(
        store.append_events(&[event("sesn_tombstoned", 1)]).await,
        Err(StoreError::SessionDeleted { .. })
    ));
    assert!(store
        .database
        .collection::<Document>(LEGACY_SESSIONS)
        .replace_one(
            doc! { "_id": &record.session.id.0 },
            doc! { "_id": &record.session.id.0, "last_seq": 0_i64 },
        )
        .with_options(ReplaceOptions::builder().upsert(true).build())
        .await
        .is_err());
    assert!(store.get(&record.session.id).await.unwrap().is_none());
    assert!(store
        .events_since(&record.session.id, 0)
        .await
        .unwrap()
        .is_empty());

    client
        .database(&database)
        .drop()
        .await
        .expect("temporary database drops");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_cutover_rejects_an_advertised_hole_but_allows_physical_history_ahead() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();

    let ahead_database = format!("chrono_tail_ahead_{}_{}", std::process::id(), nonce);
    let ahead = client.database(&ahead_database);
    let mut ahead_record = record_with(None);
    ahead_record.session.id = SessionId("sesn_tail_ahead".into());
    ahead_record.session.last_seq = 2;
    let mut ahead_doc =
        mongodb::bson::serialize_to_document(&to_doc(&ahead_record).unwrap()).unwrap();
    ahead_doc.remove("revision");
    ahead_doc.remove(WRITE_PROTOCOL_FIELD);
    ahead
        .collection::<Document>(LEGACY_SESSIONS)
        .insert_one(ahead_doc)
        .await
        .unwrap();
    let mut first = event_doc::to_document(&event("sesn_tail_ahead", 1)).unwrap();
    first.remove(WRITE_PROTOCOL_FIELD);
    ahead
        .collection::<Document>(LEGACY_EVENTS)
        .insert_one(first)
        .await
        .unwrap();

    let error = match MongoSessionStore::open(&client, &ahead_database, true).await {
        Ok(_) => panic!("an advertised second event cannot survive with only event one"),
        Err(error) => error,
    };
    assert!(matches!(error, StoreError::Unavailable(_)));
    assert!(error.to_string().contains("advertises event 2"));
    ahead.drop().await.unwrap();

    let behind_database = format!("chrono_tail_behind_{}_{}", std::process::id(), nonce);
    let behind = client.database(&behind_database);
    let mut behind_record = record_with(None);
    behind_record.session.id = SessionId("sesn_tail_behind".into());
    behind_record.session.last_seq = 1;
    let mut behind_doc =
        mongodb::bson::serialize_to_document(&to_doc(&behind_record).unwrap()).unwrap();
    behind_doc.remove("revision");
    behind_doc.remove(WRITE_PROTOCOL_FIELD);
    behind
        .collection::<Document>(LEGACY_SESSIONS)
        .insert_one(behind_doc)
        .await
        .unwrap();
    let mut history = Vec::new();
    for seq in [1, 2] {
        let mut document = event_doc::to_document(&event("sesn_tail_behind", seq)).unwrap();
        document.remove(WRITE_PROTOCOL_FIELD);
        history.push(document);
    }
    behind
        .collection::<Document>(LEGACY_EVENTS)
        .insert_many(history)
        .await
        .unwrap();

    let store = MongoSessionStore::open(&client, &behind_database, true)
        .await
        .expect("a physical tail ahead of the record is recoverable");
    assert_eq!(
        store
            .event_tail(&SessionId("sesn_tail_behind".into()))
            .await
            .unwrap(),
        2
    );
    behind.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_namespace_cutover_resumes_known_states_and_rejects_ambiguous_ones() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();

    let partial_name = format!("chrono_cutover_partial_{}_{}", std::process::id(), nonce);
    let partial = client.database(&partial_name);
    partial.create_collection(SESSIONS).await.unwrap();
    let store = MongoSessionStore::open(&client, &partial_name, true)
        .await
        .expect("rename-complete/view-missing state resumes");
    MongoSessionStore::open(&client, &partial_name, false)
        .await
        .expect("a complete cutover is idempotent");
    partial
        .collection::<Document>(LEGACY_SESSIONS)
        .drop()
        .await
        .unwrap();
    partial.create_collection("wrong_sessions").await.unwrap();
    partial
        .create_collection(LEGACY_SESSIONS)
        .view_on("wrong_sessions")
        .pipeline(Vec::<Document>::new())
        .await
        .unwrap();
    assert!(matches!(
        store.ping().await,
        Err(StoreError::Unavailable(_))
    ));
    partial.drop().await.unwrap();

    let ambiguous_name = format!("chrono_cutover_ambiguous_{}_{}", std::process::id(), nonce);
    let ambiguous = client.database(&ambiguous_name);
    ambiguous.create_collection(LEGACY_SESSIONS).await.unwrap();
    ambiguous.create_collection(SESSIONS).await.unwrap();
    assert!(MongoSessionStore::open(&client, &ambiguous_name, true)
        .await
        .is_err());
    ambiguous.drop().await.unwrap();

    let wrong_name = format!("chrono_cutover_wrong_view_{}_{}", std::process::id(), nonce);
    let wrong = client.database(&wrong_name);
    wrong.create_collection(SESSIONS).await.unwrap();
    wrong.create_collection("wrong_sessions").await.unwrap();
    wrong
        .create_collection(LEGACY_SESSIONS)
        .view_on("wrong_sessions")
        .pipeline(Vec::<Document>::new())
        .await
        .unwrap();
    assert!(MongoSessionStore::open(&client, &wrong_name, true)
        .await
        .is_err());
    wrong.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_real_late_receipt_failure_aborts_events_and_record_replacement_too() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database = format!("chrono_atomic_abort_{}_{}", std::process::id(), nonce);
    let store = MongoSessionStore::open(&client, &database, false)
        .await
        .expect("test store opens");
    let mut original = record_with(None);
    original.session.id = SessionId("sesn_atomic_abort".into());
    original.session.last_seq = 0;
    store
        .create_session_idempotent("atomic-abort-seed", &original, &[])
        .await
        .unwrap();
    store
        .database
        .run_command(doc! {
            "collMod": WRITES,
            "validator": { "required_only_for_abort_test": { "$exists": true } },
            "validationLevel": "strict",
            "validationAction": "error",
        })
        .await
        .expect("test forces the final receipt insert to fail validation");

    let mut replacement = store.get(&original.session.id).await.unwrap().unwrap();
    replacement.session.last_seq = 1;
    assert!(store
        .commit_session_idempotent(
            "atomic-abort-must-fail",
            &replacement,
            &[event("sesn_atomic_abort", 1)],
        )
        .await
        .is_err());

    let durable = store.get(&original.session.id).await.unwrap().unwrap();
    assert_eq!(durable.revision, 1);
    assert_eq!(durable.session.last_seq, 0);
    assert!(store
        .events_since(&original.session.id, 0)
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .writes
        .find_one(doc! { "_id": "atomic-abort-must-fail" })
        .await
        .unwrap()
        .is_none());
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_real_raw_append_and_delete_serialize_without_leaving_orphan_events() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database = format!("chrono_append_delete_race_{}_{}", std::process::id(), nonce);
    let store = MongoSessionStore::open(&client, &database, false)
        .await
        .expect("test store opens");

    for attempt in 0..16 {
        let id = SessionId(format!("sesn_append_delete_{attempt}"));
        let mut record = record_with(None);
        record.session.id = id.clone();
        record.session.last_seq = 0;
        store
            .create_session_idempotent(&format!("race-create-{attempt}"), &record, &[])
            .await
            .unwrap();

        let append_store = store.clone();
        let delete_store = store.clone();
        let append_id = id.clone();
        let delete_id = id.clone();
        let (append, delete) = tokio::join!(
            async move { append_store.append_events(&[event(&append_id.0, 1)]).await },
            async move { delete_store.delete(&delete_id).await }
        );
        assert!(matches!(
            append,
            Ok(()) | Err(StoreError::SessionDeleted { .. })
        ));
        delete.expect("delete commits");
        assert_eq!(
            store
                .events
                .count_documents(doc! { "session_id": &id.0 })
                .await
                .unwrap(),
            0,
            "attempt {attempt} left an event behind"
        );
        assert!(store
            .sessions
            .find_one(doc! { "_id": &id.0 })
            .await
            .unwrap()
            .is_none());
        assert!(store.is_tombstoned(&id).await.unwrap());
    }

    MongoSessionStore::open(&client, &database, false)
        .await
        .expect("the post-fence audit accepts every race result");
    client.database(&database).drop().await.unwrap();
}
