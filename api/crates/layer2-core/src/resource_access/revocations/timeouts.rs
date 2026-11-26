use super::*;
use std::time::Duration;
use tokio::sync::Semaphore;

async fn with_effects() -> (Arc<Store>, ResourceRevocationService) {
    let (store, service) = setup(8).await;
    {
        let mut state = store.0.lock().unwrap();
        let member = &mut state.members[0];
        member.effect_count = 33;
        let wire::MemberKey::Execution { execution } = &member.member else {
            panic!()
        };
        let effects = (1..=33)
            .map(|ordinal| {
                let intent = wire::EffectIntentV1 {
                    effect_id: id(&format!("effect-{ordinal}")),
                    execution: execution.clone(),
                    registration: member.registration.clone(),
                    grant: member.basis.clone(),
                    operation: wire::EffectOperationClass::ToolRequest,
                    request_digest: fixtures::digest('c'),
                    provider_idempotency: None,
                };
                EffectRecordV1 {
                    protocol: 1,
                    permission: wire::EffectPermissionV1 {
                        protocol: 1,
                        ledger_sequence: generation(ordinal),
                        permission_id: execution::digest(&(
                            "resource-effect-permission-v1",
                            &intent,
                        ))
                        .unwrap(),
                        intent,
                    },
                    observations: vec![],
                }
            })
            .collect();
        state.effects = effects;
    }
    (store, service)
}

fn retained(
    store: &Store,
) -> (
    RevocationOperationV1,
    Vec<ExecutionRecordV1>,
    Vec<EffectRecordV1>,
) {
    let state = store.0.lock().unwrap();
    (
        state.operation.clone(),
        state.members.clone(),
        state.effects.clone(),
    )
}

async fn stalled_read(point: Read, occurrence: usize) {
    let (store, service) = with_effects().await;
    let original = retained(&store);
    store.0.lock().unwrap().gates = vec![(point, occurrence, Arc::new(Semaphore::new(0)))];
    let mut read = Box::pin(status(&service, &store, "owner"));
    assert!(futures_util::poll!(read.as_mut()).is_pending());
    {
        let state = store.0.lock().unwrap();
        assert_eq!(state.reads.last(), Some(&point));
        assert_eq!(
            state.reads.iter().filter(|read| **read == point).count(),
            occurrence
        );
    }
    tokio::time::advance(Duration::from_millis(4_999)).await;
    assert!(futures_util::poll!(read.as_mut()).is_pending());
    tokio::time::advance(Duration::from_millis(1)).await;
    let std::task::Poll::Ready(result) = futures_util::poll!(read.as_mut()) else {
        panic!("owner status must release a stalled read after one five-second budget")
    };
    assert!(matches!(result, Err(ExecutionError::Unavailable)));
    drop(read);
    assert!(retained(&store) == original);
    {
        let mut state = store.0.lock().unwrap();
        state.gates.clear();
        state.reads.clear();
    }
    let recovered = status(&service, &store, "owner").await.unwrap();
    assert_eq!(recovered.operation_id, original.0.key.operation_id.as_str());
    assert_eq!(recovered.stage, RevocationStage::Pending);
    assert_eq!(
        (recovered.participants, recovered.unresolved_effects),
        (8, 33)
    );
    assert!(recovered.receipt.is_none());
    assert!(retained(&store) == original);
}

macro_rules! deadline_case {
    ($name:ident, $point:expr, $occurrence:expr) => {
        #[tokio::test(start_paused = true)]
        async fn $name() {
            stalled_read($point, $occurrence).await;
        }
    };
}

deadline_case!(initial_owner_read_has_deadline, Read::Snapshot, 1);
deadline_case!(initial_operation_read_has_deadline, Read::Operation, 1);
deadline_case!(first_cohort_page_has_deadline, Read::Members(0), 1);
deadline_case!(later_cohort_page_has_deadline, Read::Members(7), 1);
deadline_case!(first_effect_page_has_deadline, Read::Effects(0), 1);
deadline_case!(later_effect_page_has_deadline, Read::Effects(32), 1);
deadline_case!(final_owner_read_has_deadline, Read::Snapshot, 2);
deadline_case!(final_operation_read_has_deadline, Read::Operation, 2);

#[tokio::test(start_paused = true)]
async fn slow_earlier_reads_consume_the_same_status_budget() {
    let (store, service) = with_effects().await;
    let original = retained(&store);
    let first = Arc::new(Semaphore::new(0));
    store.0.lock().unwrap().gates = vec![
        (Read::Operation, 1, first.clone()),
        (Read::Members(7), 1, Arc::new(Semaphore::new(0))),
    ];
    let mut read = Box::pin(status(&service, &store, "owner"));
    assert!(futures_util::poll!(read.as_mut()).is_pending());
    tokio::time::advance(Duration::from_secs(4)).await;
    first.add_permits(1);
    assert!(futures_util::poll!(read.as_mut()).is_pending());
    assert_eq!(
        store.0.lock().unwrap().reads.last(),
        Some(&Read::Members(7))
    );
    tokio::time::advance(Duration::from_secs(1)).await;
    let std::task::Poll::Ready(result) = futures_util::poll!(read.as_mut()) else {
        panic!("later pages must not restart the owner-status read budget")
    };
    assert!(matches!(result, Err(ExecutionError::Unavailable)));
    assert!(retained(&store) == original);
}

#[tokio::test(start_paused = true)]
async fn cancelled_status_read_does_not_resume_or_mutate_retained_work() {
    let (store, service) = with_effects().await;
    let original = retained(&store);
    let gate = Arc::new(Semaphore::new(0));
    store.0.lock().unwrap().gates = vec![(Read::Members(0), 1, gate.clone())];
    let mut read = Box::pin(status(&service, &store, "owner"));
    assert!(futures_util::poll!(read.as_mut()).is_pending());
    let reads = store.0.lock().unwrap().reads.clone();
    drop(read);
    gate.add_permits(1);
    tokio::time::advance(Duration::from_secs(6)).await;
    assert_eq!(gate.available_permits(), 1);
    assert_eq!(store.0.lock().unwrap().reads, reads);
    assert!(retained(&store) == original);
    store.0.lock().unwrap().gates.clear();
    assert_eq!(
        status(&service, &store, "owner")
            .await
            .unwrap()
            .unresolved_effects,
        33
    );
    assert!(retained(&store) == original);
}
