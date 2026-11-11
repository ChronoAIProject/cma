use super::recovery::enabled;
use super::recovery_store::{Runtime, Store};
use super::*;
use crate::sandbox::agent_capacity_recovery::{ports::*, state::*};

const EXPIRES: &str = "2026-09-05T00:32:01Z";

#[tokio::test]
async fn reused_holder_name_cannot_retry_or_complete_an_older_fence() {
    let store = Store::new(Fixture::new());
    let runtime = Runtime::default();
    let caller = store.caller();
    let request = store.request();
    enabled(store.clone(), runtime.clone())
        .admit(&caller, &request)
        .await
        .unwrap();
    let ClaimOutcome::Claimed(old) = store
        .claim(&caller, &request, "same", NOW, LATER)
        .await
        .unwrap()
    else {
        panic!("first claim");
    };
    let (_, old_confirmation) = runtime.reattach_existing(&old).await.unwrap();
    let ClaimOutcome::Claimed(new) = store
        .claim(&caller, &request, "same", LATER, EXPIRES)
        .await
        .unwrap()
    else {
        panic!("replacement claim");
    };
    assert!(new.work.lease.as_ref().unwrap().fence > old.work.lease.as_ref().unwrap().fence);
    assert_eq!(
        store.retry(&caller, &old, LATER, EXPIRES).await,
        Err(RecoveryError::Conflict)
    );
    assert_eq!(
        store
            .complete(&caller, &old, &old_confirmation, LATER)
            .await,
        Err(RecoveryError::Conflict)
    );
    let (_, confirmation) = runtime.reattach_existing(&new).await.unwrap();
    let completed = store
        .complete(&caller, &new, &confirmation, LATER)
        .await
        .unwrap();
    assert_eq!(
        store.retry(&caller, &old, NOW, EXPIRES).await,
        Err(RecoveryError::Conflict)
    );
    assert_eq!(
        store.retry(&caller, &new, LATER, EXPIRES).await,
        Err(RecoveryError::Conflict)
    );
    let s = store.0.lock().unwrap();
    assert_eq!(completed, CommitOutcome::Recorded(s.work.clone().unwrap()));
    assert_eq!(s.publications, 1);
    assert_eq!(s.logs.len(), 2);
    assert_eq!(s.fixture.agent.work.step, AgentWorkStep::None);
}

#[tokio::test]
async fn changed_input_or_native_target_after_handshake_cannot_complete() {
    for variant in 0..8 {
        let store = Store::new(Fixture::new());
        let runtime = Runtime::default();
        let caller = store.caller();
        let request = store.request();
        enabled(store.clone(), runtime.clone())
            .admit(&caller, &request)
            .await
            .unwrap();
        let ClaimOutcome::Claimed(claim) = store
            .claim(&caller, &request, "holder", NOW, LATER)
            .await
            .unwrap()
        else {
            panic!("claim");
        };
        let (_, confirmation) = runtime.reattach_existing(&claim).await.unwrap();
        let changed = {
            let mut s = store.0.lock().unwrap();
            let a = &mut s.fixture.agent;
            match variant {
                0 => a.conversation.conversation_id = Some("another-native".into()),
                1 => a.conversation.runtime_generation = Some(99),
                2 => {
                    a.conversation.first_message =
                        Some(FirstMessageRecord::new(&a.creation_key, "new input", NOW).unwrap())
                }
                3 => a.conversation.managed_guidance_revision += 1,
                4 => a.work.receipts[0].request_sha256 = "f".repeat(64),
                5 => a.conversation.active_turn = Some(5),
                6 => a.work.next_fence += 1,
                _ => a.revision += 1,
            }
            a.clone()
        };
        assert_eq!(
            store.complete(&caller, &claim, &confirmation, NOW).await,
            Err(RecoveryError::Conflict),
            "variant {variant}"
        );
        let s = store.0.lock().unwrap();
        assert_eq!(s.fixture.agent, changed);
        assert_eq!(s.fixture.agent.status, AgentStatus::Failed);
        assert_eq!(s.work.as_ref(), Some(&claim.work));
        assert_eq!(s.publications, 0);
        assert_eq!(s.logs.len(), 1);
        assert_eq!(runtime.0.lock().unwrap().acknowledgements, 0);
    }
}

#[tokio::test]
async fn concurrent_admission_snapshots_create_only_one_successor() {
    let store = Store::new(Fixture::new());
    let caller = store.caller();
    let request = store.request();
    let first = store.capture(&caller, &request).await.unwrap();
    let second = store.capture(&caller, &request).await.unwrap();
    let plan = first.context().plan(&first.original_failure, NOW).unwrap();
    let work = RecoveryWork::prepare(request, &plan, &first.context()).unwrap();
    let next = plan.fenced_agent(&first.context(), NOW).unwrap();
    assert_eq!(
        store
            .admit(&caller, first, work.clone(), next.clone())
            .await
            .unwrap(),
        CommitOutcome::Recorded(work.clone())
    );
    assert_eq!(
        store.admit(&caller, second, work.clone(), next).await,
        Err(RecoveryError::Conflict)
    );
    let s = store.0.lock().unwrap();
    assert_eq!(s.successors, 1);
    assert_eq!(s.work.as_ref(), Some(&work));
    assert_eq!(
        s.fixture.agent.conversation.last_seq,
        work.request.failure_seq
    );
    assert_eq!(s.logs.len(), 1);
}

#[tokio::test]
async fn owner_revocation_blocks_live_receipt_reads_and_restart() {
    let store = Store::new(Fixture::new());
    let runtime = Runtime::default();
    let svc = enabled(store.clone(), runtime.clone());
    let caller = store.caller();
    let request = store.request();
    svc.admit(&caller, &request).await.unwrap();
    svc.resume(&caller, &request, "first", NOW, LATER, EXPIRES)
        .await
        .unwrap();
    let before = store.0.lock().unwrap().work.clone();
    store.0.lock().unwrap().fixture.agent.owner = crate::SubjectId::new("changed-owner");
    assert_eq!(
        svc.admit(&caller, &request).await,
        Err(RecoveryError::Unauthorized)
    );
    assert_eq!(
        svc.resume(&caller, &request, "restart", LATER, EXPIRES, EXPIRES)
            .await,
        Err(RecoveryError::Unauthorized)
    );
    let s = store.0.lock().unwrap();
    assert_eq!(s.work, before);
    assert_eq!(s.publications, 1);
    let r = runtime.0.lock().unwrap();
    assert_eq!(r.reconnects.len(), 1);
    assert_eq!(r.acknowledgements, 1);
}
