use super::*;
use layer2_core::ids::AgentId;

fn head(count: i64, bytes: i64) -> Head {
    Head {
        agent_id: "agt_rollover_fixture".into(),
        format_version: 1,
        incarnation: "0".repeat(64),
        revision: 4,
        active_ordinal: 0,
        active_start_seq: 1,
        last_seq: count,
        active_count: count,
        active_bytes: bytes,
        previous_seal: None,
        lifetime_count: count,
        lifetime_bytes: bytes,
        projection_format: 1,
        evidence_generation: "initial".into(),
        projection_through: count,
        migration_id: super::super::MIGRATION.into(),
        required_reader: super::super::READER.into(),
    }
}

fn entry(seq: u64, content: String) -> AgentLogEntry {
    AgentLogEntry {
        agent_id: AgentId("agt_rollover_fixture".into()),
        seq,
        appended_at: "2026-11-05T00:00:00Z".into(),
        kind: AgentLogKind::AgentMessage { content },
    }
}

fn publication(
    head: &Head,
    batch: &EncodedBatch,
    class: AppendCapacity,
    permission: RolloverPermission,
    source: SourceFormat,
) -> Result<PublicationPlan, SandboxStoreError> {
    plan(
        head,
        digest::binary(vec![7; head.active_count as usize * 32]),
        batch,
        class,
        permission,
        source,
        "fixture-publication",
    )
}

#[test]
fn normal_count_boundary_rolls_the_whole_batch_without_a_new_event_number() {
    let current = head(9999, 1_000_000);
    let batch = EncodedBatch::new(&current, &[entry(10_000, "x".into())]).unwrap();
    let result = publication(
        &current,
        &batch,
        AppendCapacity::Normal,
        RolloverPermission::Normal,
        SourceFormat::Segmented,
    )
    .unwrap();
    assert!(result.seal.is_none());
    assert_eq!(result.head.active_count, 10_000);
    let current = result.head;
    let batch = EncodedBatch::new(
        &current,
        &[entry(10_001, "a".into()), entry(10_002, "b".into())],
    )
    .unwrap();
    let result = publication(
        &current,
        &batch,
        AppendCapacity::Normal,
        RolloverPermission::Normal,
        SourceFormat::Segmented,
    )
    .unwrap();
    let seal = result.seal.unwrap();
    assert_eq!(
        (seal.start_seq, seal.end_seq, seal.entry_count),
        (1, 10_000, 10_000)
    );
    assert_eq!(seal.leaf_digests.bytes, vec![7; 10_000 * 32]);
    assert_eq!(seal.sealing_operation, "fixture-publication");
    assert_eq!(result.head.previous_seal, Some(seal.seal_digest));
    assert_eq!(
        (
            result.head.active_ordinal,
            result.head.active_start_seq,
            result.head.last_seq
        ),
        (1, 10_001, 10_002)
    );
    assert_eq!(result.head.active_count, 2);
    assert_eq!(result.head.active_bytes, batch.encoded_bytes);
    assert_eq!(
        result.head.lifetime_bytes,
        current.lifetime_bytes + batch.encoded_bytes
    );
    assert_eq!(batch.rows()[0].get_i64("seq").unwrap(), 10_001);
}

#[test]
fn complete_bson_bytes_trigger_an_early_roll_at_the_exact_normal_ceiling() {
    let mut current = head(5, 1000);
    let batch = EncodedBatch::new(&current, &[entry(6, "unicode Ω and quotes \"".into())]).unwrap();
    current.active_bytes = NORMAL_BYTES - batch.encoded_bytes;
    current.lifetime_bytes = current.active_bytes;
    let exact = publication(
        &current,
        &batch,
        AppendCapacity::Normal,
        RolloverPermission::Closed,
        SourceFormat::Segmented,
    )
    .unwrap();
    assert!(exact.seal.is_none());
    assert_eq!(exact.head.active_bytes, NORMAL_BYTES);
    current.active_bytes += 1;
    current.lifetime_bytes += 1;
    let rolled = publication(
        &current,
        &batch,
        AppendCapacity::Normal,
        RolloverPermission::Normal,
        SourceFormat::Segmented,
    )
    .unwrap();
    assert_eq!(rolled.seal.unwrap().entry_count, 5);
    assert_eq!(rolled.head.active_count, 1);
    assert!(matches!(
        publication(
            &current,
            &batch,
            AppendCapacity::Normal,
            RolloverPermission::Closed,
            SourceFormat::Segmented
        ),
        Err(SandboxStoreError::Unavailable(_))
    ));
}

#[test]
fn terminal_classes_keep_their_exact_reserves_without_cleanup_only_rollover() {
    for (class, count, bytes) in [
        (AppendCapacity::StopOrFailure, STOP_COUNT, STOP_BYTES),
        (AppendCapacity::Delete, super::super::MAX_ENTRIES, MAX_BYTES),
    ] {
        let mut current = head(count - 1, 1000);
        let batch =
            EncodedBatch::new(&current, &[entry(count as u64, "terminal fixture".into())]).unwrap();
        current.active_bytes = bytes - batch.encoded_bytes;
        current.lifetime_bytes = current.active_bytes;
        let result = publication(
            &current,
            &batch,
            class,
            RolloverPermission::Closed,
            SourceFormat::Segmented,
        )
        .unwrap();
        assert!(result.seal.is_none());
        assert_eq!(result.head.active_count, count);
        assert_eq!(result.head.active_bytes, bytes);
        current.active_bytes += 1;
        current.lifetime_bytes += 1;
        assert!(matches!(
            publication(
                &current,
                &batch,
                class,
                RolloverPermission::Normal,
                SourceFormat::Segmented
            ),
            Err(SandboxStoreError::AgentLogCapacityExceeded)
        ));
        let full = result.head;
        let batch = EncodedBatch::new(&full, &[entry(count as u64 + 1, "another".into())]).unwrap();
        assert!(publication(
            &full,
            &batch,
            class,
            RolloverPermission::Normal,
            SourceFormat::Segmented
        )
        .is_err());
        let empty = EncodedBatch::new(&full, &[]).unwrap();
        let metadata = publication(
            &full,
            &empty,
            AppendCapacity::Normal,
            RolloverPermission::Closed,
            SourceFormat::Segmented,
        )
        .unwrap();
        assert!(metadata.seal.is_none());
        assert_eq!(metadata.head.last_seq, full.last_seq);
    }
}

#[test]
fn valid_legacy_physical_capacity_can_be_sealed_without_a_spare_event_slot() {
    let current = head(super::super::MAX_ENTRIES, MAX_BYTES);
    let batch = EncodedBatch::new(&current, &[]).unwrap();
    for permission in [
        RolloverPermission::Normal,
        RolloverPermission::LegacyRecovery,
    ] {
        let result = publication(
            &current,
            &batch,
            AppendCapacity::Normal,
            permission,
            SourceFormat::Legacy,
        )
        .unwrap();
        assert_eq!(result.head.revision, 1);
        assert_eq!(result.head.last_seq, current.last_seq);
        assert_eq!(result.head.active_start_seq, current.last_seq + 1);
        assert_eq!(result.head.active_count, 0);
        assert_eq!(result.head.lifetime_bytes, MAX_BYTES);
        assert_eq!(result.seal.unwrap().source_format, "legacy_v1");
    }
    assert!(publication(
        &current,
        &batch,
        AppendCapacity::Normal,
        RolloverPermission::Closed,
        SourceFormat::Legacy
    )
    .is_err());
    let empty_head = head(0, 0);
    let empty = EncodedBatch::new(&empty_head, &[]).unwrap();
    assert!(publication(
        &empty_head,
        &empty,
        AppendCapacity::Normal,
        RolloverPermission::LegacyRecovery,
        SourceFormat::Legacy
    )
    .is_err());
}

#[test]
fn event_and_batch_limits_measure_complete_bson_before_any_rollover() {
    let current = head(100, 100_000);
    let overhead = bson::serialize_to_vec(&log_document(&entry(101, String::new())).unwrap())
        .unwrap()
        .len();
    let exact = entry(101, "x".repeat(DOCUMENT_BYTES - overhead));
    let batch = EncodedBatch::new(&current, &[exact]).unwrap();
    assert_eq!(batch.rows()[0].as_bytes().len(), DOCUMENT_BYTES);
    assert!(matches!(
        EncodedBatch::new(
            &current,
            &[entry(101, "x".repeat(DOCUMENT_BYTES - overhead + 1))]
        ),
        Err(SandboxStoreError::AgentLogCapacityExceeded)
    ));
    let entries: Vec<_> = (101..=164).map(|seq| entry(seq, "x".into())).collect();
    assert_eq!(EncodedBatch::new(&current, &entries).unwrap().count(), 64);
    let mut too_many = entries;
    too_many.push(entry(165, "x".into()));
    assert!(EncodedBatch::new(&current, &too_many).is_err());
    let entries: Vec<_> = (101..106)
        .map(|seq| entry(seq, "x".repeat(13 * 1024 * 1024 - 1024)))
        .collect();
    let batch = EncodedBatch::new(&current, &entries).unwrap();
    assert!(batch.encoded_bytes > NORMAL_BYTES && batch.encoded_bytes < MAX_BYTES);
    assert!(matches!(
        publication(
            &current,
            &batch,
            AppendCapacity::Normal,
            RolloverPermission::Normal,
            SourceFormat::Segmented
        ),
        Err(SandboxStoreError::AgentLogCapacityExceeded)
    ));
}

#[test]
fn lifetime_growth_preserves_global_sequence_but_checks_cursor_and_counter_overflow() {
    let mut current = head(2, 1000);
    current.active_ordinal = 1_000;
    current.active_start_seq = 10_000_001;
    current.last_seq = 10_000_002;
    current.lifetime_count = current.last_seq;
    current.projection_through = current.last_seq;
    current.lifetime_bytes = 1_000 * NORMAL_BYTES + current.active_bytes;
    current.previous_seal = Some(digest::binary(vec![3; 32]));
    let batch =
        EncodedBatch::new(&current, &[entry(current.last_seq as u64 + 1, "x".into())]).unwrap();
    let result = publication(
        &current,
        &batch,
        AppendCapacity::Normal,
        RolloverPermission::Closed,
        SourceFormat::Segmented,
    )
    .unwrap();
    assert!(result.seal.is_none());
    assert_eq!(result.head.last_seq, 10_000_003);
    assert_eq!(result.head.active_ordinal, 1000);
    current.lifetime_bytes = i64::MAX;
    assert!(publication(
        &current,
        &batch,
        AppendCapacity::Normal,
        RolloverPermission::Normal,
        SourceFormat::Segmented
    )
    .is_err());
    current.lifetime_bytes = 1000;
    current.last_seq = MAX_SEQUENCE;
    current.lifetime_count = MAX_SEQUENCE;
    current.projection_through = MAX_SEQUENCE;
    current.active_start_seq = MAX_SEQUENCE - 1;
    assert!(matches!(
        EncodedBatch::new(&current, &[entry(MAX_SEQUENCE as u64 + 1, "x".into())]),
        Err(SandboxStoreError::SequenceConflict)
    ));
}

#[test]
fn stale_batch_and_missing_source_digests_cannot_create_a_successor() {
    let current = head(10_000, 1_000_000);
    let batch = EncodedBatch::new(&current, &[entry(10_001, "x".into())]).unwrap();
    let mut newer = current.clone();
    newer.last_seq += 1;
    newer.lifetime_count += 1;
    newer.projection_through += 1;
    newer.active_count += 1;
    assert!(matches!(
        publication(
            &newer,
            &batch,
            AppendCapacity::Normal,
            RolloverPermission::Normal,
            SourceFormat::Segmented
        ),
        Err(SandboxStoreError::SequenceConflict)
    ));
    assert!(plan(
        &current,
        digest::binary(vec![7; 32]),
        &batch,
        AppendCapacity::Normal,
        RolloverPermission::Normal,
        SourceFormat::Segmented,
        "fixture"
    )
    .is_err());
}

#[test]
fn segmented_recovery_permission_forces_a_storage_only_successor() {
    let current = head(10_000, 1_000_000);
    let batch = EncodedBatch::new(&current, &[]).unwrap();
    let result = publication(
        &current,
        &batch,
        AppendCapacity::Normal,
        RolloverPermission::SegmentedRecovery,
        SourceFormat::Segmented,
    )
    .unwrap();
    let seal = result.seal.expect("recovery must establish a successor");
    assert_eq!(seal.entry_count, current.active_count);
    assert_eq!(result.head.last_seq, current.last_seq);
    assert_eq!(result.head.active_count, 0);
    assert_eq!(result.head.revision, current.revision + 1);
    assert!(publication(
        &current,
        &batch,
        AppendCapacity::Normal,
        RolloverPermission::SegmentedRecovery,
        SourceFormat::Legacy,
    )
    .is_err());
}

#[test]
fn segmented_recovery_refuses_empty_source_event_replay_and_terminal_reserves() {
    for (count, logs, class) in [
        (0, vec![], AppendCapacity::Normal),
        (
            10,
            vec![entry(11, "replayed input".into())],
            AppendCapacity::Normal,
        ),
        (10, vec![], AppendCapacity::StopOrFailure),
        (10, vec![], AppendCapacity::Delete),
    ] {
        let current = head(count, if count == 0 { 0 } else { 1000 });
        let batch = EncodedBatch::new(&current, &logs).unwrap();
        assert!(publication(
            &current,
            &batch,
            class,
            RolloverPermission::SegmentedRecovery,
            SourceFormat::Segmented
        )
        .is_err());
    }
}

#[test]
fn active_proof_rollover_keeps_only_the_new_source_bytes() {
    let current = head(10_000, 1_000_000);
    let batch = EncodedBatch::new(
        &current,
        &[
            entry(10_001, "new one".into()),
            entry(10_002, "new two".into()),
        ],
    )
    .unwrap();
    let result = publication(
        &current,
        &batch,
        AppendCapacity::Normal,
        RolloverPermission::Normal,
        SourceFormat::Segmented,
    )
    .unwrap();
    let prior = digest::binary(vec![7; current.active_count as usize * 32]);
    let proof = super::super::active_proof::ActiveProof::successor(
        &current,
        7,
        8,
        &result,
        &prior,
        batch.rows(),
    )
    .unwrap();
    let document = mongodb::bson::serialize_to_document(&proof).unwrap();
    let leaves = document.get_binary_generic("leaves").unwrap();
    assert_eq!(leaves.len(), 64);
    for (offset, row) in batch.rows().iter().enumerate() {
        let expected = digest::leaf(
            &current.agent_id,
            &current.incarnation,
            current.last_seq + offset as i64 + 1,
            row.as_bytes(),
        );
        assert_eq!(&leaves[offset * 32..(offset + 1) * 32], &expected.bytes);
    }
    assert_eq!(document.get_i64("agent_revision").unwrap(), 8);
    assert_eq!(
        document
            .get_document("head")
            .unwrap()
            .get_i64("active_start_seq")
            .unwrap(),
        10_001
    );
}
