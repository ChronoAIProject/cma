use super::{
    AgentWorkState, AgentWorkStep, FirstMessageDelivery, FirstMessageFailure, FirstMessageRecord,
    QueuedAgentMessage,
};

const NOW: &str = "2026-09-05T00:00:00Z";
const LATER: &str = "2026-09-05T00:00:01Z";

fn failure(retryable: bool) -> FirstMessageFailure {
    FirstMessageFailure::new(
        "transport_lost",
        "the transport outcome is unknown",
        retryable,
        LATER,
    )
    .unwrap()
}

#[test]
fn first_message_walks_the_positive_evidence_states() {
    let mut record = FirstMessageRecord::new("launch-1", "Prove the theorem", NOW).unwrap();
    assert_eq!(record.state, FirstMessageDelivery::Queued);
    record.fence_dispatch(1, 7, LATER.into()).unwrap();
    assert_eq!(record.state, FirstMessageDelivery::DispatchFenced);
    record.mark_write_confirmed(LATER.into()).unwrap();
    assert_eq!(record.state, FirstMessageDelivery::WriteConfirmed);
    record.mark_execution_observed(LATER.into()).unwrap();
    assert_eq!(record.state, FirstMessageDelivery::ExecutionObserved);
    record.complete(LATER.into()).unwrap();
    assert_eq!(record.state, FirstMessageDelivery::Completed);
    assert!(record.is_terminal());
    assert!(record.validate().is_ok());
}

#[test]
fn ambiguous_dispatch_requires_exact_acknowledged_recovery() {
    let mut record = FirstMessageRecord::new("launch-2", "Inspect the proof", NOW).unwrap();
    record.fence_dispatch(1, 1, LATER.into()).unwrap();
    record.require_recovery(failure(false)).unwrap();
    assert_eq!(record.state, FirstMessageDelivery::RecoveryRequired);

    let digest = record.payload_sha256.clone();
    assert!(record
        .authorize_recovery(&digest, false, LATER.into())
        .is_err());
    assert!(record
        .authorize_recovery(&"0".repeat(64), true, LATER.into())
        .is_err());
    record
        .authorize_recovery(&digest, true, LATER.into())
        .unwrap();
    assert_eq!(record.state, FirstMessageDelivery::Queued);
    assert_eq!(record.recovery_generation, 1);
    assert_eq!(record.attempt, 1);
    assert!(record.turn_sequence.is_none());
}

#[test]
fn only_a_proven_pre_write_failure_can_reuse_the_turn_boundary() {
    let mut record = FirstMessageRecord::new("manual-1", "Continue", NOW).unwrap();
    record.fence_dispatch(4, 8, LATER.into()).unwrap();
    record.retry_after_pre_write_failure().unwrap();
    assert_eq!(record.state, FirstMessageDelivery::Queued);
    assert_eq!(record.turn_sequence, Some(4));
    assert!(record.fence_dispatch(5, 9, LATER.into()).is_err());
    record.fence_dispatch(4, 9, LATER.into()).unwrap();
    assert_eq!(record.attempt, 2);
}

#[test]
fn first_message_and_paused_queue_validate_content_digests() {
    let mut record = FirstMessageRecord::new("manual-2", "Run tests", NOW).unwrap();
    record.payload_sha256 = "0".repeat(64);
    assert!(record.validate().is_err());

    let mut queued = QueuedAgentMessage::new(1, "Review output".into(), NOW.into()).unwrap();
    queued.content.push('!');
    assert!(queued.validate().is_err());
}

#[test]
fn agent_work_lease_takeover_fences_the_stale_holder() {
    let mut work = AgentWorkState::new(AgentWorkStep::AddWorktree, NOW.into());
    let first = work.claim("worker-a".into(), NOW, LATER.into()).unwrap();
    assert!(work.owns("worker-a", first));
    assert!(work
        .advance(
            "worker-b",
            first,
            AgentWorkStep::MaterializeHome,
            LATER.into(),
        )
        .is_err());
    work.advance(
        "worker-a",
        first,
        AgentWorkStep::MaterializeHome,
        LATER.into(),
    )
    .unwrap();
    assert_eq!(work.step, AgentWorkStep::MaterializeHome);
}
