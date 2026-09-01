use super::*;
use layer2_core::{
    store::{PendingHandover, PendingHandoverTarget},
    AgentOptions, ChatHostId, ChatOrigin, ConversationLink, ConversationWriteAuthority, Event,
    EventKind, GrantedSkill, KernelId, PageCursor, PageRequest, Rate, RateCardId, RecoveryState,
    RuntimeCleanupReason, Session, SessionBoundary, SessionCapabilities, SessionId,
    SessionLeaseRequest, SessionListQuery, SessionMode, SessionOperation, SessionPricing,
    SessionRecord, SessionStatus, SessionSurfaceFilter, SkillId, SkillPack, SkillPackDigest,
    SkillPackState, StopReason, StoredEventKind, SubjectId, TerminalGeometry,
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
        chat: None,
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
fn private_chat_origin_round_trips_and_derives_the_public_projection() {
    let mut expected = record_with(None);
    let origin = ChatOrigin {
        host_id: ChatHostId("chho_round_trip".into()),
        worktree_name: "chat-7".into(),
        worktree_path: "/workspace/chat-7".into(),
        paused_notice: Some(
            "Your current chat session has been idle for 15min and has been auto-paused.".into(),
        ),
    };
    expected.chat_origin = Some(origin.clone());
    expected.session.chat = Some(origin.public_projection());

    let stored = to_doc(&expected).expect("chat origin can be stored");
    let document = mongodb::bson::serialize_to_document(&stored).expect("stored doc serializes");
    assert!(document.contains_key("chat_origin"));
    assert!(
        !document.contains_key("chat"),
        "public projection is derived on read"
    );

    let read = from_doc(stored).expect("chat origin can be read");
    assert_eq!(read.chat_origin, Some(origin.clone()));
    assert_eq!(read.session.chat, Some(origin.public_projection()));
}

#[test]
fn a_legacy_chat_origin_without_a_paused_notice_reads_as_unpaused() {
    let mut expected = record_with(None);
    let origin = ChatOrigin {
        host_id: ChatHostId("chho_legacy_notice".into()),
        worktree_name: "chat-2".into(),
        worktree_path: "/workspace/chat-2".into(),
        paused_notice: Some("this field did not exist on the legacy writer".into()),
    };
    expected.chat_origin = Some(origin.clone());
    expected.session.chat = Some(origin.public_projection());
    let stored = to_doc(&expected).expect("chat origin can be stored");
    let mut document =
        mongodb::bson::serialize_to_document(&stored).expect("stored doc serializes");
    assert!(document
        .get_document_mut("chat_origin")
        .expect("private chat origin is nested")
        .remove("paused_notice")
        .is_some());
    let legacy: SessionDoc =
        mongodb::bson::deserialize_from_bson(document.into()).expect("legacy doc reads");

    let read = from_doc(legacy).expect("legacy record decodes");
    assert_eq!(
        read.chat_origin
            .as_ref()
            .and_then(|origin| origin.paused_notice.as_deref()),
        None
    );
    assert_eq!(
        read.session
            .chat
            .as_ref()
            .and_then(|chat| chat.paused_notice.as_deref()),
        None
    );
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
fn pending_handovers_round_trip_with_their_exact_target_shape() {
    let targets = [
        PendingHandoverTarget::RuntimeVersion {
            from_runtime_version: None,
            runtime_version: "runtime:v2".into(),
        },
        PendingHandoverTarget::Plane {
            from_mode: SessionMode::Terminal,
            mode: SessionMode::Conversation,
            capabilities: SessionCapabilities {
                conversation: true,
                snapshot: true,
                steer: true,
                resources: Some(false),
                ..Default::default()
            },
        },
    ];

    for target in targets {
        let mut expected = record_with(None);
        expected.pending_handover = Some(PendingHandover {
            claim: "rotn_recoverable".into(),
            snapshot_ref: "sessions/sesn_priced/handover.snapshot".into(),
            snapshot_at: "2026-08-27T01:02:03Z".into(),
            trigger_recovery: None,
            rotations_from: 4,
            rotations_to: 5,
            target,
        });

        let stored = to_doc(&expected).expect("pending handover can be stored");
        let read = from_doc(stored).expect("pending handover can be read");

        assert_eq!(read.pending_handover, expected.pending_handover);
    }
}

#[test]
fn pending_handover_bson_uses_the_versioned_nested_field_names() {
    let capabilities = SessionCapabilities {
        conversation: true,
        snapshot: true,
        steer: true,
        resources: Some(false),
        ..Default::default()
    };
    let cases = [
        (
            PendingHandoverTarget::RuntimeVersion {
                from_runtime_version: Some("runtime:v2".into()),
                runtime_version: "runtime:v3".into(),
            },
            mongodb::bson::doc! {
                "kind": "runtime_version",
                "from_runtime_version": "runtime:v2",
                "runtime_version": "runtime:v3",
            },
        ),
        (
            PendingHandoverTarget::Plane {
                from_mode: SessionMode::Terminal,
                mode: SessionMode::Conversation,
                capabilities,
            },
            mongodb::bson::doc! {
                "kind": "plane",
                "from_mode": "terminal",
                "mode": "conversation",
                "capabilities": mongodb::bson::serialize_to_bson(&capabilities)
                    .expect("capabilities serialize"),
            },
        ),
    ];

    for (pending_target, expected_target) in cases {
        let mut expected = record_with(None);
        expected.pending_handover = Some(PendingHandover {
            claim: "rotn_wire".into(),
            snapshot_ref: "sessions/sesn_priced/handover.snapshot".into(),
            snapshot_at: "2026-08-27T01:02:03Z".into(),
            trigger_recovery: None,
            rotations_from: 7,
            rotations_to: 8,
            target: pending_target,
        });

        let stored = to_doc(&expected).expect("pending handover can be stored");
        let document = mongodb::bson::serialize_to_document(&stored).expect("doc serializes");
        let handover = document
            .get_document("pending_handover")
            .expect("pending handover is nested");
        assert_eq!(handover.len(), 6, "handover wire fields changed");
        assert_eq!(
            handover.get_str("claim").expect("claim is a string"),
            "rotn_wire"
        );
        assert_eq!(
            handover
                .get_str("snapshot_ref")
                .expect("snapshot reference is a string"),
            "sessions/sesn_priced/handover.snapshot"
        );
        assert_eq!(
            handover
                .get_str("snapshot_at")
                .expect("snapshot time is a string"),
            "2026-08-27T01:02:03Z"
        );
        assert_eq!(
            handover
                .get_i64("rotations_from")
                .expect("source rotation count is an integer"),
            7
        );
        assert_eq!(
            handover
                .get_i64("rotations_to")
                .expect("target rotation count is an integer"),
            8
        );
        assert_eq!(
            handover.get_document("target").expect("target is nested"),
            &expected_target,
            "tagged handover target shape changed"
        );
    }
}

#[test]
fn a_pending_handover_can_survive_after_its_live_rotation_claim_is_cleared() {
    let mut expected = record_with(None);
    expected.rotation_claim = None;
    expected.pending_handover = Some(PendingHandover {
        claim: "rotn_revive_later".into(),
        snapshot_ref: "sessions/sesn_priced/handover.snapshot".into(),
        snapshot_at: "2026-08-27T01:02:03Z".into(),
        trigger_recovery: None,
        rotations_from: 8,
        rotations_to: 9,
        target: PendingHandoverTarget::RuntimeVersion {
            from_runtime_version: Some("runtime:v1".into()),
            runtime_version: "runtime:v2".into(),
        },
    });

    let stored = to_doc(&expected).expect("pending handover can be stored without a live claim");
    let read = from_doc(stored).expect("pending handover can be recovered without a live claim");

    assert_eq!(read.rotation_claim, None);
    assert_eq!(read.pending_handover, expected.pending_handover);
}

#[test]
fn a_document_without_pending_handover_reads_as_having_no_recovery_snapshot() {
    let mut current = record_with(None);
    current.pending_handover = Some(PendingHandover {
        claim: "rotn_not_present_in_legacy".into(),
        snapshot_ref: "sessions/sesn_priced/handover.snapshot".into(),
        snapshot_at: "2026-08-27T01:02:03Z".into(),
        trigger_recovery: None,
        rotations_from: 2,
        rotations_to: 3,
        target: PendingHandoverTarget::RuntimeVersion {
            from_runtime_version: Some("runtime:v1".into()),
            runtime_version: "runtime:v2".into(),
        },
    });
    let stored = to_doc(&current).expect("record can be stored");
    let mut document = mongodb::bson::serialize_to_document(&stored).expect("doc serializes");
    assert!(document.remove("pending_handover").is_some());
    let legacy: SessionDoc =
        mongodb::bson::deserialize_from_bson(document.into()).expect("legacy doc reads");

    assert_eq!(
        from_doc(legacy)
            .expect("legacy record decodes")
            .pending_handover,
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

    let legacy: SessionDoc =
        mongodb::bson::deserialize_from_bson(document.into()).expect("legacy doc reads");

    assert_eq!(from_doc(legacy).expect("legacy record decodes").revision, 0);
}

#[test]
fn a_document_without_coordination_reads_as_exact_legacy_authority() {
    let mut record = record_with(None);
    record.runtime_id = Some("runtime-legacy".into());
    let stored = to_doc(&record).expect("record can be stored");
    let mut document = mongodb::bson::serialize_to_document(&stored).expect("doc serializes");
    assert!(document.remove("coordination").is_some());
    let legacy: SessionDoc =
        mongodb::bson::deserialize_from_bson(document.into()).expect("legacy doc reads");

    assert_eq!(
        runtime_authority_from_doc(&legacy).expect("legacy authority is valid"),
        RuntimeAuthority {
            generation: 0,
            runtime_id: Some("runtime-legacy".into()),
        }
    );
}

#[test]
fn staged_skill_bytes_round_trip_as_bson_binary() {
    let mut record = record_with(None);
    record.skill_packs = SkillPackState::Staged(vec![SkillPack {
        name: "private-review".into(),
        version: Some("3.2".into()),
        files: vec![("SKILL.md".into(), b"private-body-marker".to_vec())],
    }]);

    let stored = to_doc(&record).expect("staged record encodes");
    let document = mongodb::bson::serialize_to_document(&stored).expect("document encodes");
    let body = document
        .get_document("skill_packs")
        .expect("custody is a subdocument")
        .get_array("packs")
        .expect("staged packs are present")[0]
        .as_document()
        .expect("pack is a document")
        .get_array("files")
        .expect("files are present")[0]
        .as_document()
        .expect("file is a document")
        .get_binary_generic("body")
        .expect("body is BSON binary");
    assert_eq!(body, b"private-body-marker");

    let read = from_doc(stored).expect("staged record decodes");
    assert_eq!(read.skill_packs, record.skill_packs);
}

#[test]
fn staged_skill_bytes_are_redacted_from_session_doc_debug_output() {
    let mut record = record_with(None);
    record.skill_packs = SkillPackState::Staged(vec![SkillPack {
        name: "private-review".into(),
        version: None,
        files: vec![("SKILL.md".into(), b"private-body-marker".to_vec())],
    }]);

    let debug = format!("{:?}", to_doc(&record).expect("record encodes"));
    assert!(debug.contains("bytes: 19"), "got {debug}");
    assert!(!debug.contains("112, 114, 105, 118, 97, 116, 101"));
}

#[test]
fn legacy_handover_skill_custody_round_trips_exact_bytes_receipt_and_source_generation() {
    let packs = vec![SkillPack {
        name: "private-review".into(),
        version: Some("4.1".into()),
        files: vec![("SKILL.md".into(), b"private-handover-body-marker".to_vec())],
    }];
    let digest = SkillPackDigest::for_packs(&packs);
    let mut record = record_with(None);
    record.runtime_id = Some("runtime-private-h1".into());
    record.skill_packs = SkillPackState::LegacyHandover {
        source_runtime_id: "runtime-private-h1".into(),
        source_generation: 41,
        packs,
        stored_digest: Some(digest),
    };

    let stored = to_doc(&record).expect("legacy handover record encodes");
    let document = mongodb::bson::serialize_to_document(&stored).expect("document encodes");
    let custody = document
        .get_document("skill_packs")
        .expect("custody is a subdocument");
    assert_eq!(custody.get_str("state").unwrap(), "legacy_handover");
    assert_eq!(
        custody.get_str("source_runtime_id").unwrap(),
        "runtime-private-h1"
    );
    assert_eq!(custody.get_i64("source_generation").unwrap(), 41);
    assert_eq!(
        custody.get_binary_generic("stored_digest").unwrap(),
        digest.as_bytes()
    );
    assert_eq!(
        custody.get_array("packs").unwrap()[0]
            .as_document()
            .unwrap()
            .get_array("files")
            .unwrap()[0]
            .as_document()
            .unwrap()
            .get_binary_generic("body")
            .unwrap(),
        b"private-handover-body-marker"
    );
    assert_eq!(
        from_doc(stored)
            .expect("legacy handover record decodes")
            .skill_packs,
        record.skill_packs
    );
}

#[test]
fn interactive_legacy_handover_requires_its_exact_source_runtime() {
    let packs = vec![SkillPack {
        name: "private-review".into(),
        version: Some("4.1".into()),
        files: vec![("SKILL.md".into(), b"exact candidate".to_vec())],
    }];
    let mut record = record_with(None);
    record.runtime_id = Some("runtime-h2".into());
    record.skill_packs = SkillPackState::LegacyHandover {
        source_runtime_id: "runtime-h1".into(),
        source_generation: 7,
        stored_digest: Some(SkillPackDigest::for_packs(&packs)),
        packs,
    };

    let ready = to_doc(&record).expect("the malformed persisted shape still serializes");
    assert!(matches!(from_doc(ready), Err(StoreError::Malformed(_))));

    record.session.status = SessionStatus::Rotating;
    let rotating = from_doc(to_doc(&record).expect("rotating H2 serializes"))
        .expect("a switched H2 may retain handover custody until Ready settlement");
    assert_eq!(rotating.runtime_id.as_deref(), Some("runtime-h2"));
    assert!(matches!(
        rotating.skill_packs,
        SkillPackState::LegacyHandover { .. }
    ));
}

#[test]
fn malformed_legacy_handover_skill_custody_is_rejected() {
    let packs = vec![SkillPack {
        name: "private-review".into(),
        version: Some("4.1".into()),
        files: vec![("SKILL.md".into(), b"exact candidate".to_vec())],
    }];
    let docs = skill_packs_to_docs(&packs);
    let valid_digest = skill_pack_digest_to_binary(SkillPackDigest::for_packs(&packs));
    let state = |source_runtime_id: &str, source_generation: i64, stored_digest: Option<Binary>| {
        SkillPackStateDoc::LegacyHandover {
            source_runtime_id: source_runtime_id.into(),
            source_generation,
            packs: docs.clone(),
            stored_digest,
        }
    };

    assert!(matches!(
        skill_pack_state_from_doc(Some(state("", 1, Some(valid_digest.clone())))),
        Err(StoreError::Malformed(_))
    ));
    assert!(matches!(
        skill_pack_state_from_doc(Some(state("runtime-h1", -1, Some(valid_digest.clone())))),
        Err(StoreError::Malformed(_))
    ));
    assert!(matches!(
        skill_pack_state_from_doc(Some(state(
            "runtime-h1",
            1,
            Some(Binary {
                subtype: mongodb::bson::spec::BinarySubtype::Generic,
                bytes: vec![0; 31],
            })
        ))),
        Err(StoreError::Malformed(_))
    ));
    assert!(matches!(
        skill_pack_state_from_doc(Some(state(
            "runtime-h1",
            1,
            Some(skill_pack_digest_to_binary(SkillPackDigest::for_packs(&[])))
        ))),
        Err(StoreError::Malformed(_))
    ));

    let invalid_tree = SkillPackStateDoc::LegacyHandover {
        source_runtime_id: "runtime-h1".into(),
        source_generation: 1,
        packs: skill_packs_to_docs(&[SkillPack {
            name: "private-review".into(),
            version: None,
            files: vec![("../SKILL.md".into(), Vec::new())],
        }]),
        stored_digest: None,
    };
    assert!(matches!(
        skill_pack_state_from_doc(Some(invalid_tree)),
        Err(StoreError::Malformed(_))
    ));
}

#[test]
fn legacy_handover_listing_redacts_candidate_bytes_and_source_identity() {
    let body_marker = b"private-list-handover-body-marker";
    let source_marker = b"runtime-private-list-h1";
    let packs = vec![SkillPack {
        name: "private-review".into(),
        version: Some("4.1".into()),
        files: vec![("SKILL.md".into(), body_marker.to_vec())],
    }];
    let mut record = record_with(None);
    record.session.status = SessionStatus::Rotating;
    record.runtime_id = Some("runtime-public-h2".into());
    record.skill_packs = SkillPackState::LegacyHandover {
        source_runtime_id: String::from_utf8(source_marker.to_vec()).unwrap(),
        source_generation: 7,
        stored_digest: Some(SkillPackDigest::for_packs(&packs)),
        packs,
    };
    let stored = to_doc(&record).expect("exact handover record encodes");
    assert_eq!(
        from_doc(stored.clone())
            .expect("exact get remains authoritative")
            .skill_packs,
        record.skill_packs
    );

    let mut projected =
        mongodb::bson::serialize_to_document(&stored).expect("exact record serializes");
    projected.insert(
        "skill_packs",
        mongodb::bson::serialize_to_bson(&SkillPackStateDoc::StagedSummary)
            .expect("summary serializes"),
    );
    let projected_bytes = mongodb::bson::serialize_to_vec(&projected).unwrap();
    assert!(!projected_bytes
        .windows(body_marker.len())
        .any(|window| window == body_marker));
    assert!(!projected_bytes
        .windows(source_marker.len())
        .any(|window| window == source_marker));
    let listed: SessionDoc = mongodb::bson::deserialize_from_document(projected)
        .expect("projected row retains the session shape");
    assert_eq!(
        from_list_doc(listed)
            .expect("projected handover row decodes for listing")
            .skill_packs,
        SkillPackState::StagedSummary
    );
    assert_eq!(
        staged_skill_summary_stage(),
        doc! {
            "$set": {
                "skill_packs": {
                    "$cond": [
                        { "$in": ["$skill_packs.state", ["staged", "legacy_handover"]] },
                        { "state": "staged_summary" },
                        "$skill_packs",
                    ]
                }
            }
        }
    );
}

#[test]
fn legacy_handover_skills_cannot_push_the_actual_bson_document_over_its_bound() {
    let mut record = record_with(None);
    record.skill_packs = SkillPackState::LegacyHandover {
        source_runtime_id: "runtime-h1".into(),
        source_generation: 1,
        packs: vec![SkillPack {
            name: "too-large".into(),
            version: None,
            files: vec![(
                "SKILL.md".into(),
                vec![0; MAX_STAGED_SESSION_DOCUMENT_BYTES],
            )],
        }],
        stored_digest: None,
    };

    let error = to_doc(&record).expect_err("oversized handover BSON is refused");
    assert!(matches!(error, StoreError::Malformed(_)));
    assert!(error
        .to_string()
        .contains("session BSON with staged skills"));
}

#[test]
fn a_projected_staged_skill_row_excludes_bodies_and_preserves_cleanup_custody() {
    let mut record = record_with(None);
    record.skill_packs = SkillPackState::Staged(vec![SkillPack {
        name: "private-review".into(),
        version: Some("3.2".into()),
        files: vec![("SKILL.md".into(), b"private-list-body-marker".to_vec())],
    }]);

    let stored = to_doc(&record).expect("exact staged record encodes");
    let mut projected =
        mongodb::bson::serialize_to_document(&stored).expect("exact record serializes");
    projected.insert(
        "skill_packs",
        mongodb::bson::serialize_to_bson(&SkillPackStateDoc::StagedSummary)
            .expect("summary serializes"),
    );

    let projected_bytes = mongodb::bson::serialize_to_vec(&projected).unwrap();
    assert!(!projected_bytes
        .windows(b"private-list-body-marker".len())
        .any(|window| window == b"private-list-body-marker"));
    assert_eq!(
        projected
            .get_document("skill_packs")
            .expect("summary is a document"),
        &doc! { "state": "staged_summary" }
    );

    let listed: SessionDoc = mongodb::bson::deserialize_from_document(projected.clone())
        .expect("projected row retains the session shape");
    let listed = from_list_doc(listed).expect("projected row decodes for listing");
    assert_eq!(listed.session.id, record.session.id);
    assert_eq!(listed.session.granted_skills, record.session.granted_skills);
    assert_eq!(listed.skill_packs, SkillPackState::StagedSummary);

    let exact: SessionDoc = mongodb::bson::deserialize_from_document(projected).unwrap();
    assert!(matches!(from_doc(exact), Err(StoreError::Malformed(_))));
}

#[test]
fn a_projected_staged_skill_summary_cannot_be_written_as_authority() {
    let mut record = record_with(None);
    record.skill_packs = SkillPackState::StagedSummary;

    let error = to_doc(&record).expect_err("listing projection is not durable state");
    assert!(matches!(error, StoreError::Malformed(_)));
    assert!(error.to_string().contains("cannot be persisted"));
}

#[test]
fn stored_skill_digest_round_trips_as_fixed_bson_binary() {
    let mut record = record_with(None);
    let digest = SkillPackDigest::for_packs(&[SkillPack {
        name: "private-review".into(),
        version: Some("3.2".into()),
        files: vec![("SKILL.md".into(), b"private-body-marker".to_vec())],
    }]);
    record.skill_packs = SkillPackState::Stored { digest };

    let stored = to_doc(&record).expect("stored record encodes");
    let document = mongodb::bson::serialize_to_document(&stored).expect("document encodes");
    assert_eq!(
        document
            .get_document("skill_packs")
            .expect("custody is a subdocument")
            .get_binary_generic("digest")
            .expect("digest is BSON binary"),
        digest.as_bytes()
    );
    assert_eq!(
        from_doc(stored).expect("stored record decodes").skill_packs,
        record.skill_packs
    );
}

#[test]
fn malformed_stored_skill_digests_are_refused() {
    let wrong_subtype = SkillPackStateDoc::Stored {
        digest: Binary {
            subtype: mongodb::bson::spec::BinarySubtype::Uuid,
            bytes: vec![0; 32],
        },
    };
    assert!(matches!(
        skill_pack_state_from_doc(Some(wrong_subtype)),
        Err(StoreError::Malformed(_))
    ));

    let wrong_length = SkillPackStateDoc::Stored {
        digest: Binary {
            subtype: mongodb::bson::spec::BinarySubtype::Generic,
            bytes: vec![0; 31],
        },
    };
    assert!(matches!(
        skill_pack_state_from_doc(Some(wrong_length)),
        Err(StoreError::Malformed(_))
    ));
    assert!(
        mongodb::bson::deserialize_from_document::<SkillPackStateDoc>(doc! {
            "state": "stored"
        })
        .is_err()
    );
}

#[test]
fn a_document_without_skill_custody_reads_as_legacy() {
    let mut record = record_with(None);
    record.skill_packs = SkillPackState::Stored {
        digest: SkillPackDigest::for_packs(&[]),
    };
    let stored = to_doc(&record).expect("record encodes");
    let mut document = mongodb::bson::serialize_to_document(&stored).expect("document encodes");
    assert!(document.remove("skill_packs").is_some());
    let legacy: SessionDoc =
        mongodb::bson::deserialize_from_bson(document.into()).expect("legacy document reads");

    assert_eq!(
        from_doc(legacy).expect("legacy record decodes").skill_packs,
        SkillPackState::Legacy
    );
}

#[test]
fn staged_skills_cannot_push_the_actual_bson_document_over_its_bound() {
    let mut record = record_with(None);
    record.skill_packs = SkillPackState::Staged(vec![SkillPack {
        name: "too-large".into(),
        version: None,
        files: vec![(
            "SKILL.md".into(),
            vec![0; MAX_STAGED_SESSION_DOCUMENT_BYTES],
        )],
    }]);

    let error = to_doc(&record).expect_err("oversized staged BSON is refused");
    assert!(matches!(error, StoreError::Malformed(_)));
    assert!(error
        .to_string()
        .contains("session BSON with staged skills"));
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
fn ordinary_mongo_event_inputs_reject_runtime_started() {
    let mut record = record_with(None);
    record.session.id = SessionId("sesn_reserved_runtime_marker".into());
    record.session.last_seq = 1;
    let marker = Event {
        session_id: record.session.id.clone(),
        seq: 1,
        kind: EventKind::RuntimeStarted {
            generation: std::num::NonZeroU64::new(1).unwrap(),
        },
        appended_at: "2026-08-24T00:00:00Z".into(),
    };

    assert!(matches!(
        validate_ordinary_event_batch(std::slice::from_ref(&marker)),
        Err(StoreError::Malformed(message)) if message.contains("runtime-authority coordination")
    ));
    for create in [false, true] {
        assert!(matches!(
            validate_transaction_input(
                "write-reserved-runtime-marker",
                &record,
                std::slice::from_ref(&marker),
                create,
            ),
            Err(StoreError::Malformed(message)) if message.contains("runtime-authority coordination")
        ));
    }

    let compatibility_marker = Event {
        session_id: record.session.id.clone(),
        seq: 1,
        kind: EventKind::RuntimeIncarnationStarted,
        appended_at: "2026-08-24T00:00:00Z".into(),
    };
    validate_ordinary_event_batch(std::slice::from_ref(&compatibility_marker))
        .expect("the compatibility marker uses an ordinary atomic commit");
    validate_transaction_input(
        "write-compatibility-runtime-marker",
        &record,
        std::slice::from_ref(&compatibility_marker),
        false,
    )
    .expect("the compatibility marker is not the reserved coordinated marker");
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
fn runtime_entitlement_includes_only_the_idle_approval_wait() {
    assert_eq!(
        runtime_entitlement_filter().expect("the core lifecycle values serialize"),
        doc! {
            "$or": [
                {
                    "status": {
                        "$in": ["provisioning", "ready", "running", "rotating"]
                    }
                },
                { "status": "idle", "stop_reason": "requires_action" },
            ]
        }
    );
}

#[test]
fn reserved_capacity_pipeline_matches_and_sums_only_durable_runtime_promises() {
    let pipeline = reserved_capacity_pipeline().expect("the core statuses serialize");
    assert_eq!(
        pipeline,
        vec![
            doc! {
                "$match": {
                    "chat_origin": { "$exists": false },
                    "$or": [
                        {
                            "status": {
                                "$in": ["provisioning", "ready", "running", "rotating"]
                            }
                        },
                        { "status": "idle", "stop_reason": "requires_action" },
                    ]
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
        PRE_COORDINATION_SESSIONS,
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
        Some(PRE_COORDINATION_SESSIONS)
    );

    let ready = NamespacePair {
        legacy: Some(NamespaceKind::View {
            source: Some(PRE_COORDINATION_SESSIONS.into()),
            pipeline_is_empty: true,
        }),
        authoritative: Some(NamespaceKind::Collection),
        authoritative_fenced: true,
        ..NamespacePair::default()
    };
    assert_eq!(
        cutover_session_owner_namespace(&ready).unwrap(),
        Some(PRE_COORDINATION_SESSIONS)
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
        source_ids: test_namespace_cutover_source_ids(Some(3), Some(4), None),
        tombstone_id: None,
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
    let sessions = ready(PRE_COORDINATION_SESSIONS, test_collection_uuid(1));
    let events = ready(EVENTS, test_collection_uuid(2));
    let collection_ids = BTreeMap::from([
        (
            PRE_COORDINATION_SESSIONS.to_owned(),
            test_collection_uuid(1),
        ),
        (EVENTS.to_owned(), test_collection_uuid(2)),
    ]);
    assert!(
        namespace_cutover_authorization(&sessions, &events, false, None)
            .expect_err("markerless protected namespaces must not gain completion provenance")
            .to_string()
            .contains("markerless protected namespaces")
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
        collection_ids: BTreeMap::from([
            (
                PRE_COORDINATION_SESSIONS.to_owned(),
                test_collection_uuid(1),
            ),
            (EVENTS.to_owned(), test_collection_uuid(2)),
            (TOMBSTONES.to_owned(), test_collection_uuid(5)),
        ]),
    };
    assert_eq!(
        namespace_cutover_authorization(&sessions, &events, false, Some(&completed)).unwrap(),
        NamespaceCutoverAuthorization::Complete,
    );
}

#[test]
fn staged_legacy_cutover_accepts_only_a_raw_unfenced_source_without_provenance() {
    let raw_sessions = NamespacePair {
        legacy: Some(NamespaceKind::Collection),
        legacy_uuid: Some(test_collection_uuid(1)),
        ..NamespacePair::default()
    };
    let raw_events = NamespacePair {
        legacy: Some(NamespaceKind::Collection),
        legacy_uuid: Some(test_collection_uuid(2)),
        ..NamespacePair::default()
    };
    let raw_contract = |uuid| CollectionContract {
        kind: NamespaceKind::Collection,
        validator: None,
        validation_level: None,
        validation_action: None,
        uuid: Some(uuid),
    };
    let raw_contracts = BTreeMap::from([
        (
            LEGACY_SESSIONS.to_owned(),
            raw_contract(test_collection_uuid(1)),
        ),
        (
            LEGACY_EVENTS.to_owned(),
            raw_contract(test_collection_uuid(2)),
        ),
    ]);
    validate_raw_legacy_stage_source(&raw_sessions, &raw_events, &raw_contracts)
        .expect("two raw legacy collections are an eligible staged source");
    validate_raw_legacy_stage_source(
        &raw_sessions,
        &NamespacePair::default(),
        &BTreeMap::from([(
            LEGACY_SESSIONS.to_owned(),
            raw_contract(test_collection_uuid(1)),
        )]),
    )
    .expect("one raw legacy collection is enough to establish an existing database");

    let fresh = validate_raw_legacy_stage_source(
        &NamespacePair::default(),
        &NamespacePair::default(),
        &BTreeMap::new(),
    )
    .expect_err("a fresh database is not a recovery boundary");
    assert!(fresh.to_string().contains("at least one raw legacy"));

    for changed in [
        CollectionContract {
            validator: Some(doc! { "unexpected": true }),
            ..raw_contract(test_collection_uuid(1))
        },
        CollectionContract {
            validation_level: Some(ValidationLevel::Off),
            ..raw_contract(test_collection_uuid(1))
        },
        CollectionContract {
            validation_action: Some(ValidationAction::Warn),
            ..raw_contract(test_collection_uuid(1))
        },
    ] {
        let error = validate_raw_legacy_stage_source(
            &raw_sessions,
            &NamespacePair::default(),
            &BTreeMap::from([(LEGACY_SESSIONS.to_owned(), changed)]),
        )
        .expect_err("an unknown raw collection option must never be overwritten");
        assert!(error.to_string().contains("unproved validator contract"));
    }

    let fenced = NamespacePair {
        legacy: Some(NamespaceKind::Collection),
        legacy_marked: true,
        legacy_fenced: true,
        legacy_uuid: Some(test_collection_uuid(1)),
        ..NamespacePair::default()
    };
    let fenced_contracts = BTreeMap::from([
        (
            LEGACY_SESSIONS.to_owned(),
            CollectionContract {
                kind: NamespaceKind::Collection,
                validator: Some(pre_coordination_session_protocol_validator()),
                validation_level: Some(ValidationLevel::Strict),
                validation_action: Some(ValidationAction::Error),
                uuid: Some(test_collection_uuid(1)),
            },
        ),
        (
            LEGACY_EVENTS.to_owned(),
            raw_contract(test_collection_uuid(2)),
        ),
    ]);
    assert!(validate_raw_legacy_stage_source(&fenced, &raw_events, &fenced_contracts).is_err());

    let markerless_partial = NamespacePair {
        authoritative: Some(NamespaceKind::Collection),
        authoritative_marked: true,
        authoritative_fenced: true,
        authoritative_uuid: Some(test_collection_uuid(1)),
        ..NamespacePair::default()
    };
    let partial_contracts = BTreeMap::from([
        (
            PRE_COORDINATION_SESSIONS.to_owned(),
            CollectionContract {
                kind: NamespaceKind::Collection,
                validator: Some(pre_coordination_session_protocol_validator()),
                validation_level: Some(ValidationLevel::Strict),
                validation_action: Some(ValidationAction::Error),
                uuid: Some(test_collection_uuid(1)),
            },
        ),
        (
            LEGACY_EVENTS.to_owned(),
            raw_contract(test_collection_uuid(2)),
        ),
    ]);
    let partial =
        validate_raw_legacy_stage_source(&markerless_partial, &raw_events, &partial_contracts)
            .expect_err("a markerless partial migration cannot be adopted");
    assert!(partial.to_string().contains("markerless sessions_v2"));

    let markerless_complete = NamespacePair {
        legacy: Some(NamespaceKind::View {
            source: Some(PRE_COORDINATION_SESSIONS.into()),
            pipeline_is_empty: true,
        }),
        authoritative: Some(NamespaceKind::Collection),
        authoritative_marked: true,
        authoritative_fenced: true,
        authoritative_uuid: Some(test_collection_uuid(1)),
        ..NamespacePair::default()
    };
    let markerless_complete_contracts = BTreeMap::from([
        (
            LEGACY_SESSIONS.to_owned(),
            CollectionContract {
                kind: markerless_complete.legacy.clone().unwrap(),
                validator: None,
                validation_level: None,
                validation_action: None,
                uuid: None,
            },
        ),
        (
            PRE_COORDINATION_SESSIONS.to_owned(),
            CollectionContract {
                kind: NamespaceKind::Collection,
                validator: Some(pre_coordination_session_protocol_validator()),
                validation_level: Some(ValidationLevel::Strict),
                validation_action: Some(ValidationAction::Error),
                uuid: Some(test_collection_uuid(1)),
            },
        ),
        (
            LEGACY_EVENTS.to_owned(),
            raw_contract(test_collection_uuid(2)),
        ),
    ]);
    assert!(validate_raw_legacy_stage_source(
        &markerless_complete,
        &raw_events,
        &markerless_complete_contracts,
    )
    .is_err());

    let ambiguous = NamespacePair {
        legacy: Some(NamespaceKind::Collection),
        authoritative: Some(NamespaceKind::Collection),
        legacy_uuid: Some(test_collection_uuid(1)),
        authoritative_uuid: Some(test_collection_uuid(3)),
        ..NamespacePair::default()
    };
    let ambiguous_contracts = BTreeMap::from([
        (
            LEGACY_SESSIONS.to_owned(),
            raw_contract(test_collection_uuid(1)),
        ),
        (
            PRE_COORDINATION_SESSIONS.to_owned(),
            raw_contract(test_collection_uuid(3)),
        ),
        (
            LEGACY_EVENTS.to_owned(),
            raw_contract(test_collection_uuid(2)),
        ),
    ]);
    assert!(
        validate_raw_legacy_stage_source(&ambiguous, &raw_events, &ambiguous_contracts).is_err()
    );
}

#[test]
fn in_progress_source_ids_distinguish_loss_replacement_and_legitimate_absence() {
    let bound = test_namespace_cutover_source_ids(Some(1), None, None);
    let raw = NamespacePair {
        legacy: Some(NamespaceKind::Collection),
        legacy_uuid: Some(test_collection_uuid(1)),
        ..NamespacePair::default()
    };
    validate_namespace_cutover_source_ids(&bound, &raw, &NamespacePair::default())
        .expect("the exact raw source UUID remains valid");

    let renamed = NamespacePair {
        authoritative: Some(NamespaceKind::Collection),
        authoritative_uuid: Some(test_collection_uuid(1)),
        ..NamespacePair::default()
    };
    validate_namespace_cutover_source_ids(&bound, &renamed, &NamespacePair::default())
        .expect("the source UUID follows its authoritative rename");

    let missing = validate_namespace_cutover_source_ids(
        &bound,
        &NamespacePair::default(),
        &NamespacePair::default(),
    )
    .expect_err("a UUID-bound raw source cannot disappear");
    assert!(missing
        .to_string()
        .contains("disappeared after its UUID was durably recorded"));

    let replacement = NamespacePair {
        legacy: Some(NamespaceKind::Collection),
        legacy_uuid: Some(test_collection_uuid(2)),
        ..NamespacePair::default()
    };
    assert!(
        validate_namespace_cutover_source_ids(&bound, &replacement, &NamespacePair::default(),)
            .expect_err("a replacement raw source cannot inherit provenance")
            .to_string()
            .contains("UUID no longer matches its durable provenance")
    );

    let absent = test_namespace_cutover_source_ids(None, None, None);
    validate_namespace_cutover_source_ids(
        &absent,
        &NamespacePair::default(),
        &NamespacePair::default(),
    )
    .expect("a source recorded absent may remain absent");
    let stage_created = NamespacePair {
        authoritative: Some(NamespaceKind::Collection),
        authoritative_uuid: Some(test_collection_uuid(3)),
        ..NamespacePair::default()
    };
    validate_namespace_cutover_source_ids(&absent, &stage_created, &NamespacePair::default())
        .expect("the stage may create an authoritative collection for an absent source");
    let appeared = NamespacePair {
        legacy: Some(NamespaceKind::Collection),
        legacy_uuid: Some(test_collection_uuid(3)),
        ..NamespacePair::default()
    };
    assert!(
        validate_namespace_cutover_source_ids(&absent, &appeared, &NamespacePair::default(),)
            .expect_err("a legacy source may not appear after absence was recorded")
            .to_string()
            .contains("was durably absent but a collection appeared")
    );
}

#[test]
fn staged_legacy_cutover_accepts_only_its_exact_event_capture_intermediate() {
    let sessions = NamespacePair::default();
    let events = NamespacePair {
        legacy: Some(NamespaceKind::Collection),
        legacy_marked: true,
        legacy_fenced: false,
        legacy_uuid: Some(test_collection_uuid(2)),
        ..NamespacePair::default()
    };
    let exact = CollectionContract {
        kind: NamespaceKind::Collection,
        validator: Some(event_protocol_validator()),
        validation_level: Some(ValidationLevel::Strict),
        validation_action: Some(ValidationAction::Warn),
        uuid: Some(test_collection_uuid(2)),
    };
    validate_legacy_stage_namespace_contracts(
        &sessions,
        &events,
        &BTreeMap::from([(LEGACY_EVENTS.to_owned(), exact.clone())]),
    )
    .expect("the durable strict/warn event-capture marker is resumable");

    for changed in [
        CollectionContract {
            validation_level: Some(ValidationLevel::Off),
            ..exact.clone()
        },
        CollectionContract {
            validation_action: Some(ValidationAction::Error),
            ..exact.clone()
        },
        CollectionContract {
            validator: Some(doc! { "unexpected": true }),
            ..exact.clone()
        },
    ] {
        assert!(validate_legacy_stage_namespace_contracts(
            &sessions,
            &events,
            &BTreeMap::from([(LEGACY_EVENTS.to_owned(), changed)]),
        )
        .is_err());
    }
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
    let ready_sessions = ready(PRE_COORDINATION_SESSIONS, test_collection_uuid(1));
    let ready_events = ready(EVENTS, test_collection_uuid(2));
    let in_progress = NamespaceCutoverMarker::InProgress {
        origin: "new-database".into(),
        source_ids: test_namespace_cutover_source_ids(None, None, None),
        tombstone_id: None,
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
    let in_progress = NamespaceCutoverMarker::InProgress {
        origin: "new-database".into(),
        source_ids: test_namespace_cutover_source_ids(None, None, None),
        tombstone_id: None,
    };
    assert_eq!(
        validate_namespace_cutover_marker(namespace_cutover_marker_document(&in_progress).unwrap())
            .unwrap(),
        in_progress,
    );
    let complete = NamespaceCutoverMarker::Complete {
        origin: "operator-drained".into(),
        collection_ids: BTreeMap::from([
            (
                PRE_COORDINATION_SESSIONS.to_owned(),
                test_collection_uuid(1),
            ),
            (EVENTS.to_owned(), test_collection_uuid(2)),
            (TOMBSTONES.to_owned(), test_collection_uuid(3)),
        ]),
    };
    assert_eq!(
        validate_namespace_cutover_marker(namespace_cutover_marker_document(&complete).unwrap())
            .unwrap(),
        complete
    );

    for bound in [
        NamespaceCutoverMarker::InProgress {
            origin: "operator-drained".into(),
            source_ids: test_namespace_cutover_source_ids(Some(4), None, Some(6)),
            tombstone_id: Some(test_collection_uuid(6)),
        },
        NamespaceCutoverMarker::InProgress {
            origin: "operator-drained".into(),
            source_ids: test_namespace_cutover_source_ids(Some(4), None, None),
            tombstone_id: Some(test_collection_uuid(6)),
        },
    ] {
        assert_eq!(
            validate_namespace_cutover_marker(namespace_cutover_marker_document(&bound).unwrap())
                .unwrap(),
            bound,
        );
    }

    for invalid in [
        NamespaceCutoverMarker::InProgress {
            origin: "operator-drained".into(),
            source_ids: test_namespace_cutover_source_ids(Some(4), None, Some(6)),
            tombstone_id: None,
        },
        NamespaceCutoverMarker::InProgress {
            origin: "operator-drained".into(),
            source_ids: test_namespace_cutover_source_ids(Some(4), None, Some(6)),
            tombstone_id: Some(test_collection_uuid(7)),
        },
    ] {
        assert!(namespace_cutover_marker_document(&invalid).is_err());
    }
    let mut missing_source = test_namespace_cutover_source_ids(Some(4), None, None);
    missing_source.remove(LEGACY_EVENTS);
    assert!(
        namespace_cutover_marker_document(&NamespaceCutoverMarker::InProgress {
            origin: "operator-drained".into(),
            source_ids: missing_source,
            tombstone_id: None,
        })
        .is_err()
    );
    let mut extra_source = test_namespace_cutover_source_ids(Some(4), None, None);
    extra_source.insert("unexpected".into(), None);
    assert!(
        namespace_cutover_marker_document(&NamespaceCutoverMarker::InProgress {
            origin: "operator-drained".into(),
            source_ids: extra_source,
            tombstone_id: None,
        })
        .is_err()
    );
    assert!(
        namespace_cutover_marker_document(&NamespaceCutoverMarker::Complete {
            origin: "operator-drained".into(),
            collection_ids: BTreeMap::from([
                (
                    PRE_COORDINATION_SESSIONS.to_owned(),
                    test_collection_uuid(1),
                ),
                (EVENTS.to_owned(), test_collection_uuid(2)),
            ]),
        })
        .is_err()
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
fn coordination_receipt_provenance_is_exact_and_uuid_bound() {
    assert_eq!(
        validate_coordination_receipts_provenance(
            coordination_receipts_provenance_document(&CoordinationReceiptsProvenance::InProgress,)
                .unwrap(),
        )
        .unwrap(),
        CoordinationReceiptsProvenance::InProgress
    );
    let complete = CoordinationReceiptsProvenance::Complete {
        collection_id: test_collection_uuid(9),
    };
    assert_eq!(
        validate_coordination_receipts_provenance(
            coordination_receipts_provenance_document(&complete).unwrap(),
        )
        .unwrap(),
        complete
    );
    for invalid in [
        doc! {
            "_id": COORDINATION_RECEIPTS_PROVENANCE_ID,
            "protocol": COORDINATION_RECEIPTS_PROVENANCE_PROTOCOL + 1,
            "phase": "in-progress",
        },
        doc! {
            "_id": COORDINATION_RECEIPTS_PROVENANCE_ID,
            "protocol": COORDINATION_RECEIPTS_PROVENANCE_PROTOCOL,
            "phase": "complete",
        },
        doc! {
            "_id": COORDINATION_RECEIPTS_PROVENANCE_ID,
            "protocol": COORDINATION_RECEIPTS_PROVENANCE_PROTOCOL,
            "phase": "in-progress",
            "extra": true,
        },
    ] {
        assert!(validate_coordination_receipts_provenance(invalid).is_err());
    }
    assert!(validate_exact_coordination_receipts_id(
        &test_collection_uuid(1),
        &test_collection_uuid(2),
    )
    .is_err());
}

#[test]
fn coordination_activation_provenance_is_exact_and_uuid_bound() {
    let in_progress = CoordinationActivationMarker::InProgress {
        origin: "new-database".into(),
    };
    assert_eq!(
        validate_coordination_activation_marker(
            coordination_activation_marker_document(&in_progress).unwrap(),
        )
        .unwrap(),
        in_progress
    );

    let complete = CoordinationActivationMarker::Complete {
        origin: "operator-drained".into(),
        collection_ids: BTreeMap::from([
            (SESSIONS.to_owned(), test_collection_uuid(7)),
            (COORDINATION_RECEIPTS.to_owned(), test_collection_uuid(8)),
        ]),
    };
    assert_eq!(
        validate_coordination_activation_marker(
            coordination_activation_marker_document(&complete).unwrap(),
        )
        .unwrap(),
        complete
    );

    let legacy_complete = CoordinationActivationMarker::LegacyComplete {
        protocol: LEGACY_COORDINATION_ACTIVATION_PROTOCOL,
        origin: "operator-drained".into(),
        collection_ids: BTreeMap::from([
            (SESSIONS.to_owned(), test_collection_uuid(7)),
            (COORDINATION_RECEIPTS.to_owned(), test_collection_uuid(8)),
        ]),
    };
    let legacy_document = coordination_activation_marker_document(&legacy_complete).unwrap();
    assert_eq!(
        legacy_document.get_i32("protocol").unwrap(),
        LEGACY_COORDINATION_ACTIVATION_PROTOCOL
    );
    assert_eq!(
        validate_coordination_activation_marker(legacy_document).unwrap(),
        legacy_complete
    );

    let initial_complete = CoordinationActivationMarker::LegacyComplete {
        protocol: INITIAL_COORDINATION_ACTIVATION_PROTOCOL,
        origin: "operator-drained".into(),
        collection_ids: BTreeMap::from([
            (SESSIONS.to_owned(), test_collection_uuid(7)),
            (COORDINATION_RECEIPTS.to_owned(), test_collection_uuid(8)),
        ]),
    };
    let initial_document = coordination_activation_marker_document(&initial_complete).unwrap();
    assert_eq!(
        initial_document.get_i32("protocol").unwrap(),
        INITIAL_COORDINATION_ACTIVATION_PROTOCOL
    );
    assert_eq!(
        validate_coordination_activation_marker(initial_document).unwrap(),
        initial_complete
    );

    let legacy_upgrade_ids = BTreeMap::from([
        (SESSIONS.to_owned(), test_collection_uuid(7)),
        (COORDINATION_RECEIPTS.to_owned(), test_collection_uuid(8)),
    ]);
    let legacy_upgrade = CoordinationActivationMarker::LegacyUpgradeInProgress {
        protocol: LEGACY_COORDINATION_ACTIVATION_PROTOCOL,
        origin: "operator-drained".into(),
        collection_ids: legacy_upgrade_ids.clone(),
    };
    let legacy_upgrade_document = coordination_activation_marker_document(&legacy_upgrade).unwrap();
    assert_eq!(
        legacy_upgrade_document,
        doc! {
            "_id": COORDINATION_ACTIVATION_ID,
            "protocol": 2_i32,
            "origin": "operator-drained",
            "phase": "upgrade-in-progress",
            "collection_uuids": {
                SESSIONS: test_collection_uuid(7),
                COORDINATION_RECEIPTS: test_collection_uuid(8),
            },
        }
    );
    assert_eq!(
        validate_coordination_activation_marker(legacy_upgrade_document).unwrap(),
        legacy_upgrade
    );

    let upgrade = CoordinationActivationMarker::UpgradeInProgress {
        origin: "operator-drained".into(),
        collection_ids: BTreeMap::from([
            (SESSIONS.to_owned(), test_collection_uuid(7)),
            (COORDINATION_RECEIPTS.to_owned(), test_collection_uuid(8)),
        ]),
    };
    assert_eq!(
        validate_coordination_activation_marker(
            coordination_activation_marker_document(&upgrade).unwrap()
        )
        .unwrap(),
        upgrade
    );

    for invalid in [
        doc! {
            "_id": COORDINATION_ACTIVATION_ID,
            "protocol": COORDINATION_ACTIVATION_PROTOCOL + 1,
            "origin": "new-database",
            "phase": "in-progress",
        },
        doc! {
            "_id": COORDINATION_ACTIVATION_ID,
            "protocol": COORDINATION_ACTIVATION_PROTOCOL,
            "origin": NAMESPACE_CUTOVER_OBSERVED_ORIGIN,
            "phase": "in-progress",
        },
        doc! {
            "_id": COORDINATION_ACTIVATION_ID,
            "protocol": COORDINATION_ACTIVATION_PROTOCOL,
            "origin": "new-database",
            "phase": "complete",
        },
        doc! {
            "_id": COORDINATION_ACTIVATION_ID,
            "protocol": COORDINATION_ACTIVATION_PROTOCOL,
            "origin": "new-database",
            "phase": "in-progress",
            "extra": true,
        },
    ] {
        assert!(validate_coordination_activation_marker(invalid).is_err());
    }

    let recorded = BTreeMap::from([
        (SESSIONS.to_owned(), test_collection_uuid(7)),
        (COORDINATION_RECEIPTS.to_owned(), test_collection_uuid(8)),
    ]);
    assert!(validate_coordination_activation_ids(&recorded, &recorded).is_ok());
    assert!(validate_coordination_activation_ids(
        &recorded,
        &BTreeMap::from([
            (SESSIONS.to_owned(), test_collection_uuid(9)),
            (COORDINATION_RECEIPTS.to_owned(), test_collection_uuid(8)),
        ]),
    )
    .is_err());
}

#[test]
fn legacy_and_unproven_upgrade_markers_are_always_rejected() {
    let ids = BTreeMap::from([
        (SESSIONS.to_owned(), test_collection_uuid(7)),
        (COORDINATION_RECEIPTS.to_owned(), test_collection_uuid(8)),
    ]);
    let legacy = [
        CoordinationActivationMarker::LegacyInProgress {
            protocol: INITIAL_COORDINATION_ACTIVATION_PROTOCOL,
            origin: "operator-drained".into(),
        },
        CoordinationActivationMarker::LegacyInProgress {
            protocol: LEGACY_COORDINATION_ACTIVATION_PROTOCOL,
            origin: "operator-drained".into(),
        },
        CoordinationActivationMarker::LegacyComplete {
            protocol: INITIAL_COORDINATION_ACTIVATION_PROTOCOL,
            origin: "operator-drained".into(),
            collection_ids: ids.clone(),
        },
        CoordinationActivationMarker::LegacyComplete {
            protocol: LEGACY_COORDINATION_ACTIVATION_PROTOCOL,
            origin: "operator-drained".into(),
            collection_ids: ids.clone(),
        },
        CoordinationActivationMarker::LegacyUpgradeInProgress {
            protocol: LEGACY_COORDINATION_ACTIVATION_PROTOCOL,
            origin: "operator-drained".into(),
            collection_ids: ids.clone(),
        },
    ];
    for marker in legacy {
        for coordination_cutover_confirmed in [false, true] {
            let error = authorize_coordination_protocol_upgrade(
                marker.clone(),
                coordination_cutover_confirmed,
            )
            .expect_err("old marker phases cannot prove exact usage boundaries");
            let message = error.to_string();
            assert!(message.contains("markerless runtime-switch boundaries"));
            assert!(message.contains("cannot restore that accounting boundary"));
            assert!(message.contains("reset or restore"));
        }
    }

    let unproven_upgrade = CoordinationActivationMarker::UpgradeInProgress {
        origin: "operator-drained".into(),
        collection_ids: ids.clone(),
    };
    for coordination_cutover_confirmed in [false, true] {
        let error = authorize_coordination_protocol_upgrade(
            unproven_upgrade.clone(),
            coordination_cutover_confirmed,
        )
        .expect_err("the experimental upgrade residue has no source proof");
        let message = error.to_string();
        assert!(message.contains("upgrade-in-progress provenance cannot be resumed"));
        assert!(message.contains("does not prove a reconstructable runtime-switch boundary"));
        assert!(message.contains("refusing to mutate its marker, validators, or documents"));
    }

    for current in [
        CoordinationActivationMarker::InProgress {
            origin: "operator-drained".into(),
        },
        CoordinationActivationMarker::Complete {
            origin: "operator-drained".into(),
            collection_ids: ids,
        },
    ] {
        for coordination_cutover_confirmed in [false, true] {
            assert_eq!(
                authorize_coordination_protocol_upgrade(
                    current.clone(),
                    coordination_cutover_confirmed,
                )
                .expect("current initial activation and completion remain supported"),
                current
            );
        }
    }
}

#[test]
fn coordination_activation_requires_the_exact_gate_for_every_existing_database() {
    assert_eq!(
        coordination_activation_origin(true, false).unwrap(),
        "new-database"
    );
    assert_eq!(
        coordination_activation_origin(true, true).unwrap(),
        "new-database"
    );
    assert_eq!(
        coordination_activation_origin(false, true).unwrap(),
        "operator-drained"
    );
    let error = coordination_activation_origin(false, false)
        .expect_err("an existing database cannot infer an external provider drain");
    assert!(error
        .to_string()
        .contains("CHRONO_MONGODB_COORDINATION_CUTOVER=drained"));
}

#[test]
fn coordination_activation_preserves_the_legacy_session_uuid_across_resume_states() {
    let session_uuid = test_collection_uuid(17);
    let before_fence = NamespacePair {
        legacy: Some(NamespaceKind::Collection),
        legacy_uuid: Some(session_uuid.clone()),
        ..NamespacePair::default()
    };
    let after_rename = NamespacePair {
        authoritative: Some(NamespaceKind::Collection),
        authoritative_uuid: Some(session_uuid.clone()),
        ..NamespacePair::default()
    };
    let after_view = NamespacePair {
        legacy: Some(NamespaceKind::View {
            source: Some(SESSIONS.into()),
            pipeline_is_empty: true,
        }),
        authoritative: Some(NamespaceKind::Collection),
        authoritative_uuid: Some(session_uuid.clone()),
        ..NamespacePair::default()
    };

    for state in [&before_fence, &after_rename, &after_view] {
        assert_eq!(
            coordination_migration_session_id(state).unwrap(),
            &session_uuid
        );
    }
    assert!(coordination_migration_session_id(&NamespacePair::default()).is_err());
    assert!(coordination_migration_session_id(&NamespacePair {
        legacy: Some(NamespaceKind::Collection),
        authoritative: Some(NamespaceKind::Collection),
        legacy_uuid: Some(session_uuid.clone()),
        authoritative_uuid: Some(session_uuid),
        ..NamespacePair::default()
    })
    .is_err());
}

#[test]
fn markerless_activation_accepts_only_an_uncoordinated_source_shape() {
    let pre_coordination = NamespacePair {
        legacy: Some(NamespaceKind::Collection),
        ..NamespacePair::default()
    };
    validate_markerless_coordination_activation_source(&pre_coordination, false, false)
        .expect("the untouched pre-coordination source can record initial intent");

    for coordinated in [
        NamespacePair {
            authoritative: Some(NamespaceKind::Collection),
            ..NamespacePair::default()
        },
        NamespacePair {
            legacy: Some(NamespaceKind::View {
                source: Some(SESSIONS.into()),
                pipeline_is_empty: true,
            }),
            authoritative: Some(NamespaceKind::Collection),
            ..NamespacePair::default()
        },
    ] {
        let error = validate_markerless_coordination_activation_source(&coordinated, false, false)
            .expect_err("post-rename state requires its durable activation marker");
        assert!(error
            .to_string()
            .contains("not an untouched pre-coordination source"));
    }

    for (receipts_provenance_exists, receipts_collection_exists) in
        [(true, false), (false, true), (true, true)]
    {
        let error = validate_markerless_coordination_activation_source(
            &pre_coordination,
            receipts_provenance_exists,
            receipts_collection_exists,
        )
        .expect_err("receipt residue proves this is not a fresh activation source");
        assert!(error
            .to_string()
            .contains("coordination-receipt state already exists"));
    }
}

#[test]
fn completed_coordination_namespace_is_a_rollback_fence() {
    let old_binary_pair = NamespacePair {
        legacy: Some(NamespaceKind::View {
            source: Some(PRE_COORDINATION_SESSIONS.into()),
            pipeline_is_empty: true,
        }),
        authoritative: Some(NamespaceKind::View {
            source: Some(SESSIONS.into()),
            pipeline_is_empty: true,
        }),
        ..NamespacePair::default()
    };
    assert!(
        next_namespace_action(&old_binary_pair, LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS,)
            .is_err()
    );

    let old_ids = BTreeMap::from([
        (
            PRE_COORDINATION_SESSIONS.to_owned(),
            test_collection_uuid(21),
        ),
        (EVENTS.to_owned(), test_collection_uuid(22)),
    ]);
    let current_ids = BTreeMap::from([
        (SESSIONS.to_owned(), test_collection_uuid(21)),
        (EVENTS.to_owned(), test_collection_uuid(22)),
    ]);
    assert!(validate_migrated_namespace_ids(&old_ids, &current_ids).is_ok());
    assert!(validate_migrated_namespace_ids(
        &old_ids,
        &BTreeMap::from([
            (SESSIONS.to_owned(), test_collection_uuid(23)),
            (EVENTS.to_owned(), test_collection_uuid(22)),
        ]),
    )
    .is_err());
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
            (
                PRE_COORDINATION_SESSIONS.to_owned(),
                test_collection_uuid(1),
            ),
            (EVENTS.to_owned(), test_collection_uuid(2)),
            (TOMBSTONES.to_owned(), test_collection_uuid(4)),
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
        &ready(PRE_COORDINATION_SESSIONS, test_collection_uuid(3)),
        &ready(EVENTS, test_collection_uuid(2)),
        false,
        Some(&marker),
    )
    .expect_err("completed provenance must bind the authoritative UUIDs");
    assert!(replacement.to_string().contains("UUID no longer matches"));
}

#[test]
fn protocol_validators_require_the_current_marker_and_session_revision() {
    let pre_coordination = pre_coordination_session_protocol_validator();
    let pre_coordination_required = pre_coordination
        .get_document("$jsonSchema")
        .unwrap()
        .get_array("required")
        .unwrap();
    assert!(pre_coordination_required.contains(&Bson::String(WRITE_PROTOCOL_FIELD.into())));
    assert!(pre_coordination_required.contains(&Bson::String("revision".into())));
    assert!(!pre_coordination_required.contains(&Bson::String(COORDINATION_PROTOCOL_FIELD.into())));

    let sessions = session_protocol_validator();
    let schema = sessions
        .get_document("$jsonSchema")
        .expect("sessions use JSON schema validation");
    let required = schema
        .get_array("required")
        .expect("required fields are explicit");
    assert!(required.contains(&Bson::String(WRITE_PROTOCOL_FIELD.into())));
    assert!(required.contains(&Bson::String(COORDINATION_PROTOCOL_FIELD.into())));
    assert!(required.contains(&Bson::String("revision".into())));
    assert_eq!(
        schema
            .get_document("properties")
            .unwrap()
            .get_document(WRITE_PROTOCOL_FIELD)
            .unwrap()
            .get_array("enum")
            .unwrap(),
        &[Bson::Int32(SESSION_WRITE_PROTOCOL)]
    );
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
        coordination_receipt_protocol_validator(),
    ] {
        let schema = validator.get_document("$jsonSchema").unwrap();
        assert!(schema
            .get_array("required")
            .unwrap()
            .contains(&Bson::String(WRITE_PROTOCOL_FIELD.into())));
        if documents_equivalent(&validator, &coordination_receipt_protocol_validator()) {
            assert!(schema
                .get_array("required")
                .unwrap()
                .contains(&Bson::String(COORDINATION_PROTOCOL_FIELD.into())));
        }
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

    let receipt_schema = coordination_receipt_protocol_validator();
    let receipt_schema = receipt_schema.get_document("$jsonSchema").unwrap();
    assert_eq!(
        receipt_schema
            .get_document("properties")
            .unwrap()
            .get_document(COORDINATION_PROTOCOL_FIELD)
            .unwrap()
            .get_array("enum")
            .unwrap(),
        &[Bson::Int32(COORDINATION_RECEIPT_PROTOCOL)]
    );
    assert_ne!(
        LEGACY_COORDINATION_RECEIPT_PROTOCOL, COORDINATION_RECEIPT_PROTOCOL,
        "new marker-bearing receipts need a distinct durable protocol"
    );
    assert_ne!(
        COORDINATION_RECEIPT_PROTOCOL, SESSION_COORDINATION_PROTOCOL,
        "receipt and session protocols have distinct durable schemas"
    );
    assert!(receipt_schema
        .get_array("required")
        .unwrap()
        .contains(&Bson::String("payload".into())));
    assert_eq!(
        receipt_schema
            .get_document("properties")
            .unwrap()
            .get_document("payload")
            .unwrap()
            .get_document("properties")
            .unwrap()
            .get_document("value")
            .unwrap()
            .get_document("properties")
            .unwrap()
            .get_document("completed_by")
            .unwrap()
            .get_str("bsonType")
            .unwrap(),
        "string"
    );
    assert_eq!(
        receipt_schema
            .get_document("properties")
            .unwrap()
            .get_document("payload")
            .unwrap()
            .get_document("properties")
            .unwrap()
            .get_document("value")
            .unwrap()
            .get_document("properties")
            .unwrap()
            .get_document("completed_by")
            .unwrap()
            .get_i32("minLength")
            .unwrap(),
        1
    );
}

#[test]
fn receipt_validator_requires_every_completed_switch_to_name_a_marker() {
    let current = coordination_receipt_protocol_validator();
    assert_eq!(
        current.get_array("$or").unwrap(),
        &[
            Bson::Document(doc! { "payload.value.switch": Bson::Null }),
            Bson::Document(doc! {
                "payload.value.switch.runtime_started_seq": {
                    "$type": "long",
                    "$gt": 0_i64,
                },
            }),
        ]
    );

    let legacy = legacy_coordination_receipt_protocol_validator();
    assert!(!legacy.contains_key("$or"));
    assert_eq!(
        legacy,
        doc! {
            "$jsonSchema": {
                "bsonType": "object",
                "required": [
                    WRITE_PROTOCOL_FIELD,
                    COORDINATION_PROTOCOL_FIELD,
                    "payload",
                ],
                "properties": {
                    WRITE_PROTOCOL_FIELD: {
                        "bsonType": "int",
                        "enum": [WRITE_PROTOCOL],
                    },
                    COORDINATION_PROTOCOL_FIELD: {
                        "bsonType": "int",
                        "enum": [LEGACY_COORDINATION_RECEIPT_PROTOCOL],
                    },
                    "payload": {
                        "bsonType": "object",
                        "required": ["type", "value"],
                        "properties": {
                            "type": { "bsonType": "string" },
                            "value": {
                                "bsonType": "object",
                                "properties": {
                                    "completed_by": {
                                        "bsonType": "string",
                                        "minLength": 1,
                                    },
                                },
                            },
                        },
                    },
                },
            },
        }
    );
}

fn test_collection_uuid(value: u8) -> Binary {
    Binary {
        subtype: mongodb::bson::spec::BinarySubtype::Uuid,
        bytes: vec![value; 16],
    }
}

fn test_namespace_cutover_source_ids(
    sessions: Option<u8>,
    events: Option<u8>,
    tombstones: Option<u8>,
) -> BTreeMap<String, Option<Binary>> {
    BTreeMap::from([
        (
            LEGACY_SESSIONS.to_owned(),
            sessions.map(test_collection_uuid),
        ),
        (LEGACY_EVENTS.to_owned(), events.map(test_collection_uuid)),
        (TOMBSTONES.to_owned(), tombstones.map(test_collection_uuid)),
    ])
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
fn session_write_contract_classifier_accepts_only_exact_current_or_previous_contracts() {
    let uuid = test_collection_uuid(9);
    let current = exact_collection_contract(SESSIONS, uuid.clone());
    assert_eq!(
        classify_session_write_contract(&current, &uuid).unwrap(),
        SessionWriteContract::Current
    );

    let mut previous = current.clone();
    previous.validator = Some(previous_session_protocol_validator());
    assert_eq!(
        classify_session_write_contract(&previous, &uuid).unwrap(),
        SessionWriteContract::Previous
    );

    let mut unknown = current.clone();
    unknown.validator = Some(session_protocol_validator_for(
        SESSION_WRITE_PROTOCOL + 1,
        SESSION_COORDINATION_PROTOCOL,
    ));
    assert!(classify_session_write_contract(&unknown, &uuid).is_err());

    let mut mixed = current.clone();
    let mut mixed_validator = session_protocol_validator();
    *mixed_validator
        .get_document_mut("$jsonSchema")
        .unwrap()
        .get_document_mut("properties")
        .unwrap()
        .get_document_mut(WRITE_PROTOCOL_FIELD)
        .unwrap()
        .get_array_mut("enum")
        .unwrap() = vec![
        Bson::Int32(WRITE_PROTOCOL),
        Bson::Int32(SESSION_WRITE_PROTOCOL),
    ];
    mixed.validator = Some(mixed_validator);
    assert!(classify_session_write_contract(&mixed, &uuid).is_err());

    let mut wrong_coordination = current.clone();
    wrong_coordination.validator = Some(legacy_session_protocol_validator());
    assert!(classify_session_write_contract(&wrong_coordination, &uuid).is_err());

    let mut weakened = previous.clone();
    weakened.validation_action = Some(ValidationAction::Warn);
    assert!(classify_session_write_contract(&weakened, &uuid).is_err());

    let mut view = current.clone();
    view.kind = NamespaceKind::View {
        source: Some(SESSIONS.into()),
        pipeline_is_empty: true,
    };
    assert!(classify_session_write_contract(&view, &uuid).is_err());

    assert!(classify_session_write_contract(&current, &test_collection_uuid(10)).is_err());
}

#[test]
fn validator_comparison_ignores_document_key_order_but_not_array_order_or_types() {
    let reordered = doc! {
        "$jsonSchema": {
            "properties": {
                "revision": { "bsonType": "long" },
                WRITE_PROTOCOL_FIELD: {
                    "enum": [SESSION_WRITE_PROTOCOL],
                    "bsonType": "int",
                },
                COORDINATION_PROTOCOL_FIELD: {
                    "enum": [SESSION_COORDINATION_PROTOCOL],
                    "bsonType": "int",
                },
            },
            "required": [WRITE_PROTOCOL_FIELD, "revision", COORDINATION_PROTOCOL_FIELD],
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
                    "enum": [SESSION_WRITE_PROTOCOL],
                },
                "revision": { "bsonType": "long" },
                COORDINATION_PROTOCOL_FIELD: {
                    "bsonType": "int",
                    "enum": [SESSION_COORDINATION_PROTOCOL],
                },
            },
            "required": ["revision", WRITE_PROTOCOL_FIELD, COORDINATION_PROTOCOL_FIELD],
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
            "required": [WRITE_PROTOCOL_FIELD, "revision", COORDINATION_PROTOCOL_FIELD],
            "properties": {
                WRITE_PROTOCOL_FIELD: {
                    "bsonType": "int",
                    "enum": [i64::from(SESSION_WRITE_PROTOCOL)],
                },
                "revision": { "bsonType": "long" },
                COORDINATION_PROTOCOL_FIELD: {
                    "bsonType": "int",
                    "enum": [SESSION_COORDINATION_PROTOCOL],
                },
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
fn coordination_receipts_have_a_required_session_and_kind_index() {
    let required = required_indexes()
        .into_iter()
        .find(|required| required.collection == COORDINATION_RECEIPTS)
        .expect("coordination receipts are indexed for debt scans and deletion");
    assert_eq!(
        required.keys,
        doc! {
            "_id.session_id": 1,
            "_id.kind": 1,
            "payload.type": 1,
        }
    );
    validate_required_index(&required, &[required_index_model(&required)])
        .expect("the exact coordination receipt index is accepted");
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
fn current_session_documents_carry_both_writer_protocols() {
    let stored = to_doc(&record_with(None)).expect("record encodes");
    let document = mongodb::bson::serialize_to_document(&stored).expect("record serializes");
    assert_eq!(
        document.get_i32(WRITE_PROTOCOL_FIELD).unwrap(),
        SESSION_WRITE_PROTOCOL
    );
    assert_eq!(
        document.get_i32(COORDINATION_PROTOCOL_FIELD).unwrap(),
        SESSION_COORDINATION_PROTOCOL
    );
    assert!(document.contains_key("coordination"));

    let mut predecessor = stored;
    predecessor.write_protocol = WRITE_PROTOCOL;
    assert!(from_doc(predecessor).is_err());
}

#[test]
fn session_protocol_migrations_keep_initial_activation_separate_from_writer_resume() {
    assert_eq!(
        session_protocol_migration_update(),
        doc! {
            "$set": {
                WRITE_PROTOCOL_FIELD: SESSION_WRITE_PROTOCOL,
                COORDINATION_PROTOCOL_FIELD: SESSION_COORDINATION_PROTOCOL,
            },
            "$max": { "revision": 0_i64 },
        }
    );
    assert_eq!(
        current_session_protocol_filter(),
        doc! {
            WRITE_PROTOCOL_FIELD: SESSION_WRITE_PROTOCOL,
            COORDINATION_PROTOCOL_FIELD: SESSION_COORDINATION_PROTOCOL,
            "revision": { "$type": "long" },
        }
    );
    assert_eq!(
        previous_session_protocol_filter(),
        doc! {
            WRITE_PROTOCOL_FIELD: { "$eq": WRITE_PROTOCOL, "$type": "int" },
            COORDINATION_PROTOCOL_FIELD: {
                "$eq": SESSION_COORDINATION_PROTOCOL,
                "$type": "int",
            },
            "revision": { "$type": "long" },
        }
    );
    assert_eq!(
        session_write_protocol_migration_update(),
        doc! { "$set": { WRITE_PROTOCOL_FIELD: SESSION_WRITE_PROTOCOL } }
    );
}

#[test]
fn read_aggregations_hide_tombstoned_sessions() {
    let query = SessionListQuery::new(PageRequest::new(5, None), None);
    let listed = session_page_pipeline(doc! { "owner": "subject" }, &query).unwrap();
    assert_eq!(listed[0], doc! { "$match": { "owner": "subject" } });
    assert_eq!(listed[1], doc! { "$sort": { "created_at": -1, "_id": -1 } });
    assert_eq!(listed[2], staged_skill_summary_stage());
    assert_eq!(listed[3], tombstone_lookup_stage());
    assert_eq!(
        listed[4],
        doc! { "$match": { "_cma_tombstones.0": { "$exists": false } } }
    );
    assert_eq!(listed[5], doc! { "$project": { "_cma_tombstones": 0 } });
    assert_eq!(listed[6], doc! { "$limit": 6_i64 });

    let exact = exact_session_pipeline(&SessionId("sesn_exact".into()));
    assert_eq!(exact[2], tombstone_lookup_stage());
    assert!(!exact.contains(&staged_skill_summary_stage()));
    assert_eq!(exact.last().unwrap(), &doc! { "$limit": 1_i64 });
    assert_eq!(
        runtime_authority_projection(),
        doc! {
            "_id": 1_i32,
            WRITE_PROTOCOL_FIELD: 1_i32,
            COORDINATION_PROTOCOL_FIELD: 1_i32,
            "runtime_id": 1_i32,
            "coordination": 1_i32,
        }
    );

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
    const EXAMINED_CEILING: i64 = 256;

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

    for required in required_indexes().into_iter().filter(|required| {
        required.collection == SESSIONS
            && [
                OWNER_SESSION_PAGE_INDEX,
                OWNER_STATUS_SESSION_PAGE_INDEX,
                ALL_SESSION_PAGE_INDEX,
                STATUS_SESSION_PAGE_INDEX,
            ]
            .contains(&required.name)
    }) {
        sessions
            .create_index(required_index_model(&required))
            .await
            .unwrap_or_else(|error| panic!("pagination index {} creates: {error}", required.name));
    }

    let cursor = PageCursor::new("2026-08-25T00:00:00Z", "sesn_04990");
    let cases = [
        (
            "all first page",
            doc! {},
            SessionListQuery::new(PageRequest::new(5, None), None),
            ALL_SESSION_PAGE_INDEX,
        ),
        (
            "owner first page",
            doc! { "owner": "subject-page-explain" },
            SessionListQuery::new(PageRequest::new(5, None), None),
            OWNER_SESSION_PAGE_INDEX,
        ),
        (
            "status first page",
            doc! {},
            SessionListQuery::new(PageRequest::new(5, None), Some(SessionStatus::Running)),
            STATUS_SESSION_PAGE_INDEX,
        ),
        (
            "owner status first page",
            doc! { "owner": "subject-page-explain" },
            SessionListQuery::new(PageRequest::new(5, None), Some(SessionStatus::Running)),
            OWNER_STATUS_SESSION_PAGE_INDEX,
        ),
        (
            "all continuation",
            doc! {},
            SessionListQuery::new(PageRequest::new(5, Some(cursor.clone())), None),
            ALL_SESSION_PAGE_INDEX,
        ),
        (
            "owner continuation",
            doc! { "owner": "subject-page-explain" },
            SessionListQuery::new(PageRequest::new(5, Some(cursor.clone())), None),
            OWNER_SESSION_PAGE_INDEX,
        ),
        (
            "status continuation",
            doc! {},
            SessionListQuery::new(
                PageRequest::new(5, Some(cursor.clone())),
                Some(SessionStatus::Running),
            ),
            STATUS_SESSION_PAGE_INDEX,
        ),
        (
            "owner status continuation",
            doc! { "owner": "subject-page-explain" },
            SessionListQuery::new(
                PageRequest::new(5, Some(cursor)),
                Some(SessionStatus::Running),
            ),
            OWNER_STATUS_SESSION_PAGE_INDEX,
        ),
    ];

    for (case, filter, query, expected_index) in cases {
        assert!(matches!(
            session_page_hint(&filter, &query).unwrap(),
            Hint::Name(name) if name == expected_index
        ));
        let pipeline = session_page_pipeline(filter, &query).expect("page pipeline builds");
        let explain = database
            .run_command(doc! {
                "explain": {
                    "aggregate": SESSIONS,
                    "pipeline": pipeline,
                    "cursor": {},
                    "hint": expected_index,
                },
                "verbosity": "executionStats",
            })
            .await
            .unwrap_or_else(|error| panic!("{case} explains: {error}"));

        let stats = explain_cursor_stats(&explain);
        let keys = explain_count(stats, "totalKeysExamined");
        let documents = explain_count(stats, "totalDocsExamined");
        let explain = Bson::Document(explain);
        assert!(
            contains_string_field(&explain, "indexName", expected_index),
            "{case} did not use {expected_index}"
        );
        assert!(
            !contains_string_field(&explain, "stage", "SORT"),
            "{case} introduced a blocking sort before staged-body redaction"
        );
        assert!(
            keys <= EXAMINED_CEILING,
            "{case} examined {keys} keys from {MATCHES} matches"
        );
        assert!(
            documents <= EXAMINED_CEILING,
            "{case} examined {documents} documents from {MATCHES} matches"
        );
    }

    database.drop().await.expect("temporary database drops");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_session_listing_redacts_staged_bodies_while_get_remains_exact() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name MongoDB");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database_name = format!("chrono_staged_listing_{}_{}", std::process::id(), nonce);
    let store = MongoSessionStore::open(&client, &database_name, false, false)
        .await
        .expect("test store opens");
    let mut record = record_with(None);
    record.session.id = SessionId("sesn_staged_listing".into());
    record.session.last_seq = 0;
    record.skill_packs = SkillPackState::Staged(vec![SkillPack {
        name: "private-review".into(),
        version: Some("3.2".into()),
        files: vec![("SKILL.md".into(), b"private-real-list-body-marker".to_vec())],
    }]);
    store
        .create_session_idempotent("seed-staged-listing", &record, &[])
        .await
        .expect("staged session is seeded");

    let authority_document = store
        .sessions
        .clone_with_type::<Document>()
        .find_one(doc! { "_id": &record.session.id.0 })
        .projection(runtime_authority_projection())
        .await
        .expect("projected authority read succeeds")
        .expect("projected authority row exists");
    assert!(!authority_document.contains_key("skill_packs"));
    assert!(!mongodb::bson::serialize_to_vec(&authority_document)
        .unwrap()
        .windows(b"private-real-list-body-marker".len())
        .any(|window| window == b"private-real-list-body-marker"));
    assert_eq!(
        store.runtime_authority(&record.session.id).await.unwrap(),
        Some(RuntimeAuthority {
            generation: 0,
            runtime_id: None,
        })
    );

    let identity = store
        .mongo_session_identity(&record.session.id)
        .await
        .expect("projected existence read succeeds")
        .expect("projected identity exists");
    assert_eq!(identity, doc! { "_id": &record.session.id.0 });
    assert!(store
        .runtime_cleanup_debts(&record.session.id)
        .await
        .expect("cleanup-debt read succeeds")
        .is_empty());

    let query = SessionListQuery::new(PageRequest::new(5, None), None);
    let mut raw = store
        .sessions
        .aggregate(session_page_pipeline(doc! { "_id": &record.session.id.0 }, &query).unwrap())
        .await
        .expect("listing aggregate opens");
    let projected = raw
        .try_next()
        .await
        .expect("listing aggregate reads")
        .expect("staged row is visible");
    assert_eq!(
        projected.get_document("skill_packs").unwrap(),
        &doc! { "state": "staged_summary" }
    );
    let projected_bytes = mongodb::bson::serialize_to_vec(&projected).unwrap();
    assert!(!projected_bytes
        .windows(b"private-real-list-body-marker".len())
        .any(|window| window == b"private-real-list-body-marker"));

    let listed = store
        .list_all(&query)
        .await
        .expect("bounded listing succeeds")
        .items
        .into_iter()
        .next()
        .expect("staged session is listed");
    assert_eq!(listed.skill_packs, SkillPackState::StagedSummary);
    let exact = store
        .get(&record.session.id)
        .await
        .expect("exact read succeeds")
        .expect("exact staged session remains present");
    assert_eq!(exact.skill_packs, record.skill_packs);

    store.database.drop().await.unwrap();
}

#[test]
fn raw_append_touches_the_session_without_lowering_a_legacy_tail() {
    assert_eq!(
        appended_event_session_update(&doc! { "last_seq": 9_i64 }, 7),
        doc! {
            "$set": {
                "_cma_write_protocol": SESSION_WRITE_PROTOCOL,
                "_cma_coordination_protocol": SESSION_COORDINATION_PROTOCOL,
                "revision": 0_i64,
            },
            "$max": { "last_seq": 7_i64 },
        }
    );
    assert_eq!(
        appended_event_session_update(&doc! { "last_seq": 4_i64, "revision": 3_i64 }, 5),
        doc! {
            "$set": {
                "_cma_write_protocol": SESSION_WRITE_PROTOCOL,
                "_cma_coordination_protocol": SESSION_COORDINATION_PROTOCOL,
            },
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
fn session_validation_separates_legacy_cutover_documents_from_post_fence_documents() {
    let stored = to_doc(&record_with(None)).unwrap();
    let current = mongodb::bson::serialize_to_document(&stored).unwrap();
    assert_eq!(validate_stored_session_document(&current).unwrap().1, 7);

    let mut legacy = current.clone();
    legacy.remove("revision");
    legacy.remove(WRITE_PROTOCOL_FIELD);
    assert_eq!(
        validate_compatible_stored_session_document(&legacy)
            .unwrap()
            .1,
        7
    );
    assert!(validate_stored_session_document(&legacy).is_err());

    let mut pre_coordination = legacy.clone();
    pre_coordination.remove(COORDINATION_PROTOCOL_FIELD);
    assert!(validate_compatible_stored_session_document(&pre_coordination).is_ok());
    pre_coordination.insert(
        COORDINATION_PROTOCOL_FIELD,
        LEGACY_SESSION_COORDINATION_PROTOCOL,
    );
    assert!(validate_compatible_stored_session_document(&pre_coordination).is_err());

    legacy.insert(WRITE_PROTOCOL_FIELD, WRITE_PROTOCOL);
    assert!(validate_compatible_stored_session_document(&legacy).is_ok());
    legacy.insert(WRITE_PROTOCOL_FIELD, SESSION_WRITE_PROTOCOL);
    assert!(validate_compatible_stored_session_document(&legacy).is_err());

    legacy.insert("last_seq", -1_i64);
    assert!(validate_compatible_stored_session_document(&legacy).is_err());
    legacy.insert("last_seq", 7_i64);
    legacy.insert("revision", 1_i32);
    assert!(validate_compatible_stored_session_document(&legacy).is_err());
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
fn session_page_filter_pushes_both_surface_families_into_mongo() {
    for (surface, exists) in [
        (SessionSurfaceFilter::Chat, true),
        (SessionSurfaceFilter::Agent, false),
    ] {
        let query =
            SessionListQuery::new(PageRequest::new(50, None), None).with_surface(Some(surface));
        assert_eq!(
            session_page_filter(doc! { "owner": "subject-a" }, &query).unwrap(),
            doc! {
                "owner": "subject-a",
                "chat_origin": { "$exists": exists },
            }
        );
    }
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

    let unfiltered = SessionListQuery::new(PageRequest::new(5, None), None);
    let status = SessionListQuery::new(PageRequest::new(5, None), Some(SessionStatus::Running));
    for (filter, query, expected) in [
        (doc! {}, &unfiltered, ALL_SESSION_PAGE_INDEX),
        (
            doc! { "owner": "subject-page" },
            &unfiltered,
            OWNER_SESSION_PAGE_INDEX,
        ),
        (doc! {}, &status, STATUS_SESSION_PAGE_INDEX),
        (
            doc! { "owner": "subject-page" },
            &status,
            OWNER_STATUS_SESSION_PAGE_INDEX,
        ),
    ] {
        assert!(matches!(
            session_page_hint(&filter, query).unwrap(),
            Hint::Name(name) if name == expected
        ));
    }
    assert!(session_page_hint(&doc! { "kernel": "unsupported" }, &unfiltered).is_err());
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

type DatabaseState = (
    BTreeMap<String, CollectionContract>,
    BTreeMap<String, Vec<Document>>,
);

async fn database_state_snapshot(database: &Database) -> DatabaseState {
    let mut cursor = database
        .list_collections()
        .await
        .expect("test database namespaces list");
    let mut contracts = BTreeMap::new();
    let mut documents = BTreeMap::new();
    while let Some(specification) = cursor
        .try_next()
        .await
        .expect("test database namespace reads")
    {
        let name = specification.name.clone();
        if specification.collection_type == CollectionType::Collection {
            documents.insert(
                name.clone(),
                sorted_collection_documents(database, &name).await,
            );
        }
        contracts.insert(name, collection_contract(&specification));
    }
    (contracts, documents)
}

const MONGODB_DATABASE_NAME_MAX_BYTES: usize = 64;
static TEMPORARY_DATABASE_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

fn temporary_database_name(prefix: &str) -> String {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let sequence = TEMPORARY_DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let suffix = format!("_{:x}_{nonce:x}_{sequence:x}", std::process::id());
    let prefix_budget = MONGODB_DATABASE_NAME_MAX_BYTES
        .checked_sub(suffix.len() + 1)
        .expect("temporary database suffix leaves a readable prefix");
    let mut prefix_end = prefix.len().min(prefix_budget);
    while !prefix.is_char_boundary(prefix_end) {
        prefix_end -= 1;
    }
    let name = format!("{}{suffix}", &prefix[..prefix_end]);
    assert!(
        name.len() < MONGODB_DATABASE_NAME_MAX_BYTES,
        "temporary MongoDB database names must be shorter than 64 bytes"
    );
    name
}

#[test]
fn temporary_replica_set_database_names_are_bounded_readable_and_unique() {
    for prefix in [
        "chrono_stage_refuse_provenance_unknown_document",
        "chrono_stage_refuse_provenance_unknown_contract",
        "chrono_stage_refuse_in_progress_contract",
    ] {
        let first = temporary_database_name(prefix);
        let second = temporary_database_name(prefix);

        for name in [&first, &second] {
            assert!(name.len() < MONGODB_DATABASE_NAME_MAX_BYTES);
            assert!(name.is_ascii());
            let mut parts = name.rsplitn(4, '_');
            let sequence = parts.next().expect("temporary name has a sequence");
            let timestamp = parts.next().expect("temporary name has a timestamp");
            let process = parts.next().expect("temporary name has a process id");
            let readable = parts.next().expect("temporary name keeps its prefix");
            assert!(!readable.is_empty());
            assert!(prefix.starts_with(readable));
            for hexadecimal in [process, timestamp, sequence] {
                assert!(!hexadecimal.is_empty());
                assert!(hexadecimal.bytes().all(|byte| byte.is_ascii_hexdigit()));
            }
        }
        assert_ne!(first, second, "the compact suffix must prevent collisions");
    }
}

fn raw_legacy_session_document(record: &SessionRecord) -> Document {
    let mut document = mongodb::bson::serialize_to_document(&to_doc(record).unwrap()).unwrap();
    document.remove("revision");
    document.remove(WRITE_PROTOCOL_FIELD);
    document.remove(COORDINATION_PROTOCOL_FIELD);
    document
}

fn raw_legacy_event_document(event: &Event) -> Document {
    let mut document = event_doc::to_document(event).unwrap();
    document.remove(WRITE_PROTOCOL_FIELD);
    document
}

async fn create_pre_coordination_namespace_state(database: &Database) -> BTreeMap<String, Binary> {
    database
        .create_collection(PRE_COORDINATION_SESSIONS)
        .await
        .unwrap();
    database.create_collection(EVENTS).await.unwrap();
    database.create_collection(TOMBSTONES).await.unwrap();
    for collection in [PRE_COORDINATION_SESSIONS, EVENTS] {
        database
            .run_command(doc! {
                "collMod": collection,
                "validator": expected_validator(collection).unwrap(),
                "validationLevel": "strict",
                "validationAction": "error",
            })
            .await
            .unwrap();
    }
    database
        .create_collection(LEGACY_SESSIONS)
        .view_on(PRE_COORDINATION_SESSIONS)
        .pipeline(Vec::<Document>::new())
        .await
        .unwrap();
    database
        .create_collection(LEGACY_EVENTS)
        .view_on(EVENTS)
        .pipeline(Vec::<Document>::new())
        .await
        .unwrap();

    let contracts =
        collection_contract_snapshot(database, &[PRE_COORDINATION_SESSIONS, EVENTS, TOMBSTONES])
            .await;
    let collection_ids = BTreeMap::from([
        (
            PRE_COORDINATION_SESSIONS.to_owned(),
            contracts[PRE_COORDINATION_SESSIONS].uuid.clone().unwrap(),
        ),
        (EVENTS.to_owned(), contracts[EVENTS].uuid.clone().unwrap()),
        (
            TOMBSTONES.to_owned(),
            contracts[TOMBSTONES].uuid.clone().unwrap(),
        ),
    ]);
    database
        .collection::<Document>(NAMESPACE_CUTOVERS)
        .insert_one(
            namespace_cutover_marker_document(&NamespaceCutoverMarker::Complete {
                origin: "operator-drained".into(),
                collection_ids: collection_ids.clone(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    collection_ids
}

async fn downgrade_completed_coordination_protocol_for_test(
    store: &MongoSessionStore,
) -> CoordinationActivationMarker {
    let current = store
        .coordination_activation_marker()
        .await
        .unwrap()
        .expect("the test store has an activation marker");
    let CoordinationActivationMarker::Complete {
        origin,
        collection_ids,
    } = current
    else {
        panic!("the test store reached current coordination activation");
    };
    store
        .database
        .run_command(doc! {
            "collMod": SESSIONS,
            "validator": legacy_session_protocol_validator(),
            "validationLevel": "strict",
            "validationAction": "error",
        })
        .await
        .unwrap();
    store
        .database
        .run_command(doc! {
            "collMod": COORDINATION_RECEIPTS,
            "validator": legacy_coordination_receipt_protocol_validator(),
            "validationLevel": "strict",
            "validationAction": "error",
        })
        .await
        .unwrap();
    store
        .sessions
        .clone_with_type::<Document>()
        .update_many(
            doc! {},
            doc! {
                "$set": {
                    WRITE_PROTOCOL_FIELD: WRITE_PROTOCOL,
                    COORDINATION_PROTOCOL_FIELD: LEGACY_SESSION_COORDINATION_PROTOCOL,
                },
                "$max": { "revision": 0_i64 },
            },
        )
        .await
        .unwrap();
    let legacy = CoordinationActivationMarker::LegacyComplete {
        protocol: LEGACY_COORDINATION_ACTIVATION_PROTOCOL,
        origin,
        collection_ids,
    };
    store
        .database
        .collection::<Document>(NAMESPACE_CUTOVERS)
        .replace_one(
            doc! { "_id": COORDINATION_ACTIVATION_ID },
            coordination_activation_marker_document(&legacy).unwrap(),
        )
        .await
        .unwrap();
    legacy
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

    let error = match MongoSessionStore::open(&client, &database_name, false, false).await {
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
    for absent in [
        NAMESPACE_CUTOVERS,
        TOMBSTONES,
        PRE_COORDINATION_SESSIONS,
        SESSIONS,
        EVENTS,
    ] {
        assert!(!names_after.iter().any(|name| name == absent));
    }

    database.drop().await.unwrap();
}

async fn assert_legacy_stage_refuses_without_mutation(
    client: &Client,
    database_name: &str,
    expected_error: &str,
) {
    let database = client.database(database_name);
    let before = database_state_snapshot(&database).await;
    let error = MongoSessionStore::stage_legacy_cutover(client, database_name)
        .await
        .expect_err("the staged cutover must refuse this source");
    assert!(
        error.to_string().contains(expected_error),
        "expected {expected_error:?}, got {error}"
    );
    assert_eq!(
        database_state_snapshot(&database).await,
        before,
        "a refused staged cutover changed database state"
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_legacy_stage_is_idempotent_and_hands_the_same_history_to_coordination() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let database_name = temporary_database_name("chrono_legacy_stage");
    let database = client.database(&database_name);

    let pack = SkillPack {
        name: "staged-recovery".into(),
        version: Some("4.2".into()),
        files: vec![("SKILL.md".into(), b"staged recovery contract".to_vec())],
    };
    let digest = SkillPackDigest::for_packs(std::slice::from_ref(&pack));
    let granted_skills = vec![GrantedSkill {
        id: SkillId::new("staged-recovery"),
        version: Some("4.2".into()),
    }];
    let mut record = record_with(None);
    record.session.id = SessionId("sesn_staged_recovery".into());
    record.session.last_seq = 1;
    record.session.granted_skills = Some(granted_skills.clone());
    record.context_ref = Some("contexts/staged-recovery".into());
    record.skill_packs = SkillPackState::Stored { digest };
    database
        .collection::<Document>(LEGACY_SESSIONS)
        .insert_one(raw_legacy_session_document(&record))
        .await
        .expect("raw legacy session is seeded");
    database
        .collection::<Document>(LEGACY_EVENTS)
        .insert_one(raw_legacy_event_document(&event("sesn_staged_recovery", 1)))
        .await
        .expect("raw legacy event is seeded");
    database
        .collection::<SessionWriteDoc>(WRITES)
        .insert_one(SessionWriteDoc {
            id: "write_staged_recovery".into(),
            write_protocol: WRITE_PROTOCOL,
            session_id: record.session.id.0.clone(),
            revision: 1,
        })
        .await
        .expect("pre-existing write receipt is seeded");
    database
        .collection::<Document>(TOMBSTONES)
        .insert_one(doc! {
            "_id": "sesn_deleted_before_stage",
            WRITE_PROTOCOL_FIELD: WRITE_PROTOCOL,
        })
        .await
        .expect("pre-existing tombstone is seeded");
    database
        .collection::<Document>("operator_notes")
        .insert_one(doc! { "_id": "keep", "body": "unrelated" })
        .await
        .expect("unrelated data is seeded");

    let raw_contracts =
        collection_contract_snapshot(&database, &[LEGACY_SESSIONS, LEGACY_EVENTS, TOMBSTONES])
            .await;
    let raw_session_uuid = raw_contracts[LEGACY_SESSIONS].uuid.clone().unwrap();
    let raw_event_uuid = raw_contracts[LEGACY_EVENTS].uuid.clone().unwrap();
    let raw_tombstone_uuid = raw_contracts[TOMBSTONES].uuid.clone().unwrap();
    let writes_before = sorted_collection_documents(&database, WRITES).await;
    let tombstones_before = sorted_collection_documents(&database, TOMBSTONES).await;
    let unrelated_before = sorted_collection_documents(&database, "operator_notes").await;

    MongoSessionStore::stage_legacy_cutover(&client, &database_name)
        .await
        .expect("the raw legacy source reaches the staged v2 boundary");

    let staged_contracts = collection_contract_snapshot(
        &database,
        &[
            LEGACY_SESSIONS,
            PRE_COORDINATION_SESSIONS,
            LEGACY_EVENTS,
            EVENTS,
            TOMBSTONES,
            SESSIONS,
            COORDINATION_RECEIPTS,
        ],
    )
    .await;
    assert_eq!(
        staged_contracts[LEGACY_SESSIONS].kind,
        NamespaceKind::View {
            source: Some(PRE_COORDINATION_SESSIONS.into()),
            pipeline_is_empty: true,
        }
    );
    assert_eq!(
        staged_contracts[LEGACY_EVENTS].kind,
        NamespaceKind::View {
            source: Some(EVENTS.into()),
            pipeline_is_empty: true,
        }
    );
    assert!(!staged_contracts.contains_key(SESSIONS));
    assert!(!staged_contracts.contains_key(COORDINATION_RECEIPTS));
    let staged_ids = BTreeMap::from([
        (
            PRE_COORDINATION_SESSIONS.to_owned(),
            staged_contracts[PRE_COORDINATION_SESSIONS]
                .uuid
                .clone()
                .unwrap(),
        ),
        (
            EVENTS.to_owned(),
            staged_contracts[EVENTS].uuid.clone().unwrap(),
        ),
        (
            TOMBSTONES.to_owned(),
            staged_contracts[TOMBSTONES].uuid.clone().unwrap(),
        ),
    ]);
    assert_eq!(staged_ids[PRE_COORDINATION_SESSIONS], raw_session_uuid);
    assert_eq!(staged_ids[EVENTS], raw_event_uuid);
    assert_eq!(staged_ids[TOMBSTONES], raw_tombstone_uuid);
    validate_collection_contract(
        PRE_COORDINATION_SESSIONS,
        &staged_contracts[PRE_COORDINATION_SESSIONS],
        Some(&staged_ids[PRE_COORDINATION_SESSIONS]),
    )
    .expect("sessions_v2 has the exact strict/error contract");
    validate_collection_contract(EVENTS, &staged_contracts[EVENTS], Some(&staged_ids[EVENTS]))
        .expect("session_events_v2 has the exact strict/error contract");

    let marker_document = database
        .collection::<Document>(NAMESPACE_CUTOVERS)
        .find_one(doc! { "_id": NAMESPACE_CUTOVER_ID })
        .await
        .unwrap()
        .expect("stage completion provenance exists");
    assert_eq!(
        validate_namespace_cutover_marker(marker_document).unwrap(),
        NamespaceCutoverMarker::Complete {
            origin: "operator-drained".into(),
            collection_ids: staged_ids.clone(),
        }
    );
    for absent_marker in [
        COORDINATION_ACTIVATION_ID,
        COORDINATION_RECEIPTS_PROVENANCE_ID,
    ] {
        assert!(database
            .collection::<Document>(NAMESPACE_CUTOVERS)
            .find_one(doc! { "_id": absent_marker })
            .await
            .unwrap()
            .is_none());
    }
    let staged_session = database
        .collection::<Document>(PRE_COORDINATION_SESSIONS)
        .find_one(doc! { "_id": &record.session.id.0 })
        .await
        .unwrap()
        .expect("staged session remains present");
    assert!(!staged_session.contains_key(COORDINATION_PROTOCOL_FIELD));
    assert_eq!(
        validate_compatible_stored_session_document(&staged_session).unwrap(),
        (record.session.id.0.clone(), 1)
    );
    let staged_event = database
        .collection::<Document>(EVENTS)
        .find_one(doc! { "session_id": &record.session.id.0, "seq": 1_i64 })
        .await
        .unwrap()
        .expect("staged event remains present");
    let staged_event = event_doc::from_document(staged_event).unwrap();
    let expected_event = event("sesn_staged_recovery", 1);
    assert_eq!(staged_event.session_id, expected_event.session_id);
    assert_eq!(staged_event.seq, expected_event.seq);
    assert_eq!(staged_event.appended_at, expected_event.appended_at);
    assert!(matches!(
        staged_event.kind,
        StoredEventKind::Known(EventKind::SessionCreated)
    ));
    assert_eq!(
        sorted_collection_documents(&database, WRITES).await,
        writes_before
    );
    assert_eq!(
        sorted_collection_documents(&database, TOMBSTONES).await,
        tombstones_before
    );
    assert_eq!(
        sorted_collection_documents(&database, "operator_notes").await,
        unrelated_before
    );

    let exact_stage_boundary = database_state_snapshot(&database).await;
    MongoSessionStore::stage_legacy_cutover(&client, &database_name)
        .await
        .expect("an exact completed stage is a verified no-op");
    assert_eq!(
        database_state_snapshot(&database).await,
        exact_stage_boundary,
        "stage re-entry must be byte- and contract-idempotent"
    );

    let store = MongoSessionStore::open(&client, &database_name, false, true)
        .await
        .expect("coordination activates from the exact staged boundary");
    assert_eq!(
        store.collection_ids[SESSIONS],
        staged_ids[PRE_COORDINATION_SESSIONS]
    );
    assert_eq!(store.collection_ids[EVENTS], staged_ids[EVENTS]);
    assert_eq!(store.collection_ids[TOMBSTONES], staged_ids[TOMBSTONES]);
    let durable = store
        .get(&record.session.id)
        .await
        .unwrap()
        .expect("the coordinated session remains present");
    assert_eq!(durable.session.granted_skills, Some(granted_skills));
    assert_eq!(durable.skill_packs, SkillPackState::Stored { digest });
    assert_eq!(durable.context_ref, record.context_ref);
    assert_eq!(
        store
            .events_since(&record.session.id, 0)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(store
        .writes
        .find_one(doc! { "_id": "write_staged_recovery" })
        .await
        .unwrap()
        .is_some());
    assert!(store
        .tombstones
        .find_one(doc! { "_id": "sesn_deleted_before_stage" })
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        sorted_collection_documents(&database, WRITES).await,
        writes_before
    );
    assert_eq!(
        sorted_collection_documents(&database, TOMBSTONES).await,
        tombstones_before
    );
    assert_eq!(
        sorted_collection_documents(&database, "operator_notes").await,
        unrelated_before
    );
    let coordinated_session = store
        .sessions
        .clone_with_type::<Document>()
        .find_one(doc! { "_id": &record.session.id.0 })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        coordinated_session.get_i32(WRITE_PROTOCOL_FIELD).unwrap(),
        SESSION_WRITE_PROTOCOL
    );
    assert_eq!(
        coordinated_session
            .get_i32(COORDINATION_PROTOCOL_FIELD)
            .unwrap(),
        SESSION_COORDINATION_PROTOCOL
    );
    store.verify_coordinated_namespaces().await.unwrap();
    store
        .verify_coordination_activation(store.collection_ids.as_ref())
        .await
        .unwrap();
    store
        .verify_coordination_receipts_provenance_ids(store.collection_ids.as_ref())
        .await
        .unwrap();

    drop(store);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_legacy_stage_binds_legitimately_absent_sources_and_resumes_completion() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let database_name = temporary_database_name("chrono_legacy_stage_absent_sources");
    let database = client.database(&database_name);

    let mut record = record_with(None);
    record.session.id = SessionId("sesn_stage_absent_sources".into());
    record.session.last_seq = 0;
    database
        .collection::<Document>(LEGACY_SESSIONS)
        .insert_one(raw_legacy_session_document(&record))
        .await
        .unwrap();
    let raw_session_id = collection_contract_snapshot(&database, &[LEGACY_SESSIONS]).await
        [LEGACY_SESSIONS]
        .uuid
        .clone()
        .unwrap();

    MongoSessionStore::stage_legacy_cutover(&client, &database_name)
        .await
        .expect("the stage creates authoritative state for absent event and tombstone sources");
    let complete = MongoSessionStore::new(&client, &database_name)
        .namespace_cutover_marker()
        .await
        .unwrap()
        .expect("completion provenance exists");
    let NamespaceCutoverMarker::Complete { collection_ids, .. } = complete else {
        panic!("the first stage run completed its provenance");
    };
    assert_eq!(collection_ids.len(), LEGACY_CUTOVER_COLLECTIONS.len());
    assert_eq!(collection_ids[PRE_COORDINATION_SESSIONS], raw_session_id);
    assert_eq!(
        database
            .collection::<Document>(EVENTS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        database
            .collection::<Document>(TOMBSTONES)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );

    let in_progress = NamespaceCutoverMarker::InProgress {
        origin: "operator-drained".into(),
        source_ids: BTreeMap::from([
            (LEGACY_SESSIONS.to_owned(), Some(raw_session_id)),
            (LEGACY_EVENTS.to_owned(), None),
            (TOMBSTONES.to_owned(), None),
        ]),
        tombstone_id: Some(collection_ids[TOMBSTONES].clone()),
    };
    database
        .collection::<Document>(NAMESPACE_CUTOVERS)
        .replace_one(
            doc! { "_id": NAMESPACE_CUTOVER_ID },
            namespace_cutover_marker_document(&in_progress).unwrap(),
        )
        .await
        .unwrap();
    MongoSessionStore::stage_legacy_cutover(&client, &database_name)
        .await
        .expect("both complete views resume through the exact tombstone-bound CAS");
    assert_eq!(
        MongoSessionStore::new(&client, &database_name)
            .namespace_cutover_marker()
            .await
            .unwrap(),
        Some(NamespaceCutoverMarker::Complete {
            origin: "operator-drained".into(),
            collection_ids: collection_ids.clone(),
        })
    );

    let exact_boundary = database_state_snapshot(&database).await;
    MongoSessionStore::stage_legacy_cutover(&client, &database_name)
        .await
        .expect("the completed absent-source boundary reruns read-only");
    assert_eq!(database_state_snapshot(&database).await, exact_boundary);

    database
        .collection::<Document>(TOMBSTONES)
        .drop()
        .await
        .unwrap();
    database.create_collection(TOMBSTONES).await.unwrap();
    assert_legacy_stage_refuses_without_mutation(
        &client,
        &database_name,
        "UUID no longer matches its durable completion record",
    )
    .await;

    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_legacy_stage_refuses_every_unproved_or_post_boundary_state_without_mutation() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");

    let coordinated_name = temporary_database_name("chrono_stage_refuse_coordinated");
    let coordinated = client.database(&coordinated_name);
    let coordinated_store = MongoSessionStore::open(&client, &coordinated_name, false, false)
        .await
        .expect("test coordinated database opens");
    drop(coordinated_store);
    assert_legacy_stage_refuses_without_mutation(
        &client,
        &coordinated_name,
        "existing sessions_v3 namespace",
    )
    .await;
    coordinated.drop().await.unwrap();

    let markerless_name = temporary_database_name("chrono_stage_refuse_markerless_complete");
    let markerless = client.database(&markerless_name);
    create_pre_coordination_namespace_state(&markerless).await;
    markerless
        .collection::<Document>(NAMESPACE_CUTOVERS)
        .drop()
        .await
        .unwrap();
    assert_legacy_stage_refuses_without_mutation(
        &client,
        &markerless_name,
        "markerless sessions_v2",
    )
    .await;
    markerless.drop().await.unwrap();

    let wrong_origin_name = temporary_database_name("chrono_stage_refuse_wrong_origin");
    let wrong_origin = client.database(&wrong_origin_name);
    wrong_origin
        .create_collection(LEGACY_SESSIONS)
        .await
        .unwrap();
    wrong_origin
        .collection::<Document>(NAMESPACE_CUTOVERS)
        .insert_one(
            namespace_cutover_marker_document(&NamespaceCutoverMarker::InProgress {
                origin: "new-database".into(),
                source_ids: test_namespace_cutover_source_ids(None, None, None),
                tombstone_id: None,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    assert_legacy_stage_refuses_without_mutation(
        &client,
        &wrong_origin_name,
        "exact operator-drained provenance",
    )
    .await;
    wrong_origin.drop().await.unwrap();

    for (suffix, marker, expected) in [
        (
            "protocol",
            doc! {
                "_id": NAMESPACE_CUTOVER_ID,
                "protocol": NAMESPACE_CUTOVER_PROTOCOL + 1,
                "origin": "operator-drained",
                "phase": "in-progress",
            },
            "not the supported sessions-v2 protocol",
        ),
        (
            "fields",
            doc! {
                "_id": NAMESPACE_CUTOVER_ID,
                "protocol": NAMESPACE_CUTOVER_PROTOCOL,
                "origin": "operator-drained",
                "phase": "in-progress",
                "unexpected": true,
            },
            "unsupported phase or field set",
        ),
    ] {
        let name = temporary_database_name(&format!("chrono_stage_refuse_marker_{suffix}"));
        let database = client.database(&name);
        database.create_collection(LEGACY_SESSIONS).await.unwrap();
        database
            .collection::<Document>(NAMESPACE_CUTOVERS)
            .insert_one(marker)
            .await
            .unwrap();
        assert_legacy_stage_refuses_without_mutation(&client, &name, expected).await;
        database.drop().await.unwrap();
    }

    for suffix in ["empty", "unknown_document", "unknown_contract"] {
        let name = temporary_database_name(&format!("chrono_stage_refuse_provenance_{suffix}"));
        let database = client.database(&name);
        database.create_collection(LEGACY_SESSIONS).await.unwrap();
        database
            .create_collection(NAMESPACE_CUTOVERS)
            .await
            .unwrap();
        let expected = match suffix {
            "empty" => "existing empty provenance namespace",
            "unknown_document" => {
                database
                    .collection::<Document>(NAMESPACE_CUTOVERS)
                    .insert_one(doc! { "_id": "unproved-stage-state" })
                    .await
                    .unwrap();
                "unproved document"
            }
            "unknown_contract" => {
                database
                    .run_command(doc! {
                        "collMod": NAMESPACE_CUTOVERS,
                        "validator": { "unexpected": true },
                        "validationLevel": "strict",
                        "validationAction": "error",
                    })
                    .await
                    .unwrap();
                "unproved validator contract"
            }
            _ => unreachable!(),
        };
        assert_legacy_stage_refuses_without_mutation(&client, &name, expected).await;
        database.drop().await.unwrap();
    }

    let ambiguous_name = temporary_database_name("chrono_stage_refuse_ambiguous");
    let ambiguous = client.database(&ambiguous_name);
    ambiguous.create_collection(LEGACY_SESSIONS).await.unwrap();
    ambiguous
        .create_collection(PRE_COORDINATION_SESSIONS)
        .await
        .unwrap();
    assert_legacy_stage_refuses_without_mutation(
        &client,
        &ambiguous_name,
        "markerless sessions_v2",
    )
    .await;
    ambiguous.drop().await.unwrap();

    let partial_name = temporary_database_name("chrono_stage_refuse_partial");
    let partial = client.database(&partial_name);
    partial.create_collection(LEGACY_SESSIONS).await.unwrap();
    partial.create_collection(EVENTS).await.unwrap();
    assert_legacy_stage_refuses_without_mutation(
        &client,
        &partial_name,
        "markerless session_events_v2",
    )
    .await;
    partial.drop().await.unwrap();

    for (suffix, validator, level, action) in [
        (
            "validator",
            doc! { "unexpected": { "$exists": true } },
            "strict",
            "error",
        ),
        ("level", Document::new(), "off", "error"),
        ("action", Document::new(), "strict", "warn"),
    ] {
        let name = temporary_database_name(&format!("chrono_stage_refuse_raw_{suffix}"));
        let database = client.database(&name);
        database.create_collection(LEGACY_SESSIONS).await.unwrap();
        database
            .run_command(doc! {
                "collMod": LEGACY_SESSIONS,
                "validator": validator,
                "validationLevel": level,
                "validationAction": action,
            })
            .await
            .unwrap();
        assert_legacy_stage_refuses_without_mutation(&client, &name, "unproved validator contract")
            .await;
        database.drop().await.unwrap();
    }

    let in_progress_name = temporary_database_name("chrono_stage_refuse_in_progress_contract");
    let in_progress = client.database(&in_progress_name);
    in_progress
        .create_collection(LEGACY_SESSIONS)
        .await
        .unwrap();
    in_progress
        .run_command(doc! {
            "collMod": LEGACY_SESSIONS,
            "validator": { "unexpected": true },
            "validationLevel": "strict",
            "validationAction": "error",
        })
        .await
        .unwrap();
    let in_progress_source_id = collection_contract_snapshot(&in_progress, &[LEGACY_SESSIONS])
        .await[LEGACY_SESSIONS]
        .uuid
        .clone()
        .unwrap();
    in_progress
        .collection::<Document>(NAMESPACE_CUTOVERS)
        .insert_one(
            namespace_cutover_marker_document(&NamespaceCutoverMarker::InProgress {
                origin: "operator-drained".into(),
                source_ids: BTreeMap::from([
                    (LEGACY_SESSIONS.to_owned(), Some(in_progress_source_id)),
                    (LEGACY_EVENTS.to_owned(), None),
                    (TOMBSTONES.to_owned(), None),
                ]),
                tombstone_id: None,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    assert_legacy_stage_refuses_without_mutation(
        &client,
        &in_progress_name,
        "unproved validator contract",
    )
    .await;
    in_progress.drop().await.unwrap();

    let missing_source_name = temporary_database_name("chrono_stage_refuse_source_loss");
    let missing_source = client.database(&missing_source_name);
    missing_source
        .create_collection(LEGACY_SESSIONS)
        .await
        .unwrap();
    MongoSessionStore::new(&client, &missing_source_name)
        .record_namespace_cutover_marker("operator-drained")
        .await
        .unwrap();
    missing_source
        .collection::<Document>(LEGACY_SESSIONS)
        .drop()
        .await
        .unwrap();
    assert_legacy_stage_refuses_without_mutation(
        &client,
        &missing_source_name,
        "disappeared after its UUID was durably recorded",
    )
    .await;
    missing_source.drop().await.unwrap();

    let replaced_source_name = temporary_database_name("chrono_stage_refuse_source_replacement");
    let replaced_source = client.database(&replaced_source_name);
    replaced_source
        .create_collection(LEGACY_SESSIONS)
        .await
        .unwrap();
    MongoSessionStore::new(&client, &replaced_source_name)
        .record_namespace_cutover_marker("operator-drained")
        .await
        .unwrap();
    replaced_source
        .collection::<Document>(LEGACY_SESSIONS)
        .drop()
        .await
        .unwrap();
    replaced_source
        .create_collection(LEGACY_SESSIONS)
        .await
        .unwrap();
    assert_legacy_stage_refuses_without_mutation(
        &client,
        &replaced_source_name,
        "UUID no longer matches its durable provenance",
    )
    .await;
    replaced_source.drop().await.unwrap();

    let appeared_source_name = temporary_database_name("chrono_stage_refuse_source_appearance");
    let appeared_source = client.database(&appeared_source_name);
    appeared_source
        .create_collection(LEGACY_SESSIONS)
        .await
        .unwrap();
    MongoSessionStore::new(&client, &appeared_source_name)
        .record_namespace_cutover_marker("operator-drained")
        .await
        .unwrap();
    appeared_source
        .create_collection(LEGACY_EVENTS)
        .await
        .unwrap();
    assert_legacy_stage_refuses_without_mutation(
        &client,
        &appeared_source_name,
        "was durably absent but a collection appeared",
    )
    .await;
    appeared_source.drop().await.unwrap();

    let missing_bound_tombstone_name =
        temporary_database_name("chrono_stage_refuse_bound_tombstone_loss");
    let missing_bound_tombstone = client.database(&missing_bound_tombstone_name);
    missing_bound_tombstone
        .create_collection(LEGACY_SESSIONS)
        .await
        .unwrap();
    missing_bound_tombstone
        .create_collection(TOMBSTONES)
        .await
        .unwrap();
    MongoSessionStore::new(&client, &missing_bound_tombstone_name)
        .record_namespace_cutover_marker("operator-drained")
        .await
        .unwrap();
    missing_bound_tombstone
        .collection::<Document>(TOMBSTONES)
        .drop()
        .await
        .unwrap();
    assert_legacy_stage_refuses_without_mutation(
        &client,
        &missing_bound_tombstone_name,
        "disappeared after its UUID was durably recorded",
    )
    .await;
    missing_bound_tombstone.drop().await.unwrap();

    let replaced_bound_tombstone_name =
        temporary_database_name("chrono_stage_refuse_bound_tombstone_replacement");
    let replaced_bound_tombstone = client.database(&replaced_bound_tombstone_name);
    replaced_bound_tombstone
        .create_collection(LEGACY_SESSIONS)
        .await
        .unwrap();
    replaced_bound_tombstone
        .create_collection(TOMBSTONES)
        .await
        .unwrap();
    MongoSessionStore::new(&client, &replaced_bound_tombstone_name)
        .record_namespace_cutover_marker("operator-drained")
        .await
        .unwrap();
    replaced_bound_tombstone
        .collection::<Document>(TOMBSTONES)
        .drop()
        .await
        .unwrap();
    replaced_bound_tombstone
        .create_collection(TOMBSTONES)
        .await
        .unwrap();
    assert_legacy_stage_refuses_without_mutation(
        &client,
        &replaced_bound_tombstone_name,
        "UUID no longer matches its in-progress provenance",
    )
    .await;
    replaced_bound_tombstone.drop().await.unwrap();

    let ambiguous_tombstone_name =
        temporary_database_name("chrono_stage_refuse_unbound_tombstone_appearance");
    let ambiguous_tombstone = client.database(&ambiguous_tombstone_name);
    ambiguous_tombstone
        .create_collection(LEGACY_SESSIONS)
        .await
        .unwrap();
    MongoSessionStore::new(&client, &ambiguous_tombstone_name)
        .record_namespace_cutover_marker("operator-drained")
        .await
        .unwrap();
    ambiguous_tombstone
        .create_collection(TOMBSTONES)
        .await
        .unwrap();
    assert_legacy_stage_refuses_without_mutation(
        &client,
        &ambiguous_tombstone_name,
        "refusing ambiguous DDL-crash or replacement state",
    )
    .await;
    ambiguous_tombstone.drop().await.unwrap();

    let missing_tombstones_name = temporary_database_name("chrono_stage_refuse_tombstone_loss");
    let missing_tombstones = client.database(&missing_tombstones_name);
    create_pre_coordination_namespace_state(&missing_tombstones).await;
    missing_tombstones
        .collection::<Document>(TOMBSTONES)
        .drop()
        .await
        .unwrap();
    assert_legacy_stage_refuses_without_mutation(
        &client,
        &missing_tombstones_name,
        "cannot be marked complete without collection session_tombstones",
    )
    .await;
    missing_tombstones.drop().await.unwrap();

    let replaced_tombstones_name =
        temporary_database_name("chrono_stage_refuse_tombstone_replacement");
    let replaced_tombstones = client.database(&replaced_tombstones_name);
    create_pre_coordination_namespace_state(&replaced_tombstones).await;
    replaced_tombstones
        .collection::<Document>(TOMBSTONES)
        .drop()
        .await
        .unwrap();
    replaced_tombstones
        .create_collection(TOMBSTONES)
        .await
        .unwrap();
    assert_legacy_stage_refuses_without_mutation(
        &client,
        &replaced_tombstones_name,
        "UUID no longer matches its durable completion record",
    )
    .await;
    replaced_tombstones.drop().await.unwrap();

    let wrong_tombstones_name = temporary_database_name("chrono_stage_refuse_tombstone_contract");
    let wrong_tombstones = client.database(&wrong_tombstones_name);
    create_pre_coordination_namespace_state(&wrong_tombstones).await;
    wrong_tombstones
        .run_command(doc! {
            "collMod": TOMBSTONES,
            "validator": { "unexpected": true },
            "validationLevel": "strict",
            "validationAction": "error",
        })
        .await
        .unwrap();
    assert_legacy_stage_refuses_without_mutation(
        &client,
        &wrong_tombstones_name,
        "unproved validator contract",
    )
    .await;
    wrong_tombstones.drop().await.unwrap();

    let malformed_tombstones_name =
        temporary_database_name("chrono_stage_refuse_tombstone_document");
    let malformed_tombstones = client.database(&malformed_tombstones_name);
    create_pre_coordination_namespace_state(&malformed_tombstones).await;
    malformed_tombstones
        .collection::<Document>(TOMBSTONES)
        .insert_one(doc! {
            "_id": "sesn_unproved_tombstone",
            WRITE_PROTOCOL_FIELD: WRITE_PROTOCOL + 1,
        })
        .await
        .unwrap();
    assert_legacy_stage_refuses_without_mutation(
        &client,
        &malformed_tombstones_name,
        "does not carry write protocol",
    )
    .await;
    malformed_tombstones.drop().await.unwrap();

    let wrong_v2_name = temporary_database_name("chrono_stage_refuse_wrong_v2_contract");
    let wrong_v2 = client.database(&wrong_v2_name);
    create_pre_coordination_namespace_state(&wrong_v2).await;
    wrong_v2
        .run_command(doc! {
            "collMod": PRE_COORDINATION_SESSIONS,
            "validator": session_protocol_validator(),
            "validationLevel": "strict",
            "validationAction": "error",
        })
        .await
        .unwrap();
    assert_legacy_stage_refuses_without_mutation(
        &client,
        &wrong_v2_name,
        "changed write-protocol validator",
    )
    .await;
    wrong_v2.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_existing_database_requires_the_coordination_drain_gate() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database_name = format!("chrono_coordination_gate_{}_{}", std::process::id(), nonce);
    let database = client.database(&database_name);
    let old_ids = create_pre_coordination_namespace_state(&database).await;

    let error = match MongoSessionStore::open(&client, &database_name, false, false).await {
        Ok(_) => panic!("an existing database must require the coordination drain gate"),
        Err(error) => error,
    };
    assert!(error
        .to_string()
        .contains("CHRONO_MONGODB_COORDINATION_CUTOVER=drained"));
    assert!(database
        .collection::<Document>(NAMESPACE_CUTOVERS)
        .find_one(doc! { "_id": COORDINATION_ACTIVATION_ID })
        .await
        .unwrap()
        .is_none());
    let contracts =
        collection_contract_snapshot(&database, &[PRE_COORDINATION_SESSIONS, SESSIONS, EVENTS])
            .await;
    assert_eq!(
        contracts[PRE_COORDINATION_SESSIONS].uuid.as_ref(),
        old_ids.get(PRE_COORDINATION_SESSIONS)
    );
    assert!(!contracts.contains_key(SESSIONS));

    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_previous_session_writer_requires_a_drain_and_preserves_skill_ownership() {
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
        "chrono_session_writer_upgrade_{}_{}",
        std::process::id(),
        nonce
    );
    let database = client.database(&database_name);
    let store = MongoSessionStore::open(&client, &database_name, false, false)
        .await
        .expect("test store opens");

    let pack = SkillPack {
        name: "durable-review".into(),
        version: Some("7.4".into()),
        files: vec![("SKILL.md".into(), b"durable review body".to_vec())],
    };
    let digest = SkillPackDigest::for_packs(std::slice::from_ref(&pack));
    let granted = vec![GrantedSkill {
        id: SkillId::new("durable-review"),
        version: Some("7.4".into()),
    }];
    let mut record = record_with(None);
    record.session.id = SessionId("sesn_session_writer_upgrade".into());
    record.session.last_seq = 0;
    record.session.granted_skills = Some(granted.clone());
    record.context_ref = Some("contexts/session-writer-upgrade".into());
    record.skill_packs = SkillPackState::Stored { digest };
    store
        .create_session_idempotent("seed-session-writer-upgrade", &record, &[])
        .await
        .expect("protected session is seeded");

    database
        .run_command(doc! {
            "collMod": SESSIONS,
            "validator": previous_session_protocol_validator(),
            "validationLevel": "strict",
            "validationAction": "error",
        })
        .await
        .expect("test installs the exact predecessor contract");
    store
        .sessions
        .clone_with_type::<Document>()
        .update_one(
            doc! { "_id": &record.session.id.0 },
            doc! { "$set": { WRITE_PROTOCOL_FIELD: WRITE_PROTOCOL } },
        )
        .await
        .expect("test rewinds the row to the predecessor writer");
    drop(store);

    let contracts_before = collection_contract_snapshot(&database, &[SESSIONS]).await;
    let sessions_before = sorted_collection_documents(&database, SESSIONS).await;
    let error = match MongoSessionStore::open(&client, &database_name, false, false).await {
        Ok(_) => panic!("the predecessor session writer requires an explicit drain"),
        Err(error) => error,
    };
    assert!(error
        .to_string()
        .contains("CHRONO_MONGODB_COORDINATION_CUTOVER=drained"));
    assert_eq!(
        collection_contract_snapshot(&database, &[SESSIONS]).await,
        contracts_before,
        "refusing the cutover cannot install a validator"
    );
    assert_eq!(
        sorted_collection_documents(&database, SESSIONS).await,
        sessions_before,
        "refusing the cutover cannot migrate a row"
    );

    let upgraded = MongoSessionStore::open(&client, &database_name, false, true)
        .await
        .expect("the drained predecessor upgrades");
    let contracts = collection_contract_snapshot(&database, &[SESSIONS]).await;
    assert_eq!(
        classify_session_write_contract(&contracts[SESSIONS], &upgraded.collection_ids[SESSIONS])
            .unwrap(),
        SessionWriteContract::Current
    );
    let durable = upgraded
        .get(&record.session.id)
        .await
        .unwrap()
        .expect("migrated session remains present");
    assert_eq!(durable.session.granted_skills, Some(granted));
    assert_eq!(durable.skill_packs, SkillPackState::Stored { digest });
    assert_eq!(durable.context_ref, record.context_ref);

    let current = upgraded
        .sessions
        .clone_with_type::<Document>()
        .find_one(doc! { "_id": &record.session.id.0 })
        .await
        .unwrap()
        .expect("migrated document remains present");
    assert_eq!(
        current.get_i32(WRITE_PROTOCOL_FIELD).unwrap(),
        SESSION_WRITE_PROTOCOL
    );
    let mut stale_whole_replacement = current.clone();
    stale_whole_replacement.insert(WRITE_PROTOCOL_FIELD, WRITE_PROTOCOL);
    assert!(stale_whole_replacement.remove("granted_skills").is_some());
    assert!(stale_whole_replacement.remove("skill_packs").is_some());
    assert!(upgraded
        .sessions
        .clone_with_type::<Document>()
        .replace_one(
            doc! { "_id": &record.session.id.0 },
            stale_whole_replacement,
        )
        .await
        .is_err());
    assert_eq!(
        upgraded
            .sessions
            .clone_with_type::<Document>()
            .find_one(doc! { "_id": &record.session.id.0 })
            .await
            .unwrap()
            .unwrap(),
        current,
        "a protocol-1 whole replacement cannot erase grant or custody fields"
    );

    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_current_session_validator_resumes_a_stale_protocol_one_row_without_a_gate() {
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
        "chrono_session_writer_resume_{}_{}",
        std::process::id(),
        nonce
    );
    let database = client.database(&database_name);
    let store = MongoSessionStore::open(&client, &database_name, false, false)
        .await
        .expect("test store opens");
    let mut record = record_with(None);
    record.session.id = SessionId("sesn_session_writer_resume".into());
    record.session.last_seq = 0;
    record.session.granted_skills = Some(vec![GrantedSkill {
        id: SkillId::new("resume-review"),
        version: Some("2".into()),
    }]);
    record.skill_packs = SkillPackState::CleanupPending;
    store
        .create_session_idempotent("seed-session-writer-resume", &record, &[])
        .await
        .unwrap();
    store
        .sessions
        .clone_with_type::<Document>()
        .update_one(
            doc! { "_id": &record.session.id.0 },
            doc! { "$set": { WRITE_PROTOCOL_FIELD: WRITE_PROTOCOL } },
        )
        .bypass_document_validation(true)
        .await
        .expect("test models a crash after the current validator landed");
    let mut expected = store
        .sessions
        .clone_with_type::<Document>()
        .find_one(doc! { "_id": &record.session.id.0 })
        .await
        .unwrap()
        .unwrap();
    expected.insert(WRITE_PROTOCOL_FIELD, SESSION_WRITE_PROTOCOL);
    drop(store);

    let resumed = MongoSessionStore::open(&client, &database_name, false, false)
        .await
        .expect("the current fence resumes its incomplete row migration without a gate");
    assert_eq!(
        resumed
            .sessions
            .clone_with_type::<Document>()
            .find_one(doc! { "_id": &record.session.id.0 })
            .await
            .unwrap()
            .unwrap(),
        expected
    );
    let durable = resumed.get(&record.session.id).await.unwrap().unwrap();
    assert_eq!(
        durable.session.granted_skills,
        record.session.granted_skills
    );
    assert_eq!(durable.skill_packs, SkillPackState::CleanupPending);

    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_unknown_or_weakened_session_contracts_fail_without_mutation() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();

    for scenario in ["unknown", "mixed", "weakened"] {
        let database_name = format!(
            "chrono_session_contract_{scenario}_{}_{}",
            std::process::id(),
            nonce
        );
        let database = client.database(&database_name);
        let store = MongoSessionStore::open(&client, &database_name, false, false)
            .await
            .expect("test store opens");
        let mut record = record_with(None);
        record.session.id = SessionId(format!("sesn_session_contract_{scenario}"));
        record.session.last_seq = 0;
        record.skill_packs = SkillPackState::CleanupPending;
        store
            .create_session_idempotent(&format!("seed-session-contract-{scenario}"), &record, &[])
            .await
            .unwrap();

        let mut validator = match scenario {
            "unknown" => session_protocol_validator_for(
                SESSION_WRITE_PROTOCOL + 1,
                SESSION_COORDINATION_PROTOCOL,
            ),
            "mixed" | "weakened" => session_protocol_validator(),
            _ => unreachable!(),
        };
        if scenario == "mixed" {
            *validator
                .get_document_mut("$jsonSchema")
                .unwrap()
                .get_document_mut("properties")
                .unwrap()
                .get_document_mut(WRITE_PROTOCOL_FIELD)
                .unwrap()
                .get_array_mut("enum")
                .unwrap() = vec![
                Bson::Int32(WRITE_PROTOCOL),
                Bson::Int32(SESSION_WRITE_PROTOCOL),
            ];
        }
        database
            .run_command(doc! {
                "collMod": SESSIONS,
                "validator": validator,
                "validationLevel": "strict",
                "validationAction": if scenario == "weakened" { "warn" } else { "error" },
            })
            .await
            .unwrap();
        drop(store);

        let contracts_before = collection_contract_snapshot(&database, &[SESSIONS]).await;
        let sessions_before = sorted_collection_documents(&database, SESSIONS).await;
        let cutovers_before = sorted_collection_documents(&database, NAMESPACE_CUTOVERS).await;
        let error = match MongoSessionStore::open(&client, &database_name, false, true).await {
            Ok(_) => panic!("{scenario} session contract must fail closed"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains("neither the exact current nor previous write-protocol contract"));
        assert_eq!(
            collection_contract_snapshot(&database, &[SESSIONS]).await,
            contracts_before,
            "startup changed the {scenario} contract"
        );
        assert_eq!(
            sorted_collection_documents(&database, SESSIONS).await,
            sessions_before,
            "startup changed rows under the {scenario} contract"
        );
        assert_eq!(
            sorted_collection_documents(&database, NAMESPACE_CUTOVERS).await,
            cutovers_before,
            "startup changed provenance under the {scenario} contract"
        );
        database.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_receipt_validator_rejects_legacy_protocol_and_missing_runtime_markers() {
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
        "chrono_receipt_marker_validator_{}_{}",
        std::process::id(),
        nonce
    );
    let database = client.database(&database_name);
    let store = MongoSessionStore::open(&client, &database_name, false, false)
        .await
        .unwrap();
    let receipts = store.receipts.clone_with_type::<Document>();

    for (token, protocol, switch) in [
        ("current-unswitched", COORDINATION_RECEIPT_PROTOCOL, None),
        (
            "current-null-switch",
            COORDINATION_RECEIPT_PROTOCOL,
            Some(Bson::Null),
        ),
        (
            "current-completed-with-marker",
            COORDINATION_RECEIPT_PROTOCOL,
            Some(Bson::Document(doc! { "runtime_started_seq": 1_i64 })),
        ),
    ] {
        let mut value = Document::new();
        if let Some(switch) = switch {
            value.insert("switch", switch);
        }
        receipts
            .insert_one(doc! {
                "_id": {
                    "session_id": "sesn_receipt_validator",
                    "kind": "reservation",
                    "token": token,
                },
                WRITE_PROTOCOL_FIELD: WRITE_PROTOCOL,
                COORDINATION_PROTOCOL_FIELD: protocol,
                "payload": {
                    "type": "reservation",
                    "value": value,
                },
            })
            .await
            .unwrap_or_else(|error| panic!("accepted receipt {token}: {error}"));
    }

    for (token, protocol, switch) in [
        (
            "legacy-unswitched",
            LEGACY_COORDINATION_RECEIPT_PROTOCOL,
            None,
        ),
        (
            "legacy-completed-with-marker",
            LEGACY_COORDINATION_RECEIPT_PROTOCOL,
            Some(Bson::Document(doc! { "runtime_started_seq": 1_i64 })),
        ),
        (
            "current-missing-marker",
            COORDINATION_RECEIPT_PROTOCOL,
            Some(Bson::Document(doc! {})),
        ),
        (
            "current-int-marker",
            COORDINATION_RECEIPT_PROTOCOL,
            Some(Bson::Document(doc! { "runtime_started_seq": 1_i32 })),
        ),
        (
            "current-zero-marker",
            COORDINATION_RECEIPT_PROTOCOL,
            Some(Bson::Document(doc! { "runtime_started_seq": 0_i64 })),
        ),
        (
            "current-negative-marker",
            COORDINATION_RECEIPT_PROTOCOL,
            Some(Bson::Document(doc! { "runtime_started_seq": -1_i64 })),
        ),
    ] {
        let mut value = Document::new();
        if let Some(switch) = switch {
            value.insert("switch", switch);
        }
        assert!(
            receipts
                .insert_one(doc! {
                    "_id": {
                        "session_id": "sesn_receipt_validator",
                        "kind": "reservation",
                        "token": token,
                    },
                    WRITE_PROTOCOL_FIELD: WRITE_PROTOCOL,
                    COORDINATION_PROTOCOL_FIELD: protocol,
                    "payload": {
                        "type": "reservation",
                        "value": value,
                    },
                })
                .await
                .is_err(),
            "invalid receipt {token} must fail validation"
        );
    }

    assert_eq!(receipts.count_documents(doc! {}).await.unwrap(), 3);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_v2_activation_is_rejected_without_mutating_the_database() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database_name = format!("chrono_coordination_v3_{}_{}", std::process::id(), nonce);
    let database = client.database(&database_name);
    let store = MongoSessionStore::open(&client, &database_name, false, false)
        .await
        .unwrap();
    let mut record = record_with(None);
    record.session.id = SessionId("sesn_coordination_v3_upgrade".into());
    record.session.last_seq = 0;
    record.runtime_id = Some("runtime-v1".into());
    store
        .create_session_idempotent("seed-coordination-v2-upgrade", &record, &[])
        .await
        .unwrap();
    store
        .sessions
        .clone_with_type::<Document>()
        .update_one(
            doc! { "_id": &record.session.id.0 },
            doc! {
                "$set": {
                    "coordination.conversation_epoch": 7_i64,
                    "coordination.conversation_link_token": "fixture",
                },
            },
        )
        .await
        .unwrap();
    downgrade_completed_coordination_protocol_for_test(&store).await;
    let cutovers_before = sorted_collection_documents(&database, NAMESPACE_CUTOVERS).await;
    let contracts_before =
        collection_contract_snapshot(&database, &[SESSIONS, COORDINATION_RECEIPTS]).await;
    let sessions_before = sorted_collection_documents(&database, SESSIONS).await;
    let receipts_before = sorted_collection_documents(&database, COORDINATION_RECEIPTS).await;
    drop(store);

    for coordination_cutover_confirmed in [false, true] {
        let error = match MongoSessionStore::open(
            &client,
            &database_name,
            false,
            coordination_cutover_confirmed,
        )
        .await
        {
            Ok(_) => panic!("a protocol-2 activation cannot be upgraded safely"),
            Err(error) => error,
        };
        let message = error.to_string();
        assert!(message.contains("markerless runtime-switch boundaries"));
        assert!(message.contains("cannot restore that accounting boundary"));
        assert_eq!(
            sorted_collection_documents(&database, NAMESPACE_CUTOVERS).await,
            cutovers_before,
            "startup cannot rewrite legacy activation provenance"
        );
        assert_eq!(
            collection_contract_snapshot(&database, &[SESSIONS, COORDINATION_RECEIPTS]).await,
            contracts_before,
            "startup cannot change validators or protected collection identities"
        );
        assert_eq!(
            sorted_collection_documents(&database, SESSIONS).await,
            sessions_before,
            "startup cannot migrate legacy session documents"
        );
        assert_eq!(
            sorted_collection_documents(&database, COORDINATION_RECEIPTS).await,
            receipts_before,
            "startup cannot rewrite legacy coordination receipts"
        );
    }

    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_unproven_protocol_upgrade_markers_are_rejected_without_mutation() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();

    for stage in [
        "legacy-upgrade-marker",
        "after-marker",
        "after-strict-validator",
        "after-document-migration",
    ] {
        let database_name = format!("cma_up_{}_{}_{}", stage, std::process::id(), nonce);
        let database = client.database(&database_name);
        let store = MongoSessionStore::open(&client, &database_name, false, false)
            .await
            .unwrap();
        let mut record = record_with(None);
        record.session.id = SessionId(format!("sesn_coordination_v3_resume_{stage}"));
        record.session.last_seq = 0;
        store
            .create_session_idempotent(&format!("seed-v3-resume-{stage}"), &record, &[])
            .await
            .unwrap();
        let legacy = downgrade_completed_coordination_protocol_for_test(&store).await;
        let CoordinationActivationMarker::LegacyComplete { collection_ids, .. } = legacy else {
            unreachable!("the downgrade helper returns a completed v2 marker");
        };
        let upgrade_marker = if stage == "legacy-upgrade-marker" {
            CoordinationActivationMarker::LegacyUpgradeInProgress {
                protocol: LEGACY_COORDINATION_ACTIVATION_PROTOCOL,
                origin: "operator-drained".into(),
                collection_ids,
            }
        } else {
            CoordinationActivationMarker::UpgradeInProgress {
                origin: "operator-drained".into(),
                collection_ids,
            }
        };
        database
            .collection::<Document>(NAMESPACE_CUTOVERS)
            .replace_one(
                doc! { "_id": COORDINATION_ACTIVATION_ID },
                coordination_activation_marker_document(&upgrade_marker).unwrap(),
            )
            .await
            .unwrap();
        if matches!(stage, "after-strict-validator" | "after-document-migration") {
            store
                .install_coordination_protocol_validators()
                .await
                .unwrap();
        }
        if stage == "after-document-migration" {
            store.migrate_session_coordination_protocol().await.unwrap();
        }
        let cutovers_before = sorted_collection_documents(&database, NAMESPACE_CUTOVERS).await;
        let contracts_before =
            collection_contract_snapshot(&database, &[SESSIONS, COORDINATION_RECEIPTS]).await;
        let sessions_before = sorted_collection_documents(&database, SESSIONS).await;
        let receipts_before = sorted_collection_documents(&database, COORDINATION_RECEIPTS).await;
        drop(store);

        for coordination_cutover_confirmed in [false, true] {
            let error = match MongoSessionStore::open(
                &client,
                &database_name,
                false,
                coordination_cutover_confirmed,
            )
            .await
            {
                Ok(_) => panic!("unproven upgrade marker {stage} cannot resume safely"),
                Err(error) => error,
            };
            let message = error.to_string();
            if stage == "legacy-upgrade-marker" {
                assert!(message.contains("markerless runtime-switch boundaries"));
                assert!(message.contains("cannot restore that accounting boundary"));
            } else {
                assert!(message.contains("upgrade-in-progress provenance cannot be resumed"));
                assert!(
                    message.contains("does not prove a reconstructable runtime-switch boundary")
                );
            }
            assert_eq!(
                sorted_collection_documents(&database, NAMESPACE_CUTOVERS).await,
                cutovers_before,
                "startup cannot promote or complete {stage} provenance"
            );
            assert_eq!(
                collection_contract_snapshot(&database, &[SESSIONS, COORDINATION_RECEIPTS]).await,
                contracts_before,
                "startup cannot change protected collection contracts for {stage}"
            );
            assert_eq!(
                sorted_collection_documents(&database, SESSIONS).await,
                sessions_before,
                "startup cannot migrate session documents for {stage}"
            );
            assert_eq!(
                sorted_collection_documents(&database, COORDINATION_RECEIPTS).await,
                receipts_before,
                "startup cannot rewrite receipts for {stage}"
            );
        }
        database.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_completed_v3_contract_fails_closed_without_repairing_a_downgrade() {
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
        "chrono_completed_v3_downgrade_{}_{}",
        std::process::id(),
        nonce
    );
    let database = client.database(&database_name);
    let store = MongoSessionStore::open(&client, &database_name, false, false)
        .await
        .unwrap();
    let mut record = record_with(None);
    record.session.id = SessionId("sesn_completed_v3_downgrade".into());
    record.session.last_seq = 0;
    store
        .create_session_idempotent("seed-completed-v3-downgrade", &record, &[])
        .await
        .unwrap();
    let marker_before = database
        .collection::<Document>(NAMESPACE_CUTOVERS)
        .find_one(doc! { "_id": COORDINATION_ACTIVATION_ID })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        marker_before.get_i32("protocol").unwrap(),
        COORDINATION_ACTIVATION_PROTOCOL
    );

    database
        .run_command(doc! {
            "collMod": SESSIONS,
            "validator": legacy_session_protocol_validator(),
            "validationLevel": "strict",
            "validationAction": "error",
        })
        .await
        .unwrap();
    store
        .sessions
        .clone_with_type::<Document>()
        .update_one(
            doc! { "_id": &record.session.id.0 },
            doc! {
                "$set": {
                    WRITE_PROTOCOL_FIELD: WRITE_PROTOCOL,
                    COORDINATION_PROTOCOL_FIELD: LEGACY_SESSION_COORDINATION_PROTOCOL,
                },
            },
        )
        .await
        .unwrap();
    drop(store);

    let contracts_before = collection_contract_snapshot(&database, &[SESSIONS]).await;
    let sessions_before = sorted_collection_documents(&database, SESSIONS).await;
    let error = match MongoSessionStore::open(&client, &database_name, false, false).await {
        Ok(_) => panic!("a completed v3 marker must not authorize downgrade repair"),
        Err(error) => error,
    };
    assert!(error
        .to_string()
        .contains("neither the exact current nor previous write-protocol contract"));
    assert_eq!(
        database
            .collection::<Document>(NAMESPACE_CUTOVERS)
            .find_one(doc! { "_id": COORDINATION_ACTIVATION_ID })
            .await
            .unwrap()
            .unwrap(),
        marker_before
    );
    assert_eq!(
        collection_contract_snapshot(&database, &[SESSIONS]).await,
        contracts_before
    );
    assert_eq!(
        sorted_collection_documents(&database, SESSIONS).await,
        sessions_before
    );

    database
        .run_command(doc! {
            "collMod": SESSIONS,
            "validator": session_protocol_validator(),
            "validationLevel": "strict",
            "validationAction": "error",
        })
        .await
        .unwrap();
    let contracts_before = collection_contract_snapshot(&database, &[SESSIONS]).await;
    let sessions_before = sorted_collection_documents(&database, SESSIONS).await;
    assert_eq!(
        sessions_before[0]
            .get_i32(COORDINATION_PROTOCOL_FIELD)
            .unwrap(),
        LEGACY_SESSION_COORDINATION_PROTOCOL
    );
    let error = match MongoSessionStore::open(&client, &database_name, false, false).await {
        Ok(_) => panic!("a completed v3 marker must not migrate a stale protocol-v2 document"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("coordination protocol"));
    assert_eq!(
        database
            .collection::<Document>(NAMESPACE_CUTOVERS)
            .find_one(doc! { "_id": COORDINATION_ACTIVATION_ID })
            .await
            .unwrap()
            .unwrap(),
        marker_before
    );
    assert_eq!(
        collection_contract_snapshot(&database, &[SESSIONS]).await,
        contracts_before
    );
    assert_eq!(
        sorted_collection_documents(&database, SESSIONS).await,
        sessions_before
    );

    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_completed_v3_rejects_legacy_receipt_state_without_mutation() {
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
        "chrono_completed_v3_legacy_receipt_{}_{}",
        std::process::id(),
        nonce
    );
    let database = client.database(&database_name);
    let store = MongoSessionStore::open(&client, &database_name, false, false)
        .await
        .unwrap();
    assert!(matches!(
        store.coordination_activation_marker().await.unwrap(),
        Some(CoordinationActivationMarker::Complete { .. })
    ));
    database
        .run_command(doc! {
            "collMod": COORDINATION_RECEIPTS,
            "validator": legacy_coordination_receipt_protocol_validator(),
            "validationLevel": "strict",
            "validationAction": "error",
        })
        .await
        .unwrap();
    store
        .receipts
        .clone_with_type::<Document>()
        .insert_one(doc! {
            "_id": {
                "session_id": "sesn_legacy_receipt_residue",
                "kind": "lease",
                "token": "legacy-receipt-residue",
            },
            WRITE_PROTOCOL_FIELD: WRITE_PROTOCOL,
            COORDINATION_PROTOCOL_FIELD: LEGACY_COORDINATION_RECEIPT_PROTOCOL,
            "payload": {
                "type": "lease_token",
                "value": {},
            },
        })
        .await
        .unwrap();
    drop(store);

    let cutovers_before = sorted_collection_documents(&database, NAMESPACE_CUTOVERS).await;
    let contracts_before =
        collection_contract_snapshot(&database, &[SESSIONS, COORDINATION_RECEIPTS]).await;
    let receipts_before = sorted_collection_documents(&database, COORDINATION_RECEIPTS).await;
    for coordination_cutover_confirmed in [false, true] {
        let error = match MongoSessionStore::open(
            &client,
            &database_name,
            false,
            coordination_cutover_confirmed,
        )
        .await
        {
            Ok(_) => panic!("a completed marker cannot authorize a legacy receipt validator"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains("changed write-protocol validator"));
        assert_eq!(
            sorted_collection_documents(&database, NAMESPACE_CUTOVERS).await,
            cutovers_before
        );
        assert_eq!(
            collection_contract_snapshot(&database, &[SESSIONS, COORDINATION_RECEIPTS]).await,
            contracts_before
        );
        assert_eq!(
            sorted_collection_documents(&database, COORDINATION_RECEIPTS).await,
            receipts_before
        );
    }

    database
        .run_command(doc! {
            "collMod": COORDINATION_RECEIPTS,
            "validator": coordination_receipt_protocol_validator(),
            "validationLevel": "strict",
            "validationAction": "error",
        })
        .await
        .unwrap();
    let contracts_before =
        collection_contract_snapshot(&database, &[SESSIONS, COORDINATION_RECEIPTS]).await;
    for coordination_cutover_confirmed in [false, true] {
        let error = match MongoSessionStore::open(
            &client,
            &database_name,
            false,
            coordination_cutover_confirmed,
        )
        .await
        {
            Ok(_) => panic!("the startup audit cannot accept a legacy receipt document"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains("identity or protocol is invalid"));
        assert_eq!(
            sorted_collection_documents(&database, NAMESPACE_CUTOVERS).await,
            cutovers_before
        );
        assert_eq!(
            collection_contract_snapshot(&database, &[SESSIONS, COORDINATION_RECEIPTS]).await,
            contracts_before
        );
        assert_eq!(
            sorted_collection_documents(&database, COORDINATION_RECEIPTS).await,
            receipts_before
        );
    }

    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_unproven_upgrade_marker_rejects_a_replacement_without_mutation() {
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
        "chrono_v3_upgrade_replacement_{}_{}",
        std::process::id(),
        nonce
    );
    let database = client.database(&database_name);
    let store = MongoSessionStore::open(&client, &database_name, false, false)
        .await
        .unwrap();
    let marker = store
        .coordination_activation_marker()
        .await
        .unwrap()
        .unwrap();
    let CoordinationActivationMarker::Complete { collection_ids, .. } = marker else {
        unreachable!("a fresh store has a completed current marker");
    };
    let upgrade = CoordinationActivationMarker::UpgradeInProgress {
        origin: "operator-drained".into(),
        collection_ids,
    };
    database
        .collection::<Document>(NAMESPACE_CUTOVERS)
        .replace_one(
            doc! { "_id": COORDINATION_ACTIVATION_ID },
            coordination_activation_marker_document(&upgrade).unwrap(),
        )
        .await
        .unwrap();
    store.sessions.drop().await.unwrap();
    database.create_collection(SESSIONS).await.unwrap();
    database
        .run_command(doc! {
            "collMod": SESSIONS,
            "validator": legacy_session_protocol_validator(),
            "validationLevel": "strict",
            "validationAction": "error",
        })
        .await
        .unwrap();
    let mut replacement = record_with(None);
    replacement.session.id = SessionId("sesn_replacement_upgrade_source".into());
    replacement.session.last_seq = 0;
    let mut replacement_document =
        mongodb::bson::serialize_to_document(&to_doc(&replacement).unwrap()).unwrap();
    replacement_document.insert(WRITE_PROTOCOL_FIELD, WRITE_PROTOCOL);
    replacement_document.insert(
        COORDINATION_PROTOCOL_FIELD,
        LEGACY_SESSION_COORDINATION_PROTOCOL,
    );
    database
        .collection::<Document>(SESSIONS)
        .insert_one(replacement_document)
        .await
        .unwrap();
    drop(store);

    let cutovers_before = sorted_collection_documents(&database, NAMESPACE_CUTOVERS).await;
    let contracts_before =
        collection_contract_snapshot(&database, &[SESSIONS, COORDINATION_RECEIPTS]).await;
    let sessions_before = sorted_collection_documents(&database, SESSIONS).await;
    let receipts_before = sorted_collection_documents(&database, COORDINATION_RECEIPTS).await;
    for coordination_cutover_confirmed in [false, true] {
        let error = match MongoSessionStore::open(
            &client,
            &database_name,
            false,
            coordination_cutover_confirmed,
        )
        .await
        {
            Ok(_) => panic!("an unproven upgrade marker cannot authorize any collection"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains("upgrade-in-progress provenance cannot be resumed"));
        assert_eq!(
            sorted_collection_documents(&database, NAMESPACE_CUTOVERS).await,
            cutovers_before
        );
        assert_eq!(
            collection_contract_snapshot(&database, &[SESSIONS, COORDINATION_RECEIPTS]).await,
            contracts_before
        );
        assert_eq!(
            sorted_collection_documents(&database, SESSIONS).await,
            sessions_before
        );
        assert_eq!(
            sorted_collection_documents(&database, COORDINATION_RECEIPTS).await,
            receipts_before
        );
    }

    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_fresh_database_activates_coordination_without_a_drain_gate() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database_name = format!("chrono_fresh_coordination_{}_{}", std::process::id(), nonce);
    let database = client.database(&database_name);

    let store = MongoSessionStore::open(&client, &database_name, false, false)
        .await
        .expect("a genuinely empty database activates automatically");
    store.verify_coordinated_namespaces().await.unwrap();
    assert!(matches!(
        store.coordination_activation_marker().await.unwrap(),
        Some(CoordinationActivationMarker::Complete { origin, .. })
            if origin == "new-database"
    ));

    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_missing_activation_marker_cannot_reauthorize_a_coordinated_namespace() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database_name = format!("cma_missing_activation_{}_{}", std::process::id(), nonce);
    let database = client.database(&database_name);
    let store = MongoSessionStore::open(&client, &database_name, false, false)
        .await
        .unwrap();
    let mut record = record_with(None);
    record.session.id = SessionId("sesn_missing_coordination_activation".into());
    record.session.last_seq = 0;
    store
        .create_session_idempotent("seed-missing-coordination-activation", &record, &[])
        .await
        .unwrap();
    database
        .collection::<Document>(NAMESPACE_CUTOVERS)
        .delete_one(doc! { "_id": COORDINATION_ACTIVATION_ID })
        .await
        .unwrap();
    drop(store);

    let contracts_before = collection_contract_snapshot(
        &database,
        &[
            LEGACY_SESSIONS,
            PRE_COORDINATION_SESSIONS,
            SESSIONS,
            LEGACY_EVENTS,
            EVENTS,
            WRITES,
            TOMBSTONES,
            COORDINATION_RECEIPTS,
        ],
    )
    .await;
    let cutovers_before = sorted_collection_documents(&database, NAMESPACE_CUTOVERS).await;
    let sessions_before = sorted_collection_documents(&database, SESSIONS).await;
    let events_before = sorted_collection_documents(&database, EVENTS).await;
    let writes_before = sorted_collection_documents(&database, WRITES).await;
    let tombstones_before = sorted_collection_documents(&database, TOMBSTONES).await;
    let receipts_before = sorted_collection_documents(&database, COORDINATION_RECEIPTS).await;

    for coordination_cutover_confirmed in [false, true] {
        let error = match MongoSessionStore::open(
            &client,
            &database_name,
            false,
            coordination_cutover_confirmed,
        )
        .await
        {
            Ok(_) => panic!("lost activation proof cannot reauthorize coordinated state"),
            Err(error) => error,
        };
        let message = error.to_string();
        assert!(
            message.contains("completed MongoDB namespace cutover is no longer complete"),
            "unexpected refusal: {message}"
        );
        assert!(message.contains("refusing to recreate missing namespaces"));
        assert_eq!(
            collection_contract_snapshot(
                &database,
                &[
                    LEGACY_SESSIONS,
                    PRE_COORDINATION_SESSIONS,
                    SESSIONS,
                    LEGACY_EVENTS,
                    EVENTS,
                    WRITES,
                    TOMBSTONES,
                    COORDINATION_RECEIPTS,
                ],
            )
            .await,
            contracts_before
        );
        assert_eq!(
            sorted_collection_documents(&database, NAMESPACE_CUTOVERS).await,
            cutovers_before
        );
        assert_eq!(
            sorted_collection_documents(&database, SESSIONS).await,
            sessions_before
        );
        assert_eq!(
            sorted_collection_documents(&database, EVENTS).await,
            events_before
        );
        assert_eq!(
            sorted_collection_documents(&database, WRITES).await,
            writes_before
        );
        assert_eq!(
            sorted_collection_documents(&database, TOMBSTONES).await,
            tombstones_before
        );
        assert_eq!(
            sorted_collection_documents(&database, COORDINATION_RECEIPTS).await,
            receipts_before
        );
    }

    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_in_progress_coordination_activation_resumes_every_namespace_boundary() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();

    for stage in ["before-fence", "after-fence", "after-rename", "after-view"] {
        let database_name = format!(
            "cma_coord_resume_{}_{}_{}",
            stage,
            std::process::id(),
            nonce
        );
        let database = client.database(&database_name);
        let old_ids = create_pre_coordination_namespace_state(&database).await;
        let activation = CoordinationActivationMarker::InProgress {
            origin: "operator-drained".into(),
        };
        database
            .collection::<Document>(NAMESPACE_CUTOVERS)
            .insert_one(coordination_activation_marker_document(&activation).unwrap())
            .await
            .unwrap();

        if stage != "before-fence" {
            database
                .run_command(doc! {
                    "collMod": PRE_COORDINATION_SESSIONS,
                    "validator": session_protocol_validator(),
                    "validationLevel": "strict",
                    "validationAction": "error",
                })
                .await
                .unwrap();
        }
        if matches!(stage, "after-rename" | "after-view") {
            client
                .database("admin")
                .run_command(doc! {
                    "renameCollection": format!("{database_name}.{PRE_COORDINATION_SESSIONS}"),
                    "to": format!("{database_name}.{SESSIONS}"),
                    "dropTarget": false,
                })
                .await
                .unwrap();
        }
        if stage == "after-view" {
            database
                .create_collection(PRE_COORDINATION_SESSIONS)
                .view_on(SESSIONS)
                .pipeline(Vec::<Document>::new())
                .await
                .unwrap();
        }

        let store = MongoSessionStore::open(&client, &database_name, false, false)
            .await
            .unwrap_or_else(|error| panic!("resume {stage}: {error}"));
        store.verify_coordinated_namespaces().await.unwrap();
        assert_eq!(
            store.collection_ids[SESSIONS],
            old_ids[PRE_COORDINATION_SESSIONS]
        );
        assert!(matches!(
            store.coordination_activation_marker().await.unwrap(),
            Some(CoordinationActivationMarker::Complete { .. })
        ));
        database.drop().await.unwrap();
    }

    let database_name = format!(
        "chrono_coordination_before_complete_{}_{}",
        std::process::id(),
        nonce
    );
    let database = client.database(&database_name);
    let first = MongoSessionStore::open(&client, &database_name, false, false)
        .await
        .unwrap();
    let session_id = first.collection_ids[SESSIONS].clone();
    database
        .collection::<Document>(NAMESPACE_CUTOVERS)
        .replace_one(
            doc! { "_id": COORDINATION_ACTIVATION_ID },
            coordination_activation_marker_document(&CoordinationActivationMarker::InProgress {
                origin: "new-database".into(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    drop(first);

    let resumed = MongoSessionStore::open(&client, &database_name, false, false)
        .await
        .expect("complete contracts with an in-progress marker resume to completion");
    assert_eq!(resumed.collection_ids[SESSIONS], session_id);
    assert!(matches!(
        resumed.coordination_activation_marker().await.unwrap(),
        Some(CoordinationActivationMarker::Complete { .. })
    ));
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_marker_backed_partial_legacy_bootstrap_coordinates_after_the_drain_gate() {
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
        .insert_one(
            namespace_cutover_marker_document(&NamespaceCutoverMarker::InProgress {
                origin: "new-database".into(),
                source_ids: test_namespace_cutover_source_ids(None, None, None),
                tombstone_id: None,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    database.create_collection(EVENTS).await.unwrap();

    let store = MongoSessionStore::open(&client, &database_name, false, true)
        .await
        .expect("durable new-database provenance resumes the partial bootstrap");
    store.verify_coordinated_namespaces().await.unwrap();
    MongoSessionStore::open(&client, &database_name, false, false)
        .await
        .expect("completed views remain restartable without an opt-in");
    assert!(matches!(
        store.namespace_cutover_marker().await.unwrap(),
        Some(NamespaceCutoverMarker::Complete { .. })
    ));
    assert!(matches!(
        store.coordination_activation_marker().await.unwrap(),
        Some(CoordinationActivationMarker::Complete { .. })
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

    let mut record = record_with(None);
    record.session.id = id.clone();
    record.session.last_seq = 1;
    let mut legacy_session =
        mongodb::bson::serialize_to_document(&to_doc(&record).unwrap()).unwrap();
    legacy_session.remove("revision");
    legacy_session.remove(WRITE_PROTOCOL_FIELD);
    database
        .collection::<Document>(PRE_COORDINATION_SESSIONS)
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
    for authoritative in [PRE_COORDINATION_SESSIONS, EVENTS] {
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
    let source_contracts =
        collection_contract_snapshot(&database, &[PRE_COORDINATION_SESSIONS, EVENTS]).await;
    database
        .collection::<Document>(NAMESPACE_CUTOVERS)
        .insert_one(
            namespace_cutover_marker_document(&NamespaceCutoverMarker::InProgress {
                origin: "operator-drained".into(),
                source_ids: BTreeMap::from([
                    (
                        LEGACY_SESSIONS.to_owned(),
                        source_contracts[PRE_COORDINATION_SESSIONS].uuid.clone(),
                    ),
                    (
                        LEGACY_EVENTS.to_owned(),
                        source_contracts[EVENTS].uuid.clone(),
                    ),
                    (TOMBSTONES.to_owned(), None),
                ]),
                tombstone_id: None,
            })
            .unwrap(),
        )
        .await
        .unwrap();

    let store = MongoSessionStore::open(&client, &database_name, false, true)
        .await
        .expect("cutover resumes after the session rename without inferring a delete");
    assert!(store.get(&id).await.unwrap().is_some());
    assert_eq!(store.event_tail(&id).await.unwrap(), 1);
    assert!(!store.is_tombstoned(&id).await.unwrap());
    store.verify_coordinated_namespaces().await.unwrap();
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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_legacy_stage_reconciles_an_in_flight_delete_without_coordination_state() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let database_name = temporary_database_name("chrono_stage_legacy_delete_resume");
    let database = client.database(&database_name);
    let id = SessionId("sesn_stage_legacy_delete".into());

    let mut record = record_with(None);
    record.session.id = id.clone();
    record.session.last_seq = 1;
    database
        .collection::<Document>(LEGACY_SESSIONS)
        .insert_one(raw_legacy_session_document(&record))
        .await
        .unwrap();
    database
        .collection::<Document>(LEGACY_EVENTS)
        .insert_one(raw_legacy_event_document(&event(&id.0, 1)))
        .await
        .unwrap();
    database
        .collection::<SessionWriteDoc>(WRITES)
        .insert_many([
            SessionWriteDoc {
                id: "write_stage_legacy_delete".into(),
                write_protocol: WRITE_PROTOCOL,
                session_id: id.0.clone(),
                revision: 1,
            },
            SessionWriteDoc {
                id: "write_stage_unrelated".into(),
                write_protocol: WRITE_PROTOCOL,
                session_id: "sesn_stage_unrelated".into(),
                revision: 1,
            },
        ])
        .await
        .unwrap();

    let stage = MongoSessionStore::new(&client, &database_name);
    stage
        .validate_transaction_topology(STORE_OPEN_TIMEOUT)
        .await
        .unwrap();
    let qualification = stage.qualify_transaction(STORE_OPEN_TIMEOUT).await.unwrap();
    stage
        .record_namespace_cutover_marker("operator-drained")
        .await
        .unwrap();
    let cutover = stage.authorize_namespace_cutover(true).await.unwrap();
    stage.prepare_cutover_tombstone_collection().await.unwrap();
    stage
        .prepare_legacy_session_fence(&qualification, &cutover)
        .await
        .unwrap();

    database
        .collection::<Document>(LEGACY_SESSIONS)
        .delete_one(doc! { "_id": &id.0 })
        .await
        .expect("a legacy delete already released before the fence can remove its owner");
    drop(stage);

    MongoSessionStore::stage_legacy_cutover(&client, &database_name)
        .await
        .expect("the staged resume finishes only the legacy delete's second half");
    let tombstone = database
        .collection::<Document>(TOMBSTONES)
        .find_one(doc! { "_id": &id.0 })
        .await
        .unwrap()
        .expect("the resumed legacy delete leaves permanent deletion state");
    assert_eq!(
        tombstone.get_i32(WRITE_PROTOCOL_FIELD).unwrap(),
        WRITE_PROTOCOL
    );
    assert_eq!(
        database
            .collection::<Document>(PRE_COORDINATION_SESSIONS)
            .count_documents(doc! { "_id": &id.0 })
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        database
            .collection::<Document>(EVENTS)
            .count_documents(doc! { "session_id": &id.0 })
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        database
            .collection::<Document>(WRITES)
            .count_documents(doc! { "session_id": &id.0 })
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        database
            .collection::<Document>(WRITES)
            .count_documents(doc! { "_id": "write_stage_unrelated" })
            .await
            .unwrap(),
        1
    );

    let names = database.list_collection_names().await.unwrap();
    for absent in [SESSIONS, COORDINATION_RECEIPTS] {
        assert!(
            !names.iter().any(|name| name == absent),
            "the legacy stage created coordination namespace {absent}"
        );
    }
    for absent_marker in [
        COORDINATION_ACTIVATION_ID,
        COORDINATION_RECEIPTS_PROVENANCE_ID,
    ] {
        assert!(database
            .collection::<Document>(NAMESPACE_CUTOVERS)
            .find_one(doc! { "_id": absent_marker })
            .await
            .unwrap()
            .is_none());
    }
    assert!(matches!(
        MongoSessionStore::new(&client, &database_name)
            .namespace_cutover_marker()
            .await
            .unwrap(),
        Some(NamespaceCutoverMarker::Complete { .. })
    ));

    database.drop().await.unwrap();
}

async fn drop_session_namespace_pairs(database: &Database) {
    for collection in [
        LEGACY_SESSIONS,
        PRE_COORDINATION_SESSIONS,
        LEGACY_EVENTS,
        SESSIONS,
        EVENTS,
    ] {
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
    database
        .create_collection(PRE_COORDINATION_SESSIONS)
        .view_on(SESSIONS)
        .pipeline(Vec::<Document>::new())
        .await
        .unwrap();
    database
        .create_collection(LEGACY_SESSIONS)
        .view_on(PRE_COORDINATION_SESSIONS)
        .pipeline(Vec::<Document>::new())
        .await
        .unwrap();
    database
        .create_collection(LEGACY_EVENTS)
        .view_on(EVENTS)
        .pipeline(Vec::<Document>::new())
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_completed_activation_provenance_rejects_session_loss_and_replacement() {
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

    let store = MongoSessionStore::open(&client, &database_name, false, false)
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
    let activation_before = database
        .collection::<Document>(NAMESPACE_CUTOVERS)
        .find_one(doc! { "_id": COORDINATION_ACTIVATION_ID })
        .await
        .unwrap()
        .expect("completed activation provenance is durable");
    assert!(matches!(
        validate_coordination_activation_marker(activation_before.clone()).unwrap(),
        CoordinationActivationMarker::Complete { .. }
    ));
    drop(store);

    drop_session_namespace_pairs(&database).await;
    create_fenced_session_namespace_pairs(&database).await;
    database
        .run_command(doc! {
            "collMod": SESSIONS,
            "validator": previous_session_protocol_validator(),
            "validationLevel": "strict",
            "validationAction": "error",
        })
        .await
        .unwrap();
    let replacement_contract_before =
        collection_contract_snapshot(&database, &[SESSIONS, EVENTS]).await;
    let replacement_error =
        match MongoSessionStore::open(&client, &database_name, false, true).await {
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
    assert_eq!(
        database
            .collection::<Document>(NAMESPACE_CUTOVERS)
            .find_one(doc! { "_id": COORDINATION_ACTIVATION_ID })
            .await
            .unwrap()
            .unwrap(),
        activation_before
    );
    assert_eq!(
        collection_contract_snapshot(&database, &[SESSIONS, EVENTS]).await,
        replacement_contract_before,
        "UUID provenance must be verified before a drained startup can replace the predecessor validator"
    );

    drop_session_namespace_pairs(&database).await;
    let mut names_before = database.list_collection_names().await.unwrap();
    names_before.sort();
    let loss_error = match MongoSessionStore::open(&client, &database_name, false, false).await {
        Ok(_) => panic!("lost completed namespaces must not become a fresh empty store"),
        Err(error) => error,
    };
    assert!(loss_error.to_string().contains("is missing"));
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
    assert_eq!(
        database
            .collection::<Document>(NAMESPACE_CUTOVERS)
            .find_one(doc! { "_id": COORDINATION_ACTIVATION_ID })
            .await
            .unwrap()
            .unwrap(),
        activation_before
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
    let store = MongoSessionStore::open(&client, &database, false, false)
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
    let store = MongoSessionStore::open(&client, &database, true, true)
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
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_real_legacy_document_can_acquire_its_first_durable_handover_lease() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database = format!(
        "chrono_legacy_coordination_{}_{}",
        std::process::id(),
        nonce
    );

    let mut original = record_with(None);
    original.session.id = SessionId("sesn_legacy_coordination".into());
    original.session.last_seq = 0;
    original.runtime_id = Some("runtime-generation-zero".into());
    let stored = to_doc(&original).expect("record encodes");
    let mut legacy = mongodb::bson::serialize_to_document(&stored).expect("record serializes");
    assert!(legacy.remove("revision").is_some());
    assert!(legacy.remove(WRITE_PROTOCOL_FIELD).is_some());
    assert!(legacy.remove(COORDINATION_PROTOCOL_FIELD).is_some());
    assert!(legacy.remove("coordination").is_some());
    client
        .database(&database)
        .collection::<Document>(LEGACY_SESSIONS)
        .insert_one(legacy)
        .await
        .expect("legacy document is seeded before either writer fence");

    let store = MongoSessionStore::open(&client, &database, true, true)
        .await
        .expect("test store opens and fences the legacy namespace");
    let initially_migrated = store
        .sessions
        .clone_with_type::<Document>()
        .find_one(doc! { "_id": &original.session.id.0 })
        .await
        .expect("initially migrated document reads")
        .expect("initially migrated document exists");
    assert_eq!(
        initially_migrated.get_i32(WRITE_PROTOCOL_FIELD).unwrap(),
        SESSION_WRITE_PROTOCOL,
        "initial coordination activation must finish on the durable session writer"
    );
    let record = store
        .get(&original.session.id)
        .await
        .expect("legacy session reads")
        .expect("legacy session exists");
    let authority = store
        .runtime_authority(&original.session.id)
        .await
        .expect("legacy authority reads")
        .expect("legacy session has authority metadata");
    let mut forbidden = record.clone();
    forbidden.runtime_id = Some("runtime-bypassing-coordination".into());
    assert!(matches!(
        store
            .commit_session_idempotent("legacy-runtime-bypass", &forbidden, &[])
            .await,
        Err(StoreError::SessionAuthorityChanged { .. })
    ));
    let request = SessionLeaseRequest {
        session_id: original.session.id.clone(),
        holder: "legacy-coordination-test".into(),
        token: "lease-legacy-first-touch".into(),
        boundary: SessionBoundary::for_record(&record, authority.generation),
        operation: SessionOperation::Handover,
        ttl: Duration::from_secs(30),
    };
    let mut lease = store
        .acquire_session_lease(&request)
        .await
        .expect("the first coordination write promotes every required field");
    assert_eq!(lease.boundary.authority.generation, 0);
    assert_eq!(lease.operation, SessionOperation::Handover);
    assert_eq!(
        lease.boundary.authority.runtime_id.as_deref(),
        Some("runtime-generation-zero")
    );
    assert!(store.legacy_runtime_cleanup_authorized().await.unwrap());

    let promoted = store
        .sessions
        .clone_with_type::<Document>()
        .find_one(doc! { "_id": &original.session.id.0 })
        .await
        .expect("promoted document reads")
        .expect("promoted document exists");
    assert_eq!(promoted.get_i64("revision").unwrap(), 0);
    assert_eq!(
        promoted.get_i32(WRITE_PROTOCOL_FIELD).unwrap(),
        SESSION_WRITE_PROTOCOL
    );
    assert_eq!(
        promoted.get_i32(COORDINATION_PROTOCOL_FIELD).unwrap(),
        SESSION_COORDINATION_PROTOCOL
    );
    assert!(promoted
        .get_document("coordination")
        .unwrap()
        .get_datetime("server_now")
        .is_ok());

    let reservation = store
        .reserve_runtime_generation(&lease, "reserve-generation-one")
        .await
        .expect("the first coordinated generation reserves");
    let candidate = store
        .register_runtime_candidate(&lease, &reservation, "runtime-generation-one")
        .await
        .expect("the successor candidate registers");
    let candidate_debts = store
        .runtime_cleanup_debts(&original.session.id)
        .await
        .unwrap();
    assert_eq!(candidate_debts.len(), 1);
    assert_eq!(
        candidate_debts[0].reason,
        RuntimeCleanupReason::UncommittedCandidate
    );
    store
        .switch_runtime_authority(&mut lease, &candidate)
        .await
        .expect("the successor becomes authoritative");
    let debts = store
        .runtime_cleanup_debts(&original.session.id)
        .await
        .unwrap();
    assert_eq!(debts.len(), 1);
    assert_eq!(debts[0].generation, 0);
    assert_eq!(debts[0].runtime_id, "runtime-generation-zero");
    assert_eq!(debts[0].reason, RuntimeCleanupReason::RetiredAuthority);
    store
        .release_session_lease(&lease)
        .await
        .expect("the promoted handover lease releases");
    let current = store
        .get(&original.session.id)
        .await
        .expect("switched session reads")
        .expect("switched session remains present");
    let authority = store
        .runtime_authority(&original.session.id)
        .await
        .expect("switched authority reads")
        .expect("switched session has authority metadata");
    let cleanup_lease = store
        .acquire_session_lease(&SessionLeaseRequest {
            session_id: original.session.id.clone(),
            holder: "legacy-coordination-cleanup".into(),
            token: "lease-legacy-cleanup".into(),
            boundary: SessionBoundary::for_record(&current, authority.generation),
            operation: SessionOperation::Lifecycle,
            ttl: Duration::from_secs(30),
        })
        .await
        .expect("the exact cleanup lease is acquired");
    store
        .acknowledge_runtime_cleanup(&cleanup_lease, &debts[0])
        .await
        .expect("the exact generation-zero cleanup is acknowledged");
    assert!(store
        .runtime_cleanup_debts(&original.session.id)
        .await
        .unwrap()
        .is_empty());
    store
        .release_session_lease(&cleanup_lease)
        .await
        .expect("the cleanup lease releases");

    client
        .database(&database)
        .drop()
        .await
        .expect("temporary database drops");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_real_store_rejects_runtime_started_from_every_ordinary_write_path() {
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
        "chrono_runtime_marker_fence_{}_{}",
        std::process::id(),
        nonce
    );
    let database = client.database(&database_name);
    let store = MongoSessionStore::open(&client, &database_name, false, false)
        .await
        .unwrap();
    let id = SessionId("sesn_runtime_marker_fence".into());
    let mut original = record_with(None);
    original.session.id = id.clone();
    original.session.last_seq = 0;
    store
        .create_session_idempotent("seed-runtime-marker-fence", &original, &[])
        .await
        .unwrap();
    let durable = store.get(&id).await.unwrap().unwrap();
    let marker = Event {
        session_id: id.clone(),
        seq: 1,
        kind: EventKind::RuntimeStarted {
            generation: std::num::NonZeroU64::new(1).unwrap(),
        },
        appended_at: "2026-08-27T00:00:01Z".into(),
    };
    let mut attempted = durable.clone();
    attempted.session.last_seq = marker.seq;

    let mut create_attempt = record_with(None);
    create_attempt.session.id = SessionId("sesn_created_with_runtime_marker".into());
    create_attempt.session.last_seq = marker.seq;
    let mut create_marker = marker.clone();
    create_marker.session_id = create_attempt.session.id.clone();
    let results = [
        store
            .append_events(std::slice::from_ref(&marker))
            .await
            .map(|_| ()),
        store
            .commit_session_idempotent(
                "session-runtime-marker-fence",
                &attempted,
                std::slice::from_ref(&marker),
            )
            .await
            .map(|_| ()),
        store
            .commit_conversation_idempotent(
                "conversation-runtime-marker-fence",
                &ConversationWriteAuthority::Active(ConversationLink {
                    session_id: id.clone(),
                    epoch: 1,
                    token: "conversation-runtime-marker-fence".into(),
                    runtime_generation: 0,
                    runtime_id: "runtime-marker-fence".into(),
                }),
                &attempted,
                std::slice::from_ref(&marker),
            )
            .await
            .map(|_| ()),
        store
            .create_session_idempotent(
                "create-runtime-marker-fence",
                &create_attempt,
                std::slice::from_ref(&create_marker),
            )
            .await
            .map(|_| ()),
    ];
    for result in results {
        assert!(matches!(
            result,
            Err(StoreError::Malformed(message))
                if message.contains("runtime-authority coordination")
        ));
    }

    let unchanged = store.get(&id).await.unwrap().unwrap();
    assert_eq!(unchanged.revision, durable.revision);
    assert_eq!(unchanged.session.last_seq, 0);
    assert_eq!(store.event_tail(&id).await.unwrap(), 0);
    assert!(store.events_since(&id, 0).await.unwrap().is_empty());
    assert!(store
        .get(&create_attempt.session.id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        sorted_collection_documents(&database, SESSIONS).await.len(),
        1
    );
    assert!(sorted_collection_documents(&database, EVENTS)
        .await
        .is_empty());
    assert_eq!(
        sorted_collection_documents(&database, WRITES).await.len(),
        1,
        "only the seed write owns an idempotency receipt"
    );

    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_skill_cleanup_fence_preserves_the_session_and_events_until_cleaned() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database = format!(
        "chrono_skill_cleanup_fence_{}_{}",
        std::process::id(),
        nonce
    );
    let store = MongoSessionStore::open(&client, &database, false, false)
        .await
        .expect("test store opens");
    let id = SessionId("sesn_skill_cleanup_fence".into());
    let mut seed = record_with(None);
    seed.session.id = id.clone();
    seed.session.last_seq = 1;
    seed.runtime_id = None;
    seed.skill_packs = SkillPackState::CleanupPending;
    store
        .create_session_idempotent("seed-skill-cleanup-fence", &seed, &[event(&id.0, 1)])
        .await
        .expect("session with cleanup debt is seeded");
    let session_before = store
        .sessions
        .clone_with_type::<Document>()
        .find_one(doc! { "_id": &id.0 })
        .await
        .unwrap()
        .unwrap();
    let events_before = sorted_collection_documents(&store.database, EVENTS).await;
    let writes_before = sorted_collection_documents(&store.database, WRITES).await;

    assert!(matches!(
        store.delete(&id).await,
        Err(StoreError::SessionSkillCleanupPending { id: refused }) if refused == id
    ));
    assert_eq!(
        store
            .sessions
            .clone_with_type::<Document>()
            .find_one(doc! { "_id": &id.0 })
            .await
            .unwrap()
            .unwrap(),
        session_before,
        "the refused transaction cannot retain its coordination-clock write"
    );
    assert_eq!(
        sorted_collection_documents(&store.database, EVENTS).await,
        events_before
    );
    assert_eq!(
        sorted_collection_documents(&store.database, WRITES).await,
        writes_before
    );
    assert!(!store.is_tombstoned(&id).await.unwrap());

    let mut cleaned = store.get(&id).await.unwrap().unwrap();
    cleaned.skill_packs = SkillPackState::Cleaned;
    store
        .commit_session_idempotent("finish-skill-cleanup-fence", &cleaned, &[])
        .await
        .expect("cleanup acknowledgement commits");
    store
        .delete(&id)
        .await
        .expect("the exactly cleaned session deletes");
    assert!(store.get(&id).await.unwrap().is_none());
    assert!(store.events_since(&id, 0).await.unwrap().is_empty());
    assert!(store.is_tombstoned(&id).await.unwrap());

    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_real_store_coordinates_exact_cleanup_before_deletion() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database = format!("chrono_exact_coordination_{}_{}", std::process::id(), nonce);
    let store = MongoSessionStore::open(&client, &database, false, false)
        .await
        .expect("test store opens");

    let id = SessionId("sesn_exact_coordination".into());
    let mut seed = record_with(None);
    seed.session.id = id.clone();
    seed.session.last_seq = 1;
    seed.runtime_id = None;
    seed.skill_packs = SkillPackState::Cleaned;
    store
        .create_session_idempotent("seed-exact-coordination", &seed, &[event(&id.0, 1)])
        .await
        .expect("coordination session is seeded");
    let durable = store.get(&id).await.unwrap().unwrap();
    let authority = store.runtime_authority(&id).await.unwrap().unwrap();
    let mut lease = store
        .acquire_session_lease(&SessionLeaseRequest {
            session_id: id.clone(),
            holder: "exact-coordination-test".into(),
            token: "lease-exact-coordination".into(),
            boundary: SessionBoundary::for_record(&durable, authority.generation),
            operation: SessionOperation::Lifecycle,
            ttl: Duration::from_secs(60),
        })
        .await
        .expect("lifecycle lease is acquired");
    assert!(matches!(
        store.delete(&id).await,
        Err(StoreError::SessionLeaseHeld { .. })
    ));

    let reservation = store
        .reserve_runtime_generation(&lease, "reservation-runtime-one")
        .await
        .expect("runtime generation is reserved");
    assert_eq!(
        store
            .reserve_runtime_generation(&lease, "reservation-runtime-one")
            .await
            .expect("reservation retry replays"),
        reservation
    );
    let candidate = store
        .register_runtime_candidate(&lease, &reservation, "runtime-one")
        .await
        .expect("runtime candidate is registered");
    assert_eq!(
        store
            .register_runtime_candidate(&lease, &reservation, "runtime-one")
            .await
            .expect("candidate retry replays"),
        candidate
    );
    let candidate_debts = store.runtime_cleanup_debts(&id).await.unwrap();
    assert_eq!(candidate_debts.len(), 1);
    assert_eq!(candidate_debts[0].generation, reservation.generation.get());
    assert_eq!(candidate_debts[0].runtime_id, "runtime-one");
    assert_eq!(
        candidate_debts[0].reason,
        RuntimeCleanupReason::UncommittedCandidate
    );
    assert_eq!(
        store.events_since(&id, 0).await.unwrap().len(),
        1,
        "registering a candidate does not start a runtime incarnation"
    );

    let mut switch_retry_lease = lease.clone();
    let switched = store
        .switch_runtime_authority(&mut lease, &candidate)
        .await
        .expect("candidate becomes authority atomically");
    assert_eq!(
        store
            .switch_runtime_authority(&mut switch_retry_lease, &candidate)
            .await
            .expect("switch retry replays"),
        switched
    );
    assert_eq!(
        switched.runtime_started_seq,
        seed.session.last_seq + 1,
        "the marker is contiguous with the durable pre-switch sequence floor"
    );
    let durable_after_switch = store.get(&id).await.unwrap().unwrap();
    assert_eq!(
        durable_after_switch.session.last_seq,
        switched.runtime_started_seq
    );
    let events = store.events_since(&id, 0).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].seq, 1);
    let runtime_started = &events[1];
    assert_eq!(runtime_started.seq, switched.runtime_started_seq);
    assert!(layer2_core::seconds_from_rfc3339(&runtime_started.appended_at).is_some());
    assert!(matches!(
        &runtime_started.kind,
        StoredEventKind::Known(EventKind::RuntimeStarted { generation })
            if *generation == reservation.generation
    ));
    assert_eq!(
        store
            .events
            .count_documents(doc! {
                "session_id": &id.0,
                "kind.type": "runtime_started",
            })
            .await
            .unwrap(),
        1,
        "the committed-result retry cannot insert a second marker"
    );
    let reservation_receipt = store
        .receipts
        .clone_with_type::<Document>()
        .find_one(doc! {
            "_id.session_id": &id.0,
            "_id.kind": "reservation",
            "_id.token": "reservation-runtime-one",
        })
        .await
        .unwrap()
        .expect("the completed reservation receipt is durable");
    let receipt_marker_seq = reservation_receipt
        .get_document("payload")
        .unwrap()
        .get_document("value")
        .unwrap()
        .get_document("switch")
        .unwrap()
        .get_i64("runtime_started_seq")
        .unwrap();
    assert_eq!(receipt_marker_seq as u64, switched.runtime_started_seq);
    assert!(store.runtime_cleanup_debts(&id).await.unwrap().is_empty());
    assert_eq!(
        store.runtime_authority(&id).await.unwrap().unwrap(),
        switched.boundary.authority
    );

    let mut retirement_retry_lease = lease.clone();
    let retired = store
        .retire_runtime_authority(&mut lease, "retirement-runtime-one")
        .await
        .expect("authority retires with exact cleanup debt");
    assert_eq!(
        store
            .retire_runtime_authority(&mut retirement_retry_lease, "retirement-runtime-one",)
            .await
            .expect("retirement retry replays"),
        retired
    );
    assert_eq!(
        store.runtime_cleanup_debts(&id).await.unwrap(),
        vec![retired.cleanup.clone()]
    );
    store
        .release_session_lease(&lease)
        .await
        .expect("retirement lease releases");
    assert!(matches!(
        store.delete(&id).await,
        Err(StoreError::SessionCleanupPending {
            runtime_id: None,
            cleanup_debts: 1,
            ..
        })
    ));

    let current = store.get(&id).await.unwrap().unwrap();
    let authority = store.runtime_authority(&id).await.unwrap().unwrap();
    let cleanup_lease = store
        .acquire_session_lease(&SessionLeaseRequest {
            session_id: id.clone(),
            holder: "exact-cleanup-test".into(),
            token: "lease-exact-cleanup".into(),
            boundary: SessionBoundary::for_record(&current, authority.generation),
            operation: SessionOperation::Lifecycle,
            ttl: Duration::from_secs(60),
        })
        .await
        .expect("cleanup lease is acquired");
    store
        .acknowledge_runtime_cleanup(&cleanup_lease, &retired.cleanup)
        .await
        .expect("exact cleanup is acknowledged");
    store
        .release_session_lease(&cleanup_lease)
        .await
        .expect("first cleanup lease releases");

    let current = store.get(&id).await.unwrap().unwrap();
    let authority = store.runtime_authority(&id).await.unwrap().unwrap();
    let successor_cleanup_lease = store
        .acquire_session_lease(&SessionLeaseRequest {
            session_id: id.clone(),
            holder: "successor-cleanup-test".into(),
            token: "lease-successor-cleanup".into(),
            boundary: SessionBoundary::for_record(&current, authority.generation),
            operation: SessionOperation::Lifecycle,
            ttl: Duration::from_secs(60),
        })
        .await
        .expect("successor cleanup lease is acquired");
    store
        .acknowledge_runtime_cleanup(&successor_cleanup_lease, &retired.cleanup)
        .await
        .expect("a successor authorized lease replays the completed acknowledgement");
    assert!(store.runtime_cleanup_debts(&id).await.unwrap().is_empty());
    store
        .release_session_lease(&successor_cleanup_lease)
        .await
        .expect("successor cleanup lease releases");
    store.delete(&id).await.expect("clean session deletes");
    assert_eq!(
        store
            .receipts
            .count_documents(all_session_receipts_filter(&id))
            .await
            .unwrap(),
        0
    );

    client
        .database(&database)
        .drop()
        .await
        .expect("temporary database drops");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_runtime_marker_insert_failure_rolls_back_the_authority_switch() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database = format!("chrono_marker_rollback_{}_{}", std::process::id(), nonce);
    let store = MongoSessionStore::open(&client, &database, false, false)
        .await
        .expect("test store opens");
    let id = SessionId("sesn_marker_rollback".into());
    let mut seed = record_with(None);
    seed.session.id = id.clone();
    seed.session.last_seq = 0;
    seed.runtime_id = None;
    store
        .create_session_idempotent("seed-marker-rollback", &seed, &[])
        .await
        .unwrap();
    let durable = store.get(&id).await.unwrap().unwrap();
    let authority = store.runtime_authority(&id).await.unwrap().unwrap();
    let mut lease = store
        .acquire_session_lease(&SessionLeaseRequest {
            session_id: id.clone(),
            holder: "marker-rollback-test".into(),
            token: "lease-marker-rollback".into(),
            boundary: SessionBoundary::for_record(&durable, authority.generation),
            operation: SessionOperation::Lifecycle,
            ttl: Duration::from_secs(60),
        })
        .await
        .unwrap();
    let reservation = store
        .reserve_runtime_generation(&lease, "reservation-marker-rollback")
        .await
        .unwrap();
    let candidate = store
        .register_runtime_candidate(&lease, &reservation, "runtime-marker-rollback")
        .await
        .unwrap();
    let before_document = store
        .sessions
        .clone_with_type::<Document>()
        .find_one(doc! { "_id": &id.0 })
        .await
        .unwrap()
        .unwrap();
    let before_authority = store.runtime_authority(&id).await.unwrap().unwrap();
    let before_debts = store.runtime_cleanup_debts(&id).await.unwrap();

    let rejecting_validator = doc! {
        "$and": [
            event_protocol_validator(),
            { "kind.type": { "$ne": "runtime_started" } },
        ],
    };
    store
        .database
        .run_command(doc! {
            "collMod": EVENTS,
            "validator": rejecting_validator,
            "validationLevel": "strict",
            "validationAction": "error",
        })
        .await
        .expect("the isolated test collection rejects runtime markers");
    assert!(matches!(
        store.switch_runtime_authority(&mut lease, &candidate).await,
        Err(StoreError::Unavailable(_))
    ));
    assert_eq!(
        store
            .sessions
            .clone_with_type::<Document>()
            .find_one(doc! { "_id": &id.0 })
            .await
            .unwrap()
            .unwrap(),
        before_document
    );
    assert_eq!(
        store.runtime_authority(&id).await.unwrap().unwrap(),
        before_authority
    );
    assert_eq!(
        store.runtime_cleanup_debts(&id).await.unwrap(),
        before_debts
    );
    assert!(store.events_since(&id, 0).await.unwrap().is_empty());
    let reservation_receipt = store
        .receipts
        .clone_with_type::<Document>()
        .find_one(doc! {
            "_id.session_id": &id.0,
            "_id.kind": "reservation",
            "_id.token": "reservation-marker-rollback",
        })
        .await
        .unwrap()
        .unwrap();
    assert!(reservation_receipt
        .get_document("payload")
        .unwrap()
        .get_document("value")
        .unwrap()
        .get("switch")
        .is_none());

    store
        .database
        .run_command(doc! {
            "collMod": EVENTS,
            "validator": event_protocol_validator(),
            "validationLevel": "strict",
            "validationAction": "error",
        })
        .await
        .expect("the ordinary event validator is restored");
    let switched = store
        .switch_runtime_authority(&mut lease, &candidate)
        .await
        .expect("the exact candidate can retry after the atomic failure");
    assert_eq!(switched.runtime_started_seq, 1);
    assert_eq!(store.events_since(&id, 0).await.unwrap().len(), 1);

    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_real_lease_expires_on_the_server_clock_and_allows_a_successor() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database = format!("chrono_lease_expiry_{}_{}", std::process::id(), nonce);
    let store = MongoSessionStore::open(&client, &database, false, false)
        .await
        .expect("test store opens");
    let id = SessionId("sesn_server_clock_expiry".into());
    let mut seed = record_with(None);
    seed.session.id = id.clone();
    seed.session.last_seq = 0;
    store
        .create_session_idempotent("seed-server-clock-expiry", &seed, &[])
        .await
        .unwrap();
    let record = store.get(&id).await.unwrap().unwrap();
    let authority = store.runtime_authority(&id).await.unwrap().unwrap();
    let boundary = SessionBoundary::for_record(&record, authority.generation);
    let first = store
        .acquire_session_lease(&SessionLeaseRequest {
            session_id: id.clone(),
            holder: "first-holder".into(),
            token: "first-token".into(),
            boundary: boundary.clone(),
            operation: SessionOperation::Lifecycle,
            ttl: Duration::from_millis(100),
        })
        .await
        .expect("first lease is acquired");
    assert!(matches!(
        store
            .acquire_session_lease(&SessionLeaseRequest {
                session_id: id.clone(),
                holder: "successor-holder".into(),
                token: "successor-token".into(),
                boundary: boundary.clone(),
                operation: SessionOperation::Lifecycle,
                ttl: Duration::from_secs(30),
            })
            .await,
        Err(StoreError::SessionLeaseHeld { .. })
    ));

    tokio::time::sleep(Duration::from_millis(250)).await;
    let successor = store
        .acquire_session_lease(&SessionLeaseRequest {
            session_id: id.clone(),
            holder: "successor-holder".into(),
            token: "successor-token".into(),
            boundary,
            operation: SessionOperation::Lifecycle,
            ttl: Duration::from_secs(30),
        })
        .await
        .expect("the successor acquires after MongoDB's server time expires the first lease");
    assert_ne!(successor.token, first.token);
    store.release_session_lease(&successor).await.unwrap();
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_ordinary_commits_serialize_with_authority_switch_and_retirement() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database = format!(
        "chrono_commit_coordination_race_{}_{}",
        std::process::id(),
        nonce
    );
    let store = MongoSessionStore::open(&client, &database, false, false)
        .await
        .unwrap();
    let id = SessionId("sesn_commit_coordination_race".into());
    let mut seed = record_with(None);
    seed.session.id = id.clone();
    seed.session.last_seq = 0;
    seed.runtime_id = None;
    store
        .create_session_idempotent("seed-commit-coordination-race", &seed, &[])
        .await
        .unwrap();
    let record = store.get(&id).await.unwrap().unwrap();
    let authority = store.runtime_authority(&id).await.unwrap().unwrap();
    let mut lease = store
        .acquire_session_lease(&SessionLeaseRequest {
            session_id: id.clone(),
            holder: "commit-coordination-race".into(),
            token: "lease-commit-coordination-race".into(),
            boundary: SessionBoundary::for_record(&record, authority.generation),
            operation: SessionOperation::Lifecycle,
            ttl: Duration::from_secs(60),
        })
        .await
        .unwrap();
    let reservation = store
        .reserve_runtime_generation(&lease, "reserve-race-runtime")
        .await
        .unwrap();
    let candidate = store
        .register_runtime_candidate(&lease, &reservation, "race-runtime")
        .await
        .unwrap();

    let mut ordinary_switch_record = record.clone();
    ordinary_switch_record.context_ref = Some("ordinary-commit-near-switch".into());
    let ordinary_switch =
        store.commit_session_idempotent("ordinary-near-switch", &ordinary_switch_record, &[]);
    let authority_switch = store.switch_runtime_authority(&mut lease, &candidate);
    let (ordinary_switch_result, switched) = tokio::join!(ordinary_switch, authority_switch);
    let switched = switched.expect("the authority switch eventually serializes");
    assert!(matches!(
        ordinary_switch_result,
        Ok(_)
            | Err(StoreError::RevisionConflict { .. })
            | Err(StoreError::SessionAuthorityChanged { .. })
    ));
    assert_eq!(
        store.runtime_authority(&id).await.unwrap().unwrap(),
        switched.boundary.authority
    );
    assert!(store.runtime_cleanup_debts(&id).await.unwrap().is_empty());

    let before_retire = store.get(&id).await.unwrap().unwrap();
    let mut ordinary_retire_record = before_retire.clone();
    ordinary_retire_record.context_ref = Some("ordinary-commit-near-retire".into());
    let ordinary_retire =
        store.commit_session_idempotent("ordinary-near-retire", &ordinary_retire_record, &[]);
    let authority_retirement = store.retire_runtime_authority(&mut lease, "retire-race-runtime");
    let (ordinary_retire_result, retired) = tokio::join!(ordinary_retire, authority_retirement);
    let retired = retired.expect("the authority retirement eventually serializes");
    assert!(matches!(
        ordinary_retire_result,
        Ok(_)
            | Err(StoreError::RevisionConflict { .. })
            | Err(StoreError::SessionAuthorityChanged { .. })
    ));
    assert_eq!(
        store.runtime_cleanup_debts(&id).await.unwrap(),
        vec![retired.cleanup]
    );
    assert!(store
        .runtime_authority(&id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .is_none());

    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_chat_runtime_transition_is_atomic_idempotent_and_fences_old_writers() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database = format!(
        "chrono_chat_runtime_transition_{}_{}",
        std::process::id(),
        nonce
    );
    let store = MongoSessionStore::open(&client, &database, false, false)
        .await
        .unwrap();
    let id = SessionId("sesn_real_chat_runtime_transition".into());
    let owner = SubjectId("real-chat-runtime-transition-owner".into());
    let host_id = ChatHostId::for_owner(&owner);
    let mut seed = record_with(None);
    seed.session.id = id.clone();
    seed.session.owner = owner;
    seed.session.last_seq = 0;
    seed.runtime_id = Some("runtime-chat-old".into());
    seed.chat_origin = Some(ChatOrigin {
        host_id: host_id.clone(),
        worktree_name: "chat-1".into(),
        worktree_path: "/workspace/worktrees/chat-1".into(),
        paused_notice: Some("paused".into()),
    });
    store
        .create_session_idempotent("seed-real-chat-runtime-transition", &seed, &[])
        .await
        .unwrap();

    let original = store.get(&id).await.unwrap().unwrap();
    let original_authority = store.runtime_authority(&id).await.unwrap().unwrap();
    let previous = SessionBoundary::for_record(&original, original_authority.generation);
    let lease = store
        .acquire_session_lease(&SessionLeaseRequest {
            session_id: id.clone(),
            holder: "real-chat-transition-holder".into(),
            token: "real-chat-transition-lease".into(),
            boundary: previous.clone(),
            operation: SessionOperation::Lifecycle,
            ttl: Duration::from_secs(60),
        })
        .await
        .unwrap();
    let opening = store
        .acquire_conversation_opening(
            &ConversationOpeningLeaseRequest::new(
                id.clone(),
                lease.holder.clone(),
                "runtime-chat-old",
                original_authority.generation,
                None,
                Duration::from_secs(60),
            ),
            Some(&lease),
        )
        .await
        .unwrap();
    let stale_link = store
        .publish_conversation_link(&opening, Some(&lease))
        .await
        .unwrap();

    let marker = Event {
        session_id: id.clone(),
        seq: 1,
        kind: EventKind::RuntimeIncarnationStarted,
        appended_at: "2026-09-01T00:00:01Z".into(),
    };
    let next = RuntimeAuthority {
        generation: original_authority.generation + 1,
        runtime_id: Some("runtime-chat-new".into()),
    };
    let transition = ChatRuntimeTransition {
        host_id,
        previous,
        next: next.clone(),
    };
    let mut replacement = original.clone();
    replacement.runtime_id.clone_from(&next.runtime_id);
    replacement.session.status = SessionStatus::Provisioning;
    replacement.session.recovery_state = RecoveryState::Recovering;
    replacement.session.last_seq = marker.seq;

    let first_revision = store
        .commit_chat_runtime_transition_idempotent(
            "real-chat-runtime-transition",
            &transition,
            &replacement,
            std::slice::from_ref(&marker),
        )
        .await
        .unwrap();
    let durable = store.get(&id).await.unwrap().unwrap();
    assert_eq!(durable.revision, first_revision);
    assert_eq!(durable.runtime_id, next.runtime_id);
    assert_eq!(durable.session.status, SessionStatus::Provisioning);
    assert_eq!(
        store.runtime_authority(&id).await.unwrap(),
        Some(next.clone())
    );
    assert!(matches!(
        store.validate_conversation_link(&stale_link).await,
        Err(StoreError::ConversationLinkChanged { .. })
    ));
    assert!(matches!(
        store
            .renew_session_lease(&lease, Duration::from_secs(60))
            .await,
        Err(StoreError::SessionLeaseLost { .. })
    ));
    assert!(store.runtime_cleanup_debts(&id).await.unwrap().is_empty());
    assert_eq!(store.events_since(&id, 0).await.unwrap().len(), 1);

    let successor_boundary = SessionBoundary::for_record(&durable, next.generation);
    let successor_lease = store
        .acquire_session_lease(&SessionLeaseRequest {
            session_id: id.clone(),
            holder: "real-chat-successor-holder".into(),
            token: "real-chat-successor-lease".into(),
            boundary: successor_boundary,
            operation: SessionOperation::Lifecycle,
            ttl: Duration::from_secs(60),
        })
        .await
        .unwrap();
    let successor_opening = store
        .acquire_conversation_opening(
            &ConversationOpeningLeaseRequest::new(
                id.clone(),
                successor_lease.holder.clone(),
                next.runtime_id.as_deref().unwrap(),
                next.generation,
                None,
                Duration::from_secs(60),
            ),
            Some(&successor_lease),
        )
        .await
        .unwrap();
    let successor_link = store
        .publish_conversation_link(&successor_opening, Some(&successor_lease))
        .await
        .unwrap();

    let replay_revision = store
        .commit_chat_runtime_transition_idempotent(
            "real-chat-runtime-transition",
            &transition,
            &replacement,
            std::slice::from_ref(&marker),
        )
        .await
        .unwrap();
    assert_eq!(replay_revision, first_revision);
    store
        .validate_conversation_link(&successor_link)
        .await
        .expect("the old transition receipt does not fence its successor");
    assert_eq!(store.events_since(&id, 0).await.unwrap().len(), 1);
    assert!(store.runtime_cleanup_debts(&id).await.unwrap().is_empty());

    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_real_conversation_commit_rejects_stale_authority_without_reserving_the_write() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database = format!(
        "chrono_conversation_commit_authority_{}_{}",
        std::process::id(),
        nonce
    );
    let store = MongoSessionStore::open(&client, &database, false, false)
        .await
        .unwrap();
    let id = SessionId("sesn_conversation_commit_authority".into());
    let mut seed = record_with(None);
    seed.session.id = id.clone();
    seed.session.last_seq = 0;
    seed.runtime_id = Some("runtime-conversation-commit".into());
    store
        .create_session_idempotent("seed-conversation-commit-authority", &seed, &[])
        .await
        .unwrap();

    let opening_request = |token: &str| ConversationOpeningLeaseRequest {
        session_id: id.clone(),
        holder: format!("holder-{token}"),
        token: token.into(),
        runtime_id: "runtime-conversation-commit".into(),
        runtime_generation: 0,
        runtime_candidate_token: None,
        ttl: Duration::from_secs(60),
    };
    let first_opening = store
        .acquire_conversation_opening(&opening_request("first"), None)
        .await
        .unwrap();
    let first_link = store
        .publish_conversation_link(&first_opening, None)
        .await
        .unwrap();
    let second_opening = store
        .acquire_conversation_opening(&opening_request("second"), None)
        .await
        .unwrap();
    let second_link = store
        .publish_conversation_link(&second_opening, None)
        .await
        .unwrap();
    assert!(second_link.epoch > first_link.epoch);

    let before = store.get(&id).await.unwrap().unwrap();
    let before_document = store
        .sessions
        .clone_with_type::<Document>()
        .find_one(doc! { "_id": &id.0 })
        .await
        .unwrap()
        .unwrap();
    let mut attempted = before.clone();
    attempted.session.last_seq = 1;
    let stale_active_write = "stale-active-conversation-write";
    assert!(matches!(
        store
            .commit_conversation_idempotent(
                stale_active_write,
                &ConversationWriteAuthority::Active(first_link),
                &attempted,
                &[event(&id.0, 1)],
            )
            .await,
        Err(StoreError::ConversationLinkChanged { .. })
    ));
    assert_eq!(
        store
            .sessions
            .clone_with_type::<Document>()
            .find_one(doc! { "_id": &id.0 })
            .await
            .unwrap()
            .unwrap(),
        before_document
    );
    assert!(store.events_since(&id, 0).await.unwrap().is_empty());
    assert!(store
        .writes
        .find_one(doc! { "_id": stale_active_write })
        .await
        .unwrap()
        .is_none());
    store
        .commit_conversation_idempotent(
            stale_active_write,
            &ConversationWriteAuthority::Active(second_link),
            &before,
            &[],
        )
        .await
        .expect("the valid successor reuses the unreserved write identity");

    let third_opening = store
        .acquire_conversation_opening(&opening_request("third"), None)
        .await
        .unwrap();
    store
        .publish_conversation_link(&third_opening, None)
        .await
        .unwrap();
    let fourth_opening = store
        .acquire_conversation_opening(&opening_request("fourth"), None)
        .await
        .unwrap();
    let fourth_link = store
        .publish_conversation_link(&fourth_opening, None)
        .await
        .unwrap();

    let before = store.get(&id).await.unwrap().unwrap();
    let before_document = store
        .sessions
        .clone_with_type::<Document>()
        .find_one(doc! { "_id": &id.0 })
        .await
        .unwrap()
        .unwrap();
    let mut attempted = before.clone();
    attempted.session.last_seq = 1;
    let stale_opening_write = "stale-opening-conversation-write";
    assert!(matches!(
        store
            .commit_conversation_idempotent(
                stale_opening_write,
                &ConversationWriteAuthority::Opening {
                    opening: third_opening,
                    session_lease: None,
                },
                &attempted,
                &[event(&id.0, 1)],
            )
            .await,
        Err(StoreError::ConversationOpeningLost { .. })
    ));
    assert_eq!(
        store
            .sessions
            .clone_with_type::<Document>()
            .find_one(doc! { "_id": &id.0 })
            .await
            .unwrap()
            .unwrap(),
        before_document
    );
    assert!(store.events_since(&id, 0).await.unwrap().is_empty());
    assert!(store
        .writes
        .find_one(doc! { "_id": stale_opening_write })
        .await
        .unwrap()
        .is_none());
    store
        .commit_conversation_idempotent(
            stale_opening_write,
            &ConversationWriteAuthority::Active(fourth_link.clone()),
            &before,
            &[],
        )
        .await
        .expect("the current publication reuses the unreserved write identity");

    let current = store.get(&id).await.unwrap().unwrap();
    let authority = store.runtime_authority(&id).await.unwrap().unwrap();
    let mut rotation_lease = store
        .acquire_session_lease(&SessionLeaseRequest {
            session_id: id.clone(),
            holder: "conversation-rotation-test".into(),
            token: "lease-conversation-rotation".into(),
            boundary: SessionBoundary::for_record(&current, authority.generation),
            operation: SessionOperation::Lifecycle,
            ttl: Duration::from_secs(60),
        })
        .await
        .unwrap();
    let reservation = store
        .reserve_runtime_generation(&rotation_lease, "reservation-conversation-rotation")
        .await
        .unwrap();
    let candidate = store
        .register_runtime_candidate(
            &rotation_lease,
            &reservation,
            "runtime-conversation-replacement",
        )
        .await
        .unwrap();
    let switched = store
        .switch_runtime_authority(&mut rotation_lease, &candidate)
        .await
        .unwrap();
    let after_switch = store.get(&id).await.unwrap().unwrap();
    let after_switch_document = store
        .sessions
        .clone_with_type::<Document>()
        .find_one(doc! { "_id": &id.0 })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_switch.session.last_seq, switched.runtime_started_seq);
    let mut attempted = after_switch.clone();
    attempted.session.last_seq = switched.runtime_started_seq + 1;
    let old_runtime_write = "old-runtime-write-after-marker";
    assert!(matches!(
        store
            .commit_conversation_idempotent(
                old_runtime_write,
                &ConversationWriteAuthority::Active(fourth_link),
                &attempted,
                &[event(&id.0, switched.runtime_started_seq + 1)],
            )
            .await,
        Err(StoreError::ConversationLinkChanged { .. })
    ));
    assert_eq!(
        store
            .sessions
            .clone_with_type::<Document>()
            .find_one(doc! { "_id": &id.0 })
            .await
            .unwrap()
            .unwrap(),
        after_switch_document,
        "the old runtime cannot mutate the session after the marker"
    );
    let events = store.events_since(&id, 0).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, switched.runtime_started_seq);
    assert!(matches!(
        &events[0].kind,
        StoredEventKind::Known(EventKind::RuntimeStarted { generation })
            if *generation == reservation.generation
    ));
    assert!(store
        .writes
        .find_one(doc! { "_id": old_runtime_write })
        .await
        .unwrap()
        .is_none());

    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_snapshot_receipt_audits_do_not_report_healthy_concurrent_coordination_as_corrupt() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database = format!(
        "chrono_snapshot_coordination_audit_{}_{}",
        std::process::id(),
        nonce
    );
    let store = MongoSessionStore::open(&client, &database, false, false)
        .await
        .unwrap();
    let id = SessionId("sesn_snapshot_coordination_audit".into());
    let mut seed = record_with(None);
    seed.session.id = id.clone();
    seed.session.last_seq = 0;
    seed.runtime_id = None;
    store
        .create_session_idempotent("seed-snapshot-coordination-audit", &seed, &[])
        .await
        .unwrap();
    let record = store.get(&id).await.unwrap().unwrap();
    let authority = store.runtime_authority(&id).await.unwrap().unwrap();
    let mut lease = store
        .acquire_session_lease(&SessionLeaseRequest {
            session_id: id.clone(),
            holder: "snapshot-audit-writer".into(),
            token: "lease-snapshot-audit".into(),
            boundary: SessionBoundary::for_record(&record, authority.generation),
            operation: SessionOperation::Lifecycle,
            ttl: Duration::from_secs(60),
        })
        .await
        .unwrap();

    let writer_store = store.clone();
    let writer = async move {
        for generation in 1..=8 {
            let reservation = writer_store
                .reserve_runtime_generation(&lease, &format!("snapshot-reservation-{generation}"))
                .await?;
            let candidate = writer_store
                .register_runtime_candidate(
                    &lease,
                    &reservation,
                    &format!("snapshot-runtime-{generation}"),
                )
                .await?;
            let switched = writer_store
                .switch_runtime_authority(&mut lease, &candidate)
                .await?;
            if let Some(cleanup) = switched.cleanup {
                writer_store
                    .acknowledge_runtime_cleanup(&lease, &cleanup)
                    .await?;
            }
        }
        Ok::<_, StoreError>(lease)
    };
    let audit_store = store.clone();
    let auditor = async move {
        for _ in 0..32 {
            audit_store.validate_coordination_receipts().await?;
            tokio::task::yield_now().await;
        }
        Ok::<_, StoreError>(())
    };
    let (writer_result, audit_result) = tokio::join!(writer, auditor);
    let lease = writer_result.expect("healthy coordination completes");
    audit_result.expect("every snapshot audit observes a complete transaction state");
    store.release_session_lease(&lease).await.unwrap();
    store.validate_coordination_receipts().await.unwrap();

    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_real_store_refuses_missing_cleanup_receipts_at_every_deletion_gate() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database = format!(
        "chrono_missing_cleanup_receipt_{}_{}",
        std::process::id(),
        nonce
    );
    let store = MongoSessionStore::open(&client, &database, false, false)
        .await
        .expect("test store opens");
    let id = SessionId("sesn_missing_cleanup_receipt".into());
    let mut seed = record_with(None);
    seed.session.id = id.clone();
    seed.session.last_seq = 0;
    seed.runtime_id = None;
    store
        .create_session_idempotent("seed-missing-cleanup-receipt", &seed, &[])
        .await
        .expect("coordination session is seeded");
    let durable = store.get(&id).await.unwrap().unwrap();
    let authority = store.runtime_authority(&id).await.unwrap().unwrap();
    let lease = store
        .acquire_session_lease(&SessionLeaseRequest {
            session_id: id.clone(),
            holder: "missing-cleanup-receipt-test".into(),
            token: "lease-missing-cleanup-receipt".into(),
            boundary: SessionBoundary::for_record(&durable, authority.generation),
            operation: SessionOperation::Lifecycle,
            ttl: Duration::from_secs(60),
        })
        .await
        .expect("lifecycle lease is acquired");
    let reservation = store
        .reserve_runtime_generation(&lease, "reservation-missing-cleanup")
        .await
        .expect("runtime generation is reserved");
    store
        .register_runtime_candidate(&lease, &reservation, "runtime-missing-cleanup")
        .await
        .expect("runtime candidate is registered with cleanup debt");
    store
        .release_session_lease(&lease)
        .await
        .expect("lifecycle lease releases");

    let deleted = store
        .receipts
        .delete_one(doc! {
            "_id.session_id": &id.0,
            "_id.kind": "cleanup",
        })
        .await
        .expect("test removes the required cleanup receipt");
    assert_eq!(deleted.deleted_count, 1);
    assert!(store.runtime_cleanup_debts(&id).await.unwrap().is_empty());
    assert!(matches!(
        store.delete(&id).await,
        Err(StoreError::Malformed(_))
    ));

    store.integrity.lock().await.checked_at = None;
    assert!(matches!(
        store.check_readiness().await,
        Err(StoreError::Malformed(_))
    ));
    let restart = match MongoSessionStore::open(&client, &database, false, false).await {
        Ok(_) => panic!("startup must reject a missing required cleanup receipt"),
        Err(error) => error,
    };
    assert!(matches!(restart, StoreError::Malformed(_)));

    client
        .database(&database)
        .drop()
        .await
        .expect("temporary database drops");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_real_store_rejects_null_cleanup_completion_without_hiding_the_debt() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database = format!(
        "chrono_null_cleanup_completion_{}_{}",
        std::process::id(),
        nonce
    );
    let store = MongoSessionStore::open(&client, &database, false, false)
        .await
        .expect("test store opens");
    let id = SessionId("sesn_null_cleanup_completion".into());
    let mut seed = record_with(None);
    seed.session.id = id.clone();
    seed.session.last_seq = 0;
    seed.runtime_id = None;
    store
        .create_session_idempotent("seed-null-cleanup-completion", &seed, &[])
        .await
        .expect("coordination session is seeded");
    let durable = store.get(&id).await.unwrap().unwrap();
    let authority = store.runtime_authority(&id).await.unwrap().unwrap();
    let lease = store
        .acquire_session_lease(&SessionLeaseRequest {
            session_id: id.clone(),
            holder: "null-cleanup-completion-test".into(),
            token: "lease-null-cleanup-completion".into(),
            boundary: SessionBoundary::for_record(&durable, authority.generation),
            operation: SessionOperation::Lifecycle,
            ttl: Duration::from_secs(60),
        })
        .await
        .expect("lifecycle lease is acquired");
    let reservation = store
        .reserve_runtime_generation(&lease, "reservation-null-cleanup")
        .await
        .expect("runtime generation is reserved");
    store
        .register_runtime_candidate(&lease, &reservation, "runtime-null-cleanup")
        .await
        .expect("runtime candidate is registered with cleanup debt");
    store
        .release_session_lease(&lease)
        .await
        .expect("lifecycle lease releases");

    let filter = doc! {
        "_id.session_id": &id.0,
        "_id.kind": "cleanup",
    };
    let update = doc! { "$set": { "payload.value.completed_by": Bson::Null } };
    assert!(store
        .receipts
        .clone_with_type::<Document>()
        .update_one(filter.clone(), update.clone())
        .await
        .is_err());
    store
        .receipts
        .clone_with_type::<Document>()
        .update_one(filter, update)
        .bypass_document_validation(true)
        .await
        .expect("the test bypasses validation to model administrative corruption");

    assert_eq!(store.runtime_cleanup_debts(&id).await.unwrap().len(), 1);
    assert!(matches!(
        store.delete(&id).await,
        Err(StoreError::Malformed(_))
    ));
    store.integrity.lock().await.checked_at = None;
    assert!(matches!(
        store.check_readiness().await,
        Err(StoreError::Malformed(_))
    ));

    client
        .database(&database)
        .drop()
        .await
        .expect("temporary database drops");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_real_store_refuses_a_missing_or_replaced_coordination_receipt_collection() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(&uri)
        .await
        .expect("test Mongo connects");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let database = format!("cma_receipt_replace_{}_{}", std::process::id(), nonce);
    let store = MongoSessionStore::open(&client, &database, false, false)
        .await
        .expect("test store opens");
    let original_id = store
        .collection_ids
        .get(COORDINATION_RECEIPTS)
        .expect("receipt UUID is captured")
        .clone();

    store
        .receipts
        .drop()
        .await
        .expect("test removes the proven receipt collection");
    let missing_error = match MongoSessionStore::open(&client, &database, false, false).await {
        Ok(_) => panic!("a completed activation must not recreate a missing receipt collection"),
        Err(error) => error,
    };
    assert!(missing_error.to_string().contains("is missing"));
    assert!(store
        .current_coordination_receipts_id()
        .await
        .unwrap()
        .is_none());

    store
        .database
        .create_collection(COORDINATION_RECEIPTS)
        .await
        .expect("test creates a replacement collection");
    let replacement_id = store
        .current_coordination_receipts_id()
        .await
        .unwrap()
        .expect("replacement UUID reads");
    assert_ne!(replacement_id, original_id);

    let error = match MongoSessionStore::open(&client, &database, false, false).await {
        Ok(_) => panic!("a replacement receipt namespace must not be adopted"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("UUID no longer matches"));

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

    let error = match MongoSessionStore::open(&client, &database, false, false).await {
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

    let store = MongoSessionStore::open(&client, &database, true, true)
        .await
        .expect("cutover succeeds");
    let legacy_sessions = db.collection::<Document>(LEGACY_SESSIONS);
    let pre_coordination_sessions = db.collection::<Document>(PRE_COORDINATION_SESSIONS);
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
    assert!(pre_coordination_sessions
        .insert_one(doc! { "_id": "old-v2-insert" })
        .await
        .is_err());
    assert!(pre_coordination_sessions
        .replace_one(
            doc! { "_id": "sesn_view_fence" },
            doc! { "_id": "sesn_view_fence" }
        )
        .with_options(ReplaceOptions::builder().upsert(true).build())
        .await
        .is_err());
    assert!(pre_coordination_sessions
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
    let store = MongoSessionStore::open(&client, &database, false, false)
        .await
        .expect("test store opens");
    let mut record = record_with(None);
    record.session.id = SessionId("sesn_tombstoned".into());
    record.session.last_seq = 0;
    record.skill_packs = SkillPackState::Cleaned;
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

    let error = match MongoSessionStore::open(&client, &ahead_database, true, true).await {
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

    let store = MongoSessionStore::open(&client, &behind_database, true, true)
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
    partial
        .create_collection(PRE_COORDINATION_SESSIONS)
        .await
        .unwrap();
    let partial_contracts =
        collection_contract_snapshot(&partial, &[PRE_COORDINATION_SESSIONS]).await;
    partial
        .collection::<Document>(NAMESPACE_CUTOVERS)
        .insert_one(
            namespace_cutover_marker_document(&NamespaceCutoverMarker::InProgress {
                origin: "operator-drained".into(),
                source_ids: BTreeMap::from([
                    (
                        LEGACY_SESSIONS.to_owned(),
                        partial_contracts[PRE_COORDINATION_SESSIONS].uuid.clone(),
                    ),
                    (LEGACY_EVENTS.to_owned(), None),
                    (TOMBSTONES.to_owned(), None),
                ]),
                tombstone_id: None,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    let store = MongoSessionStore::open(&client, &partial_name, true, true)
        .await
        .expect("UUID-proven rename-complete/view-missing state resumes");
    MongoSessionStore::open(&client, &partial_name, false, false)
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

    let markerless_name = format!(
        "chrono_cutover_markerless_partial_{}_{}",
        std::process::id(),
        nonce
    );
    let markerless = client.database(&markerless_name);
    markerless
        .create_collection(PRE_COORDINATION_SESSIONS)
        .await
        .unwrap();
    let markerless_before = database_state_snapshot(&markerless).await;
    let markerless_error =
        match MongoSessionStore::open(&client, &markerless_name, true, true).await {
            Ok(_) => panic!("a partial rename without original-source provenance must fail closed"),
            Err(error) => error,
        };
    assert!(markerless_error.to_string().contains(
        "cannot record MongoDB namespace-cutover intent after authoritative namespace sessions_v2 already exists"
    ));
    assert_eq!(
        database_state_snapshot(&markerless).await,
        markerless_before,
        "rejecting a markerless partial rename must not mutate its namespaces or documents"
    );
    markerless.drop().await.unwrap();

    let ambiguous_name = format!("chrono_cutover_ambiguous_{}_{}", std::process::id(), nonce);
    let ambiguous = client.database(&ambiguous_name);
    ambiguous.create_collection(LEGACY_SESSIONS).await.unwrap();
    ambiguous
        .create_collection(PRE_COORDINATION_SESSIONS)
        .await
        .unwrap();
    assert!(
        MongoSessionStore::open(&client, &ambiguous_name, true, true)
            .await
            .is_err()
    );
    ambiguous.drop().await.unwrap();

    let wrong_name = format!("chrono_cutover_wrong_view_{}_{}", std::process::id(), nonce);
    let wrong = client.database(&wrong_name);
    wrong
        .create_collection(PRE_COORDINATION_SESSIONS)
        .await
        .unwrap();
    wrong.create_collection("wrong_sessions").await.unwrap();
    wrong
        .create_collection(LEGACY_SESSIONS)
        .view_on("wrong_sessions")
        .pipeline(Vec::<Document>::new())
        .await
        .unwrap();
    assert!(MongoSessionStore::open(&client, &wrong_name, true, true)
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
    let store = MongoSessionStore::open(&client, &database, false, false)
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
    let store = MongoSessionStore::open(&client, &database, false, false)
        .await
        .expect("test store opens");

    for attempt in 0..16 {
        let id = SessionId(format!("sesn_append_delete_{attempt}"));
        let mut record = record_with(None);
        record.session.id = id.clone();
        record.session.last_seq = 0;
        record.skill_packs = SkillPackState::Cleaned;
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

    MongoSessionStore::open(&client, &database, false, false)
        .await
        .expect("the post-fence audit accepts every race result");
    client.database(&database).drop().await.unwrap();
}
