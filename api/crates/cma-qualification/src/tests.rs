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
    assert_eq!(
        ControlContext::from_digests(expected.digest(), expected.agent_digest()),
        expected
    );
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
    let selected = ControlContext::from_digests([1; 32], [0; 32]);
    let other = ControlContext::from_digests([2; 32], [0; 32]);
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
    assert_eq!(arm.reached().await.unwrap().schedule, OUTCOME);
    assert!(arm.reached().await.is_none());
    assert!(!work.is_finished());
    registry.after_schedule(selected, OUTCOME).await;
    assert!(!work.is_finished());
    arm.release();
    work.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn expiry_and_disconnect_resume_the_original_call() {
    let registry = Registry::default();
    let selected = ControlContext::from_digests([1; 32], [0; 32]);
    for disconnected in [false, true] {
        let mut arm = registry.arm(selected, LIFETIME).unwrap();
        let work = tokio::spawn({
            let registry = registry.clone();
            async move { registry.after_schedule(selected, OUTCOME).await }
        });
        assert_eq!(arm.reached().await.unwrap().schedule, OUTCOME);
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
    let selected = ControlContext::from_digests([1; 32], [0; 32]);
    let mut old = registry.arm(selected, LIFETIME).unwrap();
    tokio::time::advance(LIFETIME).await;
    assert!(old.reached().await.is_none());
    let mut replacement = registry.arm(selected, LIFETIME).unwrap();
    drop(old);
    let work = tokio::spawn({
        let registry = registry.clone();
        async move { registry.after_schedule(selected, OUTCOME).await }
    });
    assert_eq!(replacement.reached().await.unwrap().schedule, OUTCOME);
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
                .arm(ControlContext::from_digests([index; 32], [0; 32]), LIFETIME)
                .unwrap(),
        );
    }
    let another = ControlContext::from_digests([255; 32], [0; 32]);
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

#[tokio::test(start_paused = true)]
async fn cancellation_consumes_only_the_original_owner_and_its_linked_receipt() {
    use layer2_core::agent_ownership::{
        qualification_owner_digest, test_support::MemoryOwnershipStore, Member,
        OwnershipCoordinator, OwnershipStore, PROTOCOL_VERSION,
    };
    let store = MemoryOwnershipStore::default();
    let coordinator = OwnershipCoordinator::register(
        store.client(),
        Member {
            instance_id: "fixture-instance".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:8080".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 2,
        },
    )
    .await
    .unwrap();
    let agent = AgentId("agent-a".into());
    let authority = coordinator.acquire(&agent).await.unwrap().unwrap();
    let durable = store.owner(&agent).await.unwrap().unwrap();
    let mut wrong_epoch = authority.token.clone();
    wrong_epoch.epoch += 1;
    assert!(coordinator
        .capture_qualification_owner(&wrong_epoch)
        .await
        .is_none());
    let capture = coordinator
        .capture_qualification_owner(&authority.token)
        .await
        .unwrap();
    let mut wrong_process = authority.token.clone();
    wrong_process.instance_id = "other-fixture-instance".into();
    assert!(coordinator
        .capture_qualification_owner_digest(
            capture.agent_digest(),
            qualification_owner_digest(&wrong_process)
        )
        .await
        .is_none());
    assert!(coordinator
        .capture_qualification_owner_digest(
            capture.agent_digest(),
            qualification_owner_digest(&wrong_epoch)
        )
        .await
        .is_none());

    let selected = select(
        ControlOperation::Stop,
        agent.as_str(),
        &claim("agent:stop:agent-a", "subject-a", "key-a", [0; 32]),
    )
    .unwrap();
    let registry = Registry::default();
    let other = ControlContext::from_digests([99; 32], selected.agent_digest());
    let stale_capture = coordinator
        .capture_qualification_owner(&authority.token)
        .await
        .unwrap();
    let mut arm = registry
        .arm_with_owner(selected, LIFETIME, capture)
        .unwrap();
    let mut other_arm = registry
        .arm_with_owner(other, LIFETIME, stale_capture)
        .unwrap();
    let worker = |context| {
        let registry = registry.clone();
        tokio::spawn(async move { registry.after_schedule(context, OUTCOME).await })
    };
    let other_work = worker(other);
    let unrelated_receipt = other_arm.reached().await.unwrap();
    assert!(!arm.cancel_owner(&unrelated_receipt));
    assert!(authority.is_current());

    let work = worker(selected);
    let receipt = arm.reached().await.unwrap();
    assert!(arm.cancel_owner(&receipt));
    assert!(!arm.cancel_owner(&receipt));
    assert!(authority.receiver_cancelled());
    assert_eq!(store.owner(&agent).await.unwrap(), Some(durable));
    assert!(!work.is_finished());

    coordinator
        .handoff(&authority, async { Ok(()) })
        .await
        .unwrap();
    let successor = coordinator.acquire(&agent).await.unwrap().unwrap();
    assert!(successor.token.epoch > authority.token.epoch);
    assert!(coordinator
        .capture_qualification_owner_digest(
            selected.agent_digest(),
            qualification_owner_digest(&authority.token)
        )
        .await
        .is_none());
    assert!(!other_arm.cancel_owner(&unrelated_receipt));
    assert!(successor.is_current());
    assert!(!successor.receiver_cancelled());

    arm.release();
    other_arm.release();
    work.await.unwrap();
    other_work.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn selected_renewal_withholds_only_the_capture_and_records_the_callers_timeout() {
    assert_withheld_renewal(false).await;
}

#[tokio::test(start_paused = true)]
async fn disarming_after_commit_keeps_the_renewal_reply_withheld() {
    assert_withheld_renewal(true).await;
}

async fn assert_withheld_renewal(disarm_after_commit: bool) {
    use layer2_core::agent_ownership::{
        test_support::MemoryOwnershipStore, Member, OwnershipCoordinator, OwnershipStore,
        PROTOCOL_VERSION,
    };
    let store = MemoryOwnershipStore::default();
    let coordinator = OwnershipCoordinator::register(
        store.client(),
        Member {
            instance_id: "fixture-instance".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:8080".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 2,
        },
    )
    .await
    .unwrap();
    let selected = AgentId("selected".into());
    let healthy = AgentId("healthy".into());
    let selected = coordinator.acquire(&selected).await.unwrap().unwrap();
    let healthy = coordinator.acquire(&healthy).await.unwrap().unwrap();
    let original = store
        .owner(&selected.token.agent_id)
        .await
        .unwrap()
        .unwrap();
    let capture = coordinator
        .capture_qualification_owner(&selected.token)
        .await
        .unwrap();
    let registry = Registry::default();
    assert!(coordinator.attach_qualification_renewal(Arc::new(registry.clone())));
    let mut armed = registry
        .arm_renewal(capture, store.client(), LIFETIME)
        .unwrap();
    tokio::time::advance(Duration::from_millis(10)).await;
    let mut task = tokio::spawn({
        let coordinator = coordinator.clone();
        async move { coordinator.maintain().await }
    });

    loop {
        let facts = armed.changed().await.expect("renewal evidence");
        if facts.result.is_some() {
            assert_eq!(facts.result, Some(RenewalOutcome::Committed));
            assert!(facts.started);
            assert!(!facts.caller_dropped);
            break;
        }
    }
    assert!(!task.is_finished());
    assert!(selected.is_current());
    assert!(healthy.is_current());
    assert!(
        store
            .owner(&selected.token.agent_id)
            .await
            .unwrap()
            .unwrap()
            .expires_at_ms
            > original.expires_at_ms
    );

    if disarm_after_commit {
        armed.disarm();
        assert!(tokio::time::timeout(Duration::from_millis(100), &mut task)
            .await
            .is_err());
        assert!(selected.is_current());
        task.await.unwrap().unwrap();
    } else {
        task.await.unwrap().unwrap();
        assert!(armed.evidence().caller_dropped);
    }
    assert!(selected.receiver_cancelled());
    assert!(healthy.is_current());
    assert!(coordinator.is_ready());
}
