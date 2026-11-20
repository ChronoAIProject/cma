use std::sync::{Arc, Mutex};

use layer2_core::sandbox::ingress_ownership::{
    AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
    IngressWorkScope,
};
use tokio::sync::oneshot;

use super::{wait_for_reconciliation, wait_for_reconciliation_with_deadline};

fn fixture() -> (IngressBudget, IngressWorkScope) {
    let root = IngressBudget::new(8192);
    let plan = |bytes| AllocationPlan::new("reconciliation allocation fixture", &[bytes]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(8192, plan(1024), plan(4096)).unwrap(),
    )
    .unwrap();
    let scope = IngressWorkScope::new(
        policy
            .reserve(IngressClass::Transformation, plan(4096))
            .unwrap(),
    );
    (root, scope)
}

struct Proof {
    bytes: Vec<u8>,
    root: IngressBudget,
    dropped_with_available: Arc<Mutex<Option<usize>>>,
}

impl Drop for Proof {
    fn drop(&mut self) {
        assert_eq!(self.bytes.len(), 2048);
        *self.dropped_with_available.lock().unwrap() = Some(self.root.available_bytes());
    }
}

#[tokio::test]
async fn reconciliation_inherits_the_actual_allocation_scope() {
    let (root, scope) = fixture();
    let result = scope
        .run(async {
            wait_for_reconciliation("synthetic ingress scope", async {
                IngressWorkScope::current().map(|scope| scope.requested_bytes())
            })
            .await
            .unwrap()
        })
        .await;
    assert_eq!(*result, Some(4096));
    drop(result);
    drop(scope);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[tokio::test]
async fn cancelled_caller_cannot_release_a_detached_proof_allocation() {
    let (root, scope) = fixture();
    let observation = Arc::new(Mutex::new(None));
    let (started, ready) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let (finished, done) = oneshot::channel();
    let child_root = root.clone();
    let child_observation = observation.clone();
    let caller = tokio::spawn(async move {
        scope
            .run(async move {
                let proof = Proof {
                    bytes: vec![7; 2048],
                    root: child_root,
                    dropped_with_available: child_observation,
                };
                wait_for_reconciliation("synthetic cancelled ingress", async move {
                    started.send(()).unwrap();
                    released.await.unwrap();
                    drop(proof);
                    finished.send(()).unwrap();
                })
                .await
            })
            .await
    });
    ready.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let held_after_cancel = root.available_bytes();
    release.send(()).unwrap();
    done.await.unwrap();
    // JoinHandle cancellation is not child cancellation; let the actual child
    // finish dropping its output before checking the final balance.
    tokio::task::yield_now().await;
    assert_eq!(held_after_cancel, 4096 - root.metadata_bytes());
    assert_eq!(
        *observation.lock().unwrap(),
        Some(4096 - root.metadata_bytes())
    );
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[tokio::test(start_paused = true)]
async fn timed_out_reconciliation_keeps_its_proof_charged_until_real_completion() {
    let (root, scope) = fixture();
    let observation = Arc::new(Mutex::new(None));
    let (release, released) = oneshot::channel();
    let (finished, done) = oneshot::channel();
    let result = scope
        .run(async {
            let proof = Proof {
                bytes: vec![9; 2048],
                root: root.clone(),
                dropped_with_available: observation.clone(),
            };
            wait_for_reconciliation_with_deadline(
                "synthetic timed-out ingress",
                std::time::Duration::from_secs(1),
                async move {
                    released.await.unwrap();
                    drop(proof);
                    finished.send(()).unwrap();
                },
            )
            .await
        })
        .await;
    assert!(result.is_err());
    drop(result);
    drop(scope);
    let held_after_timeout = root.available_bytes();
    release.send(()).unwrap();
    done.await.unwrap();
    tokio::task::yield_now().await;
    assert_eq!(held_after_timeout, 4096 - root.metadata_bytes());
    assert_eq!(
        *observation.lock().unwrap(),
        Some(4096 - root.metadata_bytes())
    );
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[tokio::test]
async fn detached_result_drops_before_its_allocation_scope() {
    detached_drop(false).await;
}

#[tokio::test]
async fn detached_panic_drops_its_proof_before_releasing_allocation_custody() {
    detached_drop(true).await;
}

async fn detached_drop(panic_after_cancel: bool) {
    let (root, scope) = fixture();
    let observation = Arc::new(Mutex::new(None));
    let (started, ready) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let child_root = root.clone();
    let child_observation = observation.clone();
    let caller = tokio::spawn(async move {
        scope
            .run(async move {
                let proof = Proof {
                    bytes: vec![5; 2048],
                    root: child_root,
                    dropped_with_available: child_observation,
                };
                wait_for_reconciliation("synthetic detached output", async move {
                    started.send(()).unwrap();
                    released.await.unwrap();
                    assert!(!panic_after_cancel, "synthetic detached proof panic");
                    proof
                })
                .await
            })
            .await
    });
    ready.await.unwrap();
    caller.abort();
    assert!(matches!(caller.await, Err(error) if error.is_cancelled()));
    let held_after_cancel = root.available_bytes();
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while observation.lock().unwrap().is_none()
            || root.available_bytes() != root.capacity_bytes() - root.metadata_bytes()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("detached task did not drop its actual proof and scope");
    assert_eq!(held_after_cancel, 4096 - root.metadata_bytes());
    assert_eq!(
        *observation.lock().unwrap(),
        Some(4096 - root.metadata_bytes())
    );
}

#[tokio::test]
async fn unrelated_reconciliation_has_no_implicit_allocation_policy() {
    assert!(
        wait_for_reconciliation("synthetic legacy reconciliation", async {
            IngressWorkScope::current().is_none()
        })
        .await
        .unwrap()
    );
}

#[tokio::test]
async fn completed_reconciliation_keeps_payload_and_error_custody_in_the_receiver() {
    for failure in [false, true] {
        let (root, scope) = fixture();
        let observation = Arc::new(Mutex::new(None));
        let result = scope
            .run(async {
                let proof = Proof {
                    bytes: vec![17; 2048],
                    root: root.clone(),
                    dropped_with_available: observation.clone(),
                };
                wait_for_reconciliation("synthetic returned proof", async move {
                    if failure {
                        Err(proof)
                    } else {
                        Ok(proof)
                    }
                })
                .await
                .unwrap()
            })
            .await;
        drop(scope);
        assert_eq!(root.available_bytes(), 4096 - root.metadata_bytes());
        assert_eq!(result.is_err(), failure);
        let proof = match &*result {
            Ok(proof) | Err(proof) => proof,
        };
        assert!(proof.bytes.iter().all(|byte| *byte == 17));
        assert!(observation.lock().unwrap().is_none());
        drop(result);
        assert_eq!(
            *observation.lock().unwrap(),
            Some(4096 - root.metadata_bytes())
        );
        assert_eq!(
            root.available_bytes(),
            root.capacity_bytes() - root.metadata_bytes()
        );
    }
}

#[test]
fn full_reconciliation_slots_refuse_before_polling_or_detaching_proof() {
    const TEST: &str = "store::sandbox_mongo::ingress_reconciliation_tests::full_reconciliation_slots_refuse_before_polling_or_detaching_proof";
    const CHILD: &str = "CMA_INGRESS_RECONCILIATION_SLOT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        // Slot pressure is process-wide. Isolate it so ordinary parallel unit
        // tests cannot accidentally become the pressure fixture's callers.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let occupied = super::RECONCILIATION_SLOTS
                .try_acquire_many(super::MAX_PENDING_RECONCILIATIONS as u32)
                .unwrap();
            let (root, scope) = fixture();
            let observation = Arc::new(Mutex::new(None));
            let polled = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let output = scope
                .run(async {
                    let proof = Proof {
                        bytes: vec![3; 2048],
                        root: root.clone(),
                        dropped_with_available: observation.clone(),
                    };
                    let child_polled = polled.clone();
                    wait_for_reconciliation("synthetic slot refusal", async move {
                        child_polled.store(true, std::sync::atomic::Ordering::SeqCst);
                        proof
                    })
                    .await
                })
                .await;
            let Err(message) = &*output else {
                panic!("full slots must refuse admission")
            };
            assert!(message.contains("prior reconciliations remain pending"));
            assert!(!polled.load(std::sync::atomic::Ordering::SeqCst));
            assert_eq!(
                *observation.lock().unwrap(),
                Some(4096 - root.metadata_bytes())
            );
            drop(output);
            drop(scope);
            assert_eq!(
                root.available_bytes(),
                root.capacity_bytes() - root.metadata_bytes()
            );
            assert_eq!(super::RECONCILIATION_SLOTS.available_permits(), 0);
            drop(occupied);
            assert_eq!(
                super::RECONCILIATION_SLOTS.available_permits(),
                super::MAX_PENDING_RECONCILIATIONS
            );
        });
}
