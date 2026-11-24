use super::*;
use layer2_core::{
    qualification::{ControlOperation, ScheduleEffect},
    AgentId, OperationClaim, SubjectId,
};
use std::time::SystemTime;

const LIFETIME: Duration = Duration::from_secs(5);
const OUTCOME: ControlSchedule = ControlSchedule {
    agent_revision: 12,
    effect: ScheduleEffect::Scheduled,
};

fn claim(operation: &str, actor: &str, key: &str, fingerprint: [u8; 32]) -> OperationClaim {
    OperationClaim::new(
        SubjectId::new(actor),
        operation,
        key,
        fingerprint,
        SystemTime::UNIX_EPOCH,
        LIFETIME,
    )
}

fn select(
    operation: ControlOperation,
    agent: &str,
    claim: &OperationClaim,
) -> Option<ControlContext> {
    ControlContext::from_claim(operation, &AgentId(agent.into()), claim)
}

#[test]
fn selectors_bind_exact_operation_target_subject_key_and_fingerprint() {
    let exact = claim("agent:stop:agent-a", "subject-a", "key-a", [1; 32]);
    let expected = select(ControlOperation::Stop, "agent-a", &exact).unwrap();
    assert_eq!(ControlContext::from_digest(expected.digest()), expected);
    assert!(select(ControlOperation::Stop, "agent-b", &exact).is_none());
    assert!(select(ControlOperation::Delete, "agent-a", &exact).is_none());
    let delete = claim("agent:delete:agent-a", "subject-a", "key-a", [1; 32]);
    assert_ne!(
        select(ControlOperation::Delete, "agent-a", &delete),
        Some(expected)
    );
    for alternate in [
        claim("agent:stop:agent-a", "subject-b", "key-a", [1; 32]),
        claim("agent:stop:agent-a", "subject-a", "key-b", [1; 32]),
        claim("agent:stop:agent-a", "subject-a", "key-a", [2; 32]),
    ] {
        assert_ne!(
            select(ControlOperation::Stop, "agent-a", &alternate),
            Some(expected)
        );
    }
}

#[tokio::test(start_paused = true)]
async fn reaches_only_the_selected_checkpoint_and_only_once() {
    let registry = Registry::default();
    let selected = ControlContext::from_digest([1; 32]);
    let other = ControlContext::from_digest([2; 32]);
    let mut arm = registry.arm(selected, LIFETIME).unwrap();
    assert!(matches!(
        registry.arm(selected, LIFETIME),
        Err(ArmError::AlreadyArmed)
    ));
    registry.after_schedule(other, OUTCOME).await;
    let work = tokio::spawn({
        let registry = registry.clone();
        async move { registry.after_schedule(selected, OUTCOME).await }
    });
    assert_eq!(arm.reached().await, Some(OUTCOME));
    assert_eq!(arm.reached().await, None);
    assert!(!work.is_finished());
    registry.after_schedule(selected, OUTCOME).await;
    assert!(!work.is_finished());
    arm.release();
    work.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn expiry_and_disconnect_resume_the_original_call() {
    let registry = Registry::default();
    let selected = ControlContext::from_digest([1; 32]);
    for disconnected in [false, true] {
        let mut arm = registry.arm(selected, LIFETIME).unwrap();
        let work = tokio::spawn({
            let registry = registry.clone();
            async move { registry.after_schedule(selected, OUTCOME).await }
        });
        assert_eq!(arm.reached().await, Some(OUTCOME));
        if disconnected {
            drop(arm);
        } else {
            tokio::time::advance(LIFETIME).await;
        }
        work.await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn removing_an_expired_arm_does_not_disarm_its_replacement() {
    let registry = Registry::default();
    let selected = ControlContext::from_digest([1; 32]);
    let mut old = registry.arm(selected, LIFETIME).unwrap();
    tokio::time::advance(LIFETIME).await;
    assert_eq!(old.reached().await, None);
    let mut replacement = registry.arm(selected, LIFETIME).unwrap();
    drop(old);
    let work = tokio::spawn({
        let registry = registry.clone();
        async move { registry.after_schedule(selected, OUTCOME).await }
    });
    assert_eq!(replacement.reached().await, Some(OUTCOME));
    replacement.release();
    work.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn dropping_a_pending_checkpoint_releases_capacity() {
    let registry = Registry::default();
    let mut arms = Vec::new();
    for index in 0..MAX_ARMS as u8 {
        arms.push(
            registry
                .arm(ControlContext::from_digest([index; 32]), LIFETIME)
                .unwrap(),
        );
    }
    let another = ControlContext::from_digest([255; 32]);
    assert!(matches!(
        registry.arm(another, Duration::ZERO),
        Err(ArmError::InvalidLifetime)
    ));
    assert!(matches!(
        registry.arm(another, LIFETIME),
        Err(ArmError::AtCapacity)
    ));
    arms.pop();
    registry.arm(another, LIFETIME).unwrap();
}
