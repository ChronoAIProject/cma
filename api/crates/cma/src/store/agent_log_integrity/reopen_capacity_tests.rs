use super::*;

#[test]
fn reopening_requires_normal_headroom_without_consuming_cleanup_reserves() {
    let (count, bytes) = AppendCapacity::Normal.limits();
    let mut summary = LogSummary {
        entry_count: count - 64,
        encoded_bytes: bytes - 512 * 1024,
        ..Default::default()
    };
    ensure_reopen_capacity(&summary).unwrap();
    summary.entry_count += 1;
    assert!(matches!(
        ensure_reopen_capacity(&summary),
        Err(SandboxStoreError::AgentLogCapacityExceeded)
    ));
    summary.entry_count -= 1;
    summary.encoded_bytes += 1;
    assert!(matches!(
        ensure_reopen_capacity(&summary),
        Err(SandboxStoreError::AgentLogCapacityExceeded)
    ));
    assert!(AppendCapacity::StopOrFailure.limits().0 > count);
}
