use super::*;

#[test]
fn ledger_class_survives_context_and_cleanup_without_becoming_quota_or_loss() {
    for map in [adapter_failure, cleanup_failure] {
        let failure = map(AdapterError::RuntimeLedgerHold("capacity".into()).context("close"));
        assert!(failure.retryable);
        assert_eq!(failure.typed().class, "agent_runtime_ledger_hold");
        assert!(!failure.typed().detail.contains("capacity: close"));
    }
    let quota = adapter_failure(AdapterError::AtCapacity(
        "native HTTP request capacity is exhausted".into(),
    ));
    assert_eq!(quota.typed().class, "agent_resource_quota");
    assert!(!quota.retryable);
    let malformed = adapter_failure(AdapterError::Rejected("malformed record".into()));
    assert_eq!(malformed.typed().class, "agent_runtime_rejected");
    assert!(!malformed.retryable);
    assert_eq!(
        cleanup_failure(AdapterError::Rejected("malformed record".into()))
            .typed()
            .class,
        "agent_cleanup_unavailable"
    );
}
