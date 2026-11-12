use super::*;

fn entry(seq: u64) -> RawDocumentBuf {
    RawDocumentBuf::try_from(
        &log_document(&AgentLogEntry {
            agent_id: AgentId("agt_capacity".into()),
            seq,
            appended_at: "2026-09-05T00:00:00Z".into(),
            kind: layer2_core::sandbox::AgentLogKind::Deleted,
        })
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn normal_and_terminal_agent_log_capacity_are_distinct_and_bounded() {
    assert!(capacity_allows(
        NORMAL_AGENT_LOG_ENTRIES - 1,
        NORMAL_AGENT_LOG_BYTES - 1,
        1,
        1,
        AppendCapacity::Normal,
    ));
    assert!(!capacity_allows(
        NORMAL_AGENT_LOG_ENTRIES,
        NORMAL_AGENT_LOG_BYTES,
        1,
        1,
        AppendCapacity::Normal,
    ));
    assert!(capacity_allows(
        NORMAL_AGENT_LOG_ENTRIES,
        NORMAL_AGENT_LOG_BYTES,
        STOP_RESERVE_ENTRIES,
        STOP_RESERVE_BYTES,
        AppendCapacity::StopOrFailure,
    ));
    assert!(!capacity_allows(
        NORMAL_AGENT_LOG_ENTRIES,
        NORMAL_AGENT_LOG_BYTES,
        STOP_RESERVE_ENTRIES + 1,
        STOP_RESERVE_BYTES,
        AppendCapacity::StopOrFailure,
    ));
    assert!(!capacity_allows(
        NORMAL_AGENT_LOG_ENTRIES,
        NORMAL_AGENT_LOG_BYTES,
        STOP_RESERVE_ENTRIES,
        STOP_RESERVE_BYTES + 1,
        AppendCapacity::StopOrFailure,
    ));
    assert!(capacity_allows(
        NORMAL_AGENT_LOG_ENTRIES,
        NORMAL_AGENT_LOG_BYTES,
        STOP_RESERVE_ENTRIES + DELETE_RESERVE_ENTRIES,
        STOP_RESERVE_BYTES + DELETE_RESERVE_BYTES,
        AppendCapacity::Delete,
    ));
    assert!(!capacity_allows(
        NORMAL_AGENT_LOG_ENTRIES,
        NORMAL_AGENT_LOG_BYTES,
        STOP_RESERVE_ENTRIES + DELETE_RESERVE_ENTRIES,
        STOP_RESERVE_BYTES + DELETE_RESERVE_BYTES + 1,
        AppendCapacity::Delete,
    ));
    assert!(!capacity_allows(
        STOP_AGENT_LOG_ENTRIES,
        STOP_AGENT_LOG_BYTES,
        DELETE_RESERVE_ENTRIES + 1,
        DELETE_RESERVE_BYTES,
        AppendCapacity::Delete,
    ));
}

#[test]
fn append_checks_leave_staged_terminal_headroom() {
    let normal_full = LogSummary {
        entry_count: NORMAL_AGENT_LOG_ENTRIES,
        encoded_bytes: NORMAL_AGENT_LOG_BYTES,
        ..Default::default()
    };
    assert_eq!(
        ensure_append_capacity(&normal_full, &[entry(10_001)], AppendCapacity::Normal),
        Err(SandboxStoreError::AgentLogCapacityExceeded)
    );
    assert!(ensure_append_capacity(
        &normal_full,
        &[entry(10_001)],
        AppendCapacity::StopOrFailure,
    )
    .is_ok());
    assert!(ensure_append_capacity(&normal_full, &[], AppendCapacity::Normal).is_ok());

    let stop_full = LogSummary {
        entry_count: STOP_AGENT_LOG_ENTRIES,
        encoded_bytes: STOP_AGENT_LOG_BYTES,
        ..Default::default()
    };
    assert!(ensure_append_capacity(&stop_full, &[entry(10_065)], AppendCapacity::Delete).is_ok());
    assert_eq!(
        ensure_append_capacity(&stop_full, &[entry(10_065)], AppendCapacity::StopOrFailure,),
        Err(SandboxStoreError::AgentLogCapacityExceeded)
    );
}

#[test]
fn atomic_append_batches_are_bounded_before_materialization() {
    assert!(ensure_append_batch_size(0).is_ok());
    assert!(ensure_append_batch_size(MAX_AGENT_LOG_BATCH_SIZE).is_ok());
    assert_eq!(
        ensure_append_batch_size(MAX_AGENT_LOG_BATCH_SIZE + 1),
        Err(SandboxStoreError::AgentLogCapacityExceeded)
    );
}
