use super::participants::Participant;
use super::*;
use layer2_core::resource_access::execution::*;
use std::sync::Mutex;

mod budget;
mod fixtures;
mod io_budget;
mod mongo;
mod participants;
mod process_child;
mod process_fixture;
mod reconciliation;
mod store;
mod sweep;
use fixtures::*;
use participants::{Directory, Endpoint};
use store::{Store, WriteFault};

fn completion_barrier(completion: &MemberCompletionV1) -> &wire::BarrierReceiptV1 {
    match completion {
        MemberCompletionV1::Execution { receipt } => &receipt.barrier,
        MemberCompletionV1::Release { receipt } => &receipt.barrier,
    }
}
fn setup(rows: Vec<ExecutionRecordV1>) -> (Arc<Store>, Arc<Directory>, RevocationWorker) {
    let directory = Directory::new(&rows);
    let store = Store::new(rows);
    let worker = RevocationWorker::new(store.clone(), directory.clone());
    (store, directory, worker)
}
fn pending(value: Progress, count: u32) {
    let Progress::Pending {
        completed,
        failures,
        deadline_exceeded,
        ..
    } = value
    else {
        panic!("missing evidence cannot finalize a revocation")
    };
    assert_eq!(completed, count);
    assert!(!failures.is_empty());
    assert!(
        deadline_exceeded,
        "expiry is reported, never substituted for evidence"
    );
}
fn retain(endpoint: &Endpoint, operation: &RevocationOperationV1) {
    let request = endpoint.row.barrier_request(operation).unwrap();
    *endpoint.retained.lock().unwrap() = Some(endpoint.completion(&request));
}

#[tokio::test]
async fn full_cohort_pages_and_both_participant_kinds_finalize() {
    let rows = (1..=MAX_FAMILY_MEMBERS)
        .map(|n| {
            if n % 2 == 0 {
                release_member(n)
            } else {
                member(n)
            }
        })
        .collect();
    let (store, directory, worker) = setup(rows);
    assert_eq!(
        worker.reconcile(&store.id()).await.unwrap(),
        Progress::Effective
    );
    let state = store.0.lock().unwrap();
    assert_eq!(state.pages, [0, 32]);
    assert_eq!(state.finalizations, 1);
    assert!(state
        .members
        .iter()
        .all(|member| member.completion.is_some()));
    assert_eq!(state.events.last(), Some(&"finalize"));
    for endpoint in directory.0.values() {
        assert_eq!(
            *endpoint.calls.lock().unwrap(),
            ["query", "barrier", "stop"]
        );
    }
}

#[tokio::test]
async fn short_pages_are_followed_until_the_frozen_count() {
    let (store, _, worker) = setup((1..=5).map(member).collect());
    store.0.lock().unwrap().page_size = 2;
    assert_eq!(
        worker.reconcile(&store.id()).await.unwrap(),
        Progress::Effective
    );
    assert_eq!(store.0.lock().unwrap().pages, [0, 2, 4]);
}

#[tokio::test]
async fn missing_page_never_looks_like_an_empty_cohort() {
    let (store, directory, worker) = setup(vec![member(1)]);
    store.0.lock().unwrap().missing_page = true;
    assert_eq!(
        worker.reconcile(&store.id()).await,
        Err(ExecutionError::Corrupt)
    );
    assert_eq!(store.0.lock().unwrap().finalizations, 0);
    assert!(directory.endpoint(1).calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn duplicate_gap_wrong_epoch_and_foreign_family_are_refused_before_control() {
    for fault in 0..4 {
        let (store, directory, worker) = setup(vec![member(1), member(2)]);
        {
            let mut state = store.0.lock().unwrap();
            match fault {
                0 => state.members[1] = state.members[0].clone(),
                1 => state.members[1].ordinal = 3,
                2 => state.members[1].epoch = generation(2),
                _ => {
                    let row = &mut state.members[1];
                    row.permit = None;
                    if let wire::ExecutionBasisV1::Share { grant } = &mut row.basis {
                        grant.family.generation = generation(9);
                    }
                }
            }
        }
        assert!(worker.reconcile(&store.id()).await.is_err());
        assert_eq!(store.0.lock().unwrap().finalizations, 0);
        assert!(directory
            .0
            .values()
            .all(|endpoint| endpoint.calls.lock().unwrap().is_empty()));
    }
}

#[tokio::test]
async fn wrong_authenticated_boot_or_participant_kind_is_never_contacted() {
    for wrong_kind in [false, true] {
        let (store, directory, worker) = setup(vec![member(1)]);
        {
            let mut behavior = directory.endpoint(1).behavior.lock().unwrap();
            behavior.wrong_kind = wrong_kind;
            behavior.wrong_audience = !wrong_kind;
        }
        pending(worker.reconcile(&store.id()).await.unwrap(), 0);
        assert_eq!(store.0.lock().unwrap().finalizations, 0);
        assert!(directory.endpoint(1).calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn retained_receipt_is_recovered_before_another_stop_request() {
    let (store, directory, worker) = setup(vec![member(1), release_member(2)]);
    for endpoint in directory.0.values() {
        retain(endpoint, &store.0.lock().unwrap().operation);
    }
    assert_eq!(
        worker.reconcile(&store.id()).await.unwrap(),
        Progress::Effective
    );
    for endpoint in directory.0.values() {
        assert_eq!(*endpoint.calls.lock().unwrap(), ["query"]);
    }
    let state = store.0.lock().unwrap();
    assert_eq!(
        state.events,
        [
            "read_operation",
            "persist_barrier",
            "persist_completion",
            "persist_barrier",
            "persist_completion",
            "finalize"
        ]
    );
}

#[tokio::test]
async fn lost_stop_reply_survives_manager_restart_with_original_identity() {
    for row in [member(1), release_member(1)] {
        let (store, directory, worker) = setup(vec![row]);
        directory.endpoint(1).behavior.lock().unwrap().lose_reply = true;
        let id = store.id();
        pending(worker.reconcile(&id).await.unwrap(), 0);
        assert_eq!(store.0.lock().unwrap().finalizations, 0);
        let restarted = RevocationWorker::new(store.clone(), directory.clone());
        assert_eq!(restarted.reconcile(&id).await.unwrap(), Progress::Effective);
        assert_eq!(store.id(), id);
        assert_eq!(
            *directory.endpoint(1).calls.lock().unwrap(),
            ["query", "barrier", "stop", "query"]
        );
    }
}

#[tokio::test]
async fn ambiguous_evidence_commits_are_reloaded_without_new_work() {
    for cut in [WriteFault::Barrier, WriteFault::Completion] {
        let (store, directory, worker) = setup(vec![member(1)]);
        store.0.lock().unwrap().fault = Some(cut);
        pending(worker.reconcile(&store.id()).await.unwrap(), 0);
        assert_eq!(store.0.lock().unwrap().finalizations, 0);
        let restarted = RevocationWorker::new(store.clone(), directory.clone());
        assert_eq!(
            restarted.reconcile(&store.id()).await.unwrap(),
            Progress::Effective
        );
        let calls = directory.endpoint(1).calls.lock().unwrap();
        assert_eq!(calls.iter().filter(|&&call| call == "stop").count(), 1);
        if cut == WriteFault::Barrier {
            assert_eq!(*calls, ["query", "barrier", "query", "barrier", "stop"]);
        }
    }
}

#[tokio::test]
async fn unknown_finalization_is_reconciled_from_the_original_operation() {
    let (store, directory, worker) = setup(vec![member(1)]);
    store.0.lock().unwrap().fault = Some(WriteFault::Finalize);
    assert_eq!(
        worker.reconcile(&store.id()).await,
        Err(ExecutionError::Unknown)
    );
    let calls = directory.endpoint(1).calls.lock().unwrap().clone();
    let restarted = RevocationWorker::new(store.clone(), directory.clone());
    assert_eq!(
        restarted.reconcile(&store.id()).await.unwrap(),
        Progress::Effective
    );
    assert_eq!(*directory.endpoint(1).calls.lock().unwrap(), calls);
    assert_eq!(store.0.lock().unwrap().finalizations, 1);
}

#[tokio::test]
async fn duplicate_workers_retain_identical_evidence() {
    let (store, directory, first) = setup(vec![member(1), release_member(2)]);
    *directory.endpoint(1).query_gate.lock().unwrap() =
        Some(Arc::new(tokio::sync::Barrier::new(2)));
    let second = RevocationWorker::new(store.clone(), directory.clone());
    let id = store.id();
    let (a, b) = tokio::join!(first.reconcile(&id), second.reconcile(&id));
    assert_eq!(a.unwrap(), Progress::Effective);
    assert_eq!(b.unwrap(), Progress::Effective);
    let state = store.0.lock().unwrap();
    state.operation.validate_cohort(&state.members).unwrap();
    let calls = directory.endpoint(1).calls.lock().unwrap();
    assert_eq!(calls.iter().filter(|&&call| call == "query").count(), 2);
    assert_eq!(calls.iter().filter(|&&call| call == "stop").count(), 2);
}

#[tokio::test]
async fn unavailable_member_does_not_starve_other_members_or_finalize() {
    let (store, directory, worker) = setup((1..=12).map(member).collect());
    directory.endpoint(1).behavior.lock().unwrap().unavailable = true;
    pending(worker.reconcile(&store.id()).await.unwrap(), 11);
    let state = store.0.lock().unwrap();
    assert!(state.members[0].completion.is_none());
    assert!(state.members[1..]
        .iter()
        .all(|member| member.completion.is_some()));
    assert_eq!(state.finalizations, 0);
    assert_eq!(*directory.endpoint(1).calls.lock().unwrap(), ["query"]);
}

#[tokio::test(start_paused = true)]
async fn stalled_observation_expires_without_becoming_stop_evidence() {
    let (store, directory, worker) = setup((1..=12).map(member).collect());
    directory.endpoint(1).behavior.lock().unwrap().stall = true;
    pending(worker.reconcile(&store.id()).await.unwrap(), 11);
    assert_eq!(store.0.lock().unwrap().finalizations, 0);
    directory.endpoint(1).behavior.lock().unwrap().stall = false;
    assert_eq!(
        worker.reconcile(&store.id()).await.unwrap(),
        Progress::Effective
    );
}

#[tokio::test]
async fn active_local_writer_is_not_an_external_effect_exception() {
    let (store, directory, worker) = setup(vec![member(1)]);
    directory.endpoint(1).behavior.lock().unwrap().local_writer = true;
    pending(worker.reconcile(&store.id()).await.unwrap(), 0);
    assert_eq!(store.0.lock().unwrap().finalizations, 0);
    assert!(store.0.lock().unwrap().members[0].completion.is_none());
}

#[tokio::test]
async fn body_closure_cannot_acknowledge_an_undrained_transport() {
    let (store, directory, worker) = setup(vec![release_member(1)]);
    directory
        .endpoint(1)
        .behavior
        .lock()
        .unwrap()
        .transport_open = true;
    pending(worker.reconcile(&store.id()).await.unwrap(), 0);
    assert_eq!(store.0.lock().unwrap().finalizations, 0);
}

#[tokio::test]
async fn unknown_external_effects_do_not_authorize_replay() {
    let mut row = member(1);
    row.effect_count = 2;
    let (store, _, worker) = setup(vec![row]);
    assert_eq!(
        worker.reconcile(&store.id()).await.unwrap(),
        Progress::Effective
    );
    let state = store.0.lock().unwrap();
    let Some(MemberCompletionV1::Execution { receipt }) = &state.members[0].completion else {
        panic!()
    };
    assert_eq!(receipt.quiescence.effects.unresolved_count, 2);
    assert_eq!(receipt.quiescence.effects.high_water, 2);
}

#[tokio::test]
async fn wrong_effect_high_water_or_barrier_watermark_remains_pending() {
    for high_water in [true, false] {
        let (store, directory, worker) = setup(vec![member(1)]);
        {
            let mut behavior = directory.endpoint(1).behavior.lock().unwrap();
            behavior.bad_high_water = high_water;
            behavior.bad_watermark = !high_water;
        }
        pending(worker.reconcile(&store.id()).await.unwrap(), 0);
        assert_eq!(store.0.lock().unwrap().finalizations, 0);
    }
}

#[tokio::test]
async fn retained_receipt_for_another_operation_is_rejected() {
    let (store, directory, worker) = setup(vec![member(1)]);
    retain(directory.endpoint(1), &store.0.lock().unwrap().operation);
    {
        let mut retained = directory.endpoint(1).retained.lock().unwrap();
        let Some(MemberCompletionV1::Execution { receipt }) = retained.as_mut() else {
            panic!()
        };
        receipt.barrier.request.revocation.operation_id = digest('9');
    }
    pending(worker.reconcile(&store.id()).await.unwrap(), 0);
    assert_eq!(store.0.lock().unwrap().events, ["read_operation"]);
}

#[tokio::test]
async fn discovery_retains_bounded_strict_cursor_and_rejects_duplicate_rows() {
    let (store, _, worker) = setup(Vec::new());
    {
        let mut state = store.0.lock().unwrap();
        for n in 1..=36 {
            let mut row = operation(0);
            row.intent.key_sha256 = wire::Digest::new(&format!("{n:064x}")).unwrap();
            let request = row.intent.mutation(1);
            row.intent.fingerprint = wire::Digest::new(&request.fingerprint()).unwrap();
            row.key.operation_id = wire::Digest::new(&request.operation_id()).unwrap();
            state.extra_discovery.push(row);
        }
    }
    let first = worker.discover(None).await.unwrap();
    assert_eq!(first.operations.len(), 32);
    let second = worker.discover(first.next.as_ref()).await.unwrap();
    assert_eq!(second.operations.len(), 5);
    assert!(second.next.is_none());
    assert!(second
        .operations
        .iter()
        .all(|id| id.as_str() > first.next.as_ref().unwrap().as_str()));
    store.0.lock().unwrap().duplicate_discovery = true;
    assert!(matches!(
        worker.discover(None).await,
        Err(ExecutionError::Corrupt)
    ));
}

#[tokio::test]
async fn unsupported_control_ports_never_supply_completion() {
    struct Unsupported;
    #[async_trait::async_trait]
    impl ParticipantDirectory for Unsupported {
        async fn resolve(&self, member: &wire::MemberKey) -> ExecutionResult<Participant> {
            Ok(match member {
                wire::MemberKey::Execution { .. } => Participant::execution(
                    member.receiver().clone(),
                    Arc::new(wire::UnsupportedExecutionControl),
                ),
                wire::MemberKey::Release { .. } => Participant::release(
                    member.receiver().clone(),
                    Arc::new(UnsupportedReleaseGate),
                ),
            })
        }
    }
    let store = Store::new(vec![member(1), release_member(2)]);
    let worker = RevocationWorker::new(store.clone(), Arc::new(Unsupported));
    pending(worker.reconcile(&store.id()).await.unwrap(), 0);
    assert_eq!(store.0.lock().unwrap().finalizations, 0);
}

#[tokio::test]
async fn foreign_retained_completion_cannot_count_as_stopped() {
    let (store, directory, worker) = setup(vec![member(1)]);
    {
        let mut state = store.0.lock().unwrap();
        let mut other = state.operation.clone();
        other.key.operation_id = digest('b');
        let row = &mut state.members[0];
        let completion = directory
            .0
            .values()
            .next()
            .unwrap()
            .completion(&row.barrier_request(&other).unwrap());
        let barrier = completion_barrier(&completion).clone();
        row.observe_barrier(&barrier.request, &barrier).unwrap();
        row.observe_completion(&completion).unwrap();
    }
    pending(worker.reconcile(&store.id()).await.unwrap(), 0);
    assert_eq!(store.0.lock().unwrap().finalizations, 0);
}
