use super::super::super::sweep::{RevocationSweep, SweepEvent, SweepWork};
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

struct Workers {
    workers: Vec<(Arc<Store>, RevocationWorker)>,
    discovered: AtomicBool,
}

#[async_trait::async_trait]
impl SweepWork for Workers {
    async fn discover(&self, _: Option<&wire::Digest>) -> ExecutionResult<PendingPage> {
        Ok(PendingPage {
            operations: if self.discovered.swap(true, Ordering::SeqCst) {
                Vec::new()
            } else {
                self.workers.iter().map(|(store, _)| store.id()).collect()
            },
            next: None,
        })
    }

    async fn reconcile(&self, operation: &wire::Digest) -> ExecutionResult<Progress> {
        let (_, worker) = self
            .workers
            .iter()
            .find(|(store, _)| store.id() == *operation)
            .unwrap();
        worker.reconcile(operation).await
    }
}

#[tokio::test(start_paused = true)]
async fn four_stalled_store_operations_release_sweep_slots_for_later_work() {
    for cut in ["operation", "cohort", "finalization"] {
        let workers = (1..=5)
            .map(|n| {
                let (store, _, worker) = setup(vec![member(1)]);
                {
                    let mut state = store.0.lock().unwrap();
                    state.operation.intent.key_sha256 =
                        wire::Digest::new(&format!("{n:064x}")).unwrap();
                    let request = state.operation.intent.mutation(1);
                    state.operation.intent.fingerprint =
                        wire::Digest::new(&request.fingerprint()).unwrap();
                    state.operation.key.operation_id =
                        wire::Digest::new(&request.operation_id()).unwrap();
                    state.operation.validate().unwrap();
                    if n < 5 {
                        let gate = Arc::new(Semaphore::new(0));
                        match cut {
                            "operation" => state.operation_gate = Some(gate),
                            "cohort" => state.cohort_page_gate = Some((0, gate)),
                            _ => state.finalize_before_gate = Some(gate),
                        }
                    }
                }
                (store, worker)
            })
            .collect();
        let work = Arc::new(Workers {
            workers,
            discovered: AtomicBool::new(false),
        });
        let later = work.workers[4].0.id();
        let mut sweep = RevocationSweep::with_work(work.clone());
        let observed = tokio::time::timeout(Duration::from_secs(8), async {
            let mut results = std::collections::HashMap::new();
            while results.len() < 5 {
                if let SweepEvent::Reconciled { operation, result } = sweep.next().await {
                    results.insert(operation, result);
                }
            }
            results
        })
        .await
        .expect("stalled store IO must not starve the fifth queued revocation");
        assert_eq!(observed[&later], Ok(Progress::Effective));
        for (store, worker) in &work.workers[..4] {
            let original = store.id();
            assert_eq!(
                observed[&original],
                Err(if cut == "finalization" {
                    ExecutionError::Unknown
                } else {
                    ExecutionError::Unavailable
                })
            );
            {
                let mut state = store.0.lock().unwrap();
                assert!(matches!(state.operation.phase, RevocationPhaseV1::Pending));
                state.operation_gate = None;
                state.cohort_page_gate = None;
                state.finalize_before_gate = None;
            }
            assert_eq!(
                worker.reconcile(&original).await.unwrap(),
                Progress::Effective
            );
            assert_eq!(store.id(), original);
        }
        assert!(sweep.shutdown().await.is_empty());
    }
}
