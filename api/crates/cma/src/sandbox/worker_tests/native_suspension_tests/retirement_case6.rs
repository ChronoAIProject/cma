use super::*;
use crate::sandbox::agent_worker::{AgentWorker, AgentWorkerConfig, AgentWorkerDependencies};
use crate::store::agent_ownership::MongoAgentOwnership;
use layer2_core::agent_ownership::{
    Authority, Member, OwnershipCoordinator, OwnershipStore, PROTOCOL_VERSION,
};
use layer2_core::sandbox::{Agent, AgentWorkReceipt, AgentWorkStep};
use serde_json::json;
mod faults;
mod negatives;
mod preclose;
pub(super) mod records;
mod support;
use support::*;

pub(crate) async fn exercise_native_retirement_finality() {
    retirement_case6_healthy_siblings_complete().await;
    retirement_case6_permanent_404_completes_after_native_finality().await;
    negatives::retirement_case6_negative_boundaries_retain_debt().await;
    negatives::retirement_case6_retained_provider_denial_blocks_paused_siblings().await;
    negatives::retirement_case6_replaced_provider_uid_blocks_retained_delete().await;
    faults::retirement_case6_retained_claim_survives_proof_failure_then_settles_without_expiry()
        .await;
    faults::retirement_case6_refuses_external_debt_and_active_parent_control().await;
    faults::recovery_between_admission_and_dispatch_returns_to_graceful_stop().await;
    faults::provider_retirement_refuses_a_child_writer_that_wins_admission().await;
    faults::disabled_admission_preserves_legacy_absent_runtime_retirement().await;
}

async fn retirement_case6_healthy_siblings_complete() {
    let (f, active, children, mut now) = active_siblings().await;
    let clock = Arc::new(Mutex::new(now));
    let worker = finalizing_parent_worker(&f, clock.clone());
    let owners = Owners::new().await;
    let workers: Vec<_> = owners
        .coordinators
        .iter()
        .enumerate()
        .map(|(i, c)| child_worker(&f, Some(c.clone()), &format!("healthy-{i}")))
        .collect();
    admit(&f, &active).await;
    for _ in 0..20 {
        now += Duration::from_secs(6);
        *clock.lock().unwrap() = now + Duration::from_secs(1);
        worker.run_one(&active.id, now).await.unwrap();
        for child_worker in &workers {
            for child in &children {
                child_worker.run_one(&child.id, now).await.unwrap();
            }
        }
        if f.harness.stored(&active.id).await.status == SandboxStatus::Retired {
            break;
        }
    }
    assert!(!f
        .backend
        .calls
        .snapshot()
        .contains(&"retire-alternative".into()));
    let parent = f.harness.stored(&active.id).await;
    assert_eq!(parent.status, SandboxStatus::Retired);
    assert!(parent.runtime.is_none());
    assert!(parent.credential.current.is_none());
    assert!(parent.lifecycle.home.as_ref().unwrap().released);
    assert!(f
        .harness
        .adapter
        .runtime_inventory()
        .await
        .unwrap()
        .is_empty());
    for child in &children {
        let stored = f.harness.store.get_agent(&child.id).await.unwrap().unwrap();
        assert_eq!(stored.status, AgentStatus::Stopped);
    }
    report(
        "healthy_siblings",
        &f,
        &active.id,
        &children,
        json!({"complete":true,"replicas":2}),
    )
    .await;
    owners.cleanup().await;
}

async fn retirement_case6_permanent_404_completes_after_native_finality() {
    let (f, active, children, mut now) = active_siblings().await;
    let owners = Owners::new().await;
    let receiver = Receiver::new(&f, &active).await;
    let parent_worker = parent_worker(&f);
    let workers: Vec<_> = owners
        .coordinators
        .iter()
        .enumerate()
        .map(|(i, c)| child_worker(&f, Some(c.clone()), &format!("replica-{i}")))
        .collect();
    admit(&f, &active).await;
    let mut old = Vec::new();
    for child in &children {
        let authority = owners.acquire(&child.id).await;
        old.push(authority);
    }
    for _ in 0..4 {
        now += Duration::from_secs(6);
        parent_worker.run_one(&active.id, now).await.unwrap();
        for worker in &workers {
            for child in &children {
                let result = worker.run_one(&child.id, now).await;
                assert!(matches!(result, Ok(_) | Err(SandboxStoreError::Conflict)));
            }
        }
    }
    for authority in &old {
        assert!(authority.receiver_cancelled());
        let coordinator = owners
            .coordinators
            .iter()
            .find(|c| c.instance_id == authority.token.instance_id)
            .unwrap();
        assert!(coordinator
            .handoff(authority, authority.revoke_transports())
            .await
            .is_err());
    }
    let mut before = Vec::new();
    for child in &children {
        before.push(owners.store.owner(&child.id).await.unwrap().unwrap());
    }
    for _ in 0..3 {
        now += Duration::from_secs(6);
        parent_worker.run_one(&active.id, now).await.unwrap();
        for worker in &workers {
            for child in &children {
                assert!(!worker.run_one(&child.id, now).await.unwrap());
            }
        }
    }
    for (child, lease) in children.iter().zip(&before) {
        let after = owners.store.owner(&child.id).await.unwrap().unwrap();
        assert_eq!(after.expires_at_ms, lease.expires_at_ms);
        assert_eq!(after.token.epoch, lease.token.epoch);
    }
    blocked(&f, &active, &children).await;
    let revision_before_repeat = f.harness.stored(&active.id).await.revision;
    let repeat = f
        .harness
        .service
        .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &active.id)
        .await
        .unwrap();
    assert_eq!(repeat.status, SandboxStatus::Retiring);
    assert_eq!(repeat.revision, revision_before_repeat);
    let calls_before = receiver.calls.load(Ordering::SeqCst);
    // Heartbeat membership without renewing cancelled custody; expiry is Mongo time.
    let started = tokio::time::Instant::now();
    loop {
        for c in &owners.coordinators {
            c.maintain().await.unwrap();
        }
        let membership = owners.store.membership().await.unwrap();
        if before
            .iter()
            .all(|lease| membership.now_ms >= lease.expires_at_ms)
        {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(25));
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let mut successors = Vec::new();
    let mut work_due = now;
    let mut retained_work_claims = 0;
    let mut already_due_retries = 0;
    for (child, previous) in children.iter().zip(&old) {
        let successor = owners.acquire(&child.id).await;
        assert!(successor.token.epoch > previous.token.epoch);
        assert!(successor.is_current());
        let selected = owners
            .coordinators
            .iter()
            .position(|c| c.instance_id == successor.token.instance_id)
            .unwrap();
        let stored = f.harness.store.get_agent(&child.id).await.unwrap().unwrap();
        let due = stored.work.is_due(&rfc3339_at(now));
        println!(
            "RETIREMENT_RETRY_TIMING {}",
            json!({"due":due,"work_claim_retained":stored.work.lease.is_some(),"failure_class":stored.failure.as_ref().map(|f|f.class.clone())})
        );
        let result = workers[selected].run_one(&child.id, now).await;
        if due {
            assert!(matches!(result, Err(SandboxStoreError::Conflict)));
            assert!(successor.receiver_cancelled());
            already_due_retries += 1;
        } else {
            assert!(!result.unwrap());
            let lease = stored
                .work
                .lease
                .as_ref()
                .expect("not-due cleanup retains its work claim");
            work_due = work_due.max(stamp_time(&lease.expires_at) + Duration::from_secs(1));
            assert!(!successor.receiver_cancelled());
            retained_work_claims += 1;
        }
        successors.push((successor, selected, !due));
    }
    // Only a retained in-memory lifecycle claim advances the scheduling clock.
    // Ownership/store expiry above was real time; persisted leases are never edited.
    now = work_due;
    for (index, child) in children.iter().enumerate() {
        let (successor, selected, waiting) = &successors[index];
        if *waiting {
            assert!(matches!(
                workers[*selected].run_one(&child.id, now).await,
                Err(SandboxStoreError::Conflict)
            ));
        }
        assert!(successor.receiver_cancelled());
        let before_peer = f.harness.store.get_agent(&child.id).await.unwrap().unwrap();
        let peer = workers[1 - selected].run_one(&child.id, now).await;
        assert!(matches!(peer, Ok(false) | Err(SandboxStoreError::Conflict)));
        assert_eq!(
            f.harness.store.get_agent(&child.id).await.unwrap().unwrap(),
            before_peer
        );
    }
    assert_eq!(retained_work_claims + already_due_retries, 2);
    let new_epochs: Vec<_> = successors.iter().map(|(a, _, _)| a.token.epoch).collect();
    parent_worker.run_one(&active.id, now).await.unwrap();
    blocked(&f, &active, &children).await;
    assert!(receiver.calls.load(Ordering::SeqCst) > calls_before);
    report("permanent_404_siblings", &f, &active.id, &children, json!({
        "complete":false,"challenge_http":404,"old_epochs":old.iter().map(|a|a.token.epoch).collect::<Vec<_>>(),
        "new_epochs":new_epochs,"lease_expiry":"real Mongo time; no expiry mutation",
        "live_lease_unchanged":true,"successor_waits_for_work_claim":retained_work_claims > 0,"retained_work_claims":retained_work_claims,"already_due_retries":already_due_retries,"work_claim_seconds":1080,"receiver_calls":receiver.calls.load(Ordering::SeqCst),
        "repeat_delete":"Retiring; idempotent","replicas":2,"receiver_revoke":"404; handoff refused"
    })).await;
    let clock = Arc::new(Mutex::new(now));
    let finalizer = finalizing_parent_worker(&f, clock.clone());
    let mut live_owners = Vec::new();
    for child in &children {
        live_owners.push(owners.store.owner(&child.id).await.unwrap().unwrap());
    }
    let calls_at_finality = receiver.calls.load(Ordering::SeqCst);
    for _ in 0..12 {
        now += Duration::from_secs(6);
        *clock.lock().unwrap() = now + Duration::from_secs(1);
        finalizer.run_one(&active.id, now).await.unwrap();
        if f.harness.stored(&active.id).await.status == SandboxStatus::Retired {
            break;
        }
    }
    let retired = f.harness.stored(&active.id).await;
    assert_eq!(
        retired.status,
        SandboxStatus::Retired,
        "{:?}",
        retired.failure
    );
    assert_eq!(retired.work.step, SandboxWorkStep::WaitForPurge);
    assert!(retired.runtime.is_none());
    assert!(retired.credential.current.is_none());
    assert!(retired.lifecycle.home.as_ref().unwrap().released);
    assert!(f
        .backend
        .calls
        .snapshot()
        .contains(&"verify-retired".into()));
    assert!(receiver.calls.load(Ordering::SeqCst) > calls_at_finality);
    for (child, before) in children.iter().zip(&live_owners) {
        let after = owners.store.owner(&child.id).await.unwrap().unwrap();
        assert_eq!(after.token, before.token);
        assert_eq!(after.expires_at_ms, before.expires_at_ms);
        let settled = f.harness.store.get_agent(&child.id).await.unwrap().unwrap();
        assert_eq!(settled.status, AgentStatus::Stopped);
        assert_eq!(settled.work.step, AgentWorkStep::None);
        assert!(settled.work.lease.is_none());
        assert_eq!(
            settled.conversation.conversation_id,
            child.conversation.conversation_id
        );
        assert!(!settled
            .work
            .receipts
            .iter()
            .any(|r| r.step == AgentWorkStep::CloseProcess));
    }
    report(
        "permanent_404_finalized",
        &f,
        &active.id,
        &children,
        json!({"complete":true,"ownership_unchanged":true,"receiver_ack_fabricated":false}),
    )
    .await;
    receiver.server.abort();
    owners.cleanup().await;
}
