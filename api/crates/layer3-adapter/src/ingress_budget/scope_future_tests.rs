use super::*;
use crate::ingress_budget::{
    AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
};
use std::sync::atomic::{AtomicBool, Ordering};

struct Payload {
    bytes: Vec<u8>,
    root: IngressBudget,
    charged_at_drop: Arc<AtomicBool>,
}

impl Drop for Payload {
    fn drop(&mut self) {
        self.charged_at_drop
            .store(self.root.reserved_work_bytes() == 4096, Ordering::SeqCst);
    }
}

fn fixture() -> (IngressBudget, Arc<AtomicBool>, OwnedWork<Payload>) {
    let root = IngressBudget::new(16 * 1024);
    let plan = AllocationPlan::checked("owned async input", &[4096]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan, plan).unwrap(),
    )
    .unwrap();
    let scope = IngressWorkScope::new(policy.admit(IngressClass::Data, plan).unwrap());
    let charged_at_drop = Arc::new(AtomicBool::new(false));
    let payload = scope.initialize_owned(|| Payload {
        bytes: vec![7; 2048],
        root: root.clone(),
        charged_at_drop: charged_at_drop.clone(),
    });
    (root, charged_at_drop, payload)
}

#[test]
fn dropping_an_unpolled_transformation_drops_input_before_custody() {
    let (root, charged_at_drop, input) = fixture();
    let operation = input
        .map(|input| async move {
            std::future::pending::<()>().await;
            input
        })
        .resolve();
    drop(operation);
    assert!(charged_at_drop.load(Ordering::SeqCst));
    root.assert_idle();
}

#[tokio::test]
async fn cancelling_a_suspended_transformation_drops_input_before_custody() {
    let (root, charged_at_drop, input) = fixture();
    let (started, ready) = tokio::sync::oneshot::channel();
    let operation = input
        .map(|input| async move {
            started.send(()).unwrap();
            std::future::pending::<()>().await;
            input
        })
        .resolve();
    let task = tokio::spawn(operation);
    ready.await.unwrap();
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    assert!(charged_at_drop.load(Ordering::SeqCst));
    root.assert_idle();
}

#[tokio::test]
async fn completed_success_and_error_keep_the_input_owner_through_result_drop() {
    for success in [true, false] {
        let (root, charged_at_drop, input) = fixture();
        let task = tokio::spawn(
            input
                .map(|input| async move {
                    if success {
                        Ok(input)
                    } else {
                        Err(input)
                    }
                })
                .resolve(),
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !task.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(root.available_bytes(), 12 * 1024 - root.metadata_bytes());
        let outcome = task.await.unwrap().transpose();
        let value = match outcome {
            Ok(value) => {
                assert!(success);
                value
            }
            Err(value) => {
                assert!(!success);
                value
            }
        };
        assert_eq!(value.bytes.len(), 2048);
        assert_eq!(root.available_bytes(), 12 * 1024 - root.metadata_bytes());
        drop(value);
        assert!(charged_at_drop.load(Ordering::SeqCst));
        root.assert_idle();
    }
}

#[tokio::test]
async fn panicking_async_transformations_release_input_before_custody() {
    let (root, charged_at_drop, input) = fixture();
    let task = tokio::spawn(
        input
            .map(|input| async move {
                let _input = input;
                panic!("synthetic async input failure");
            })
            .resolve(),
    );
    assert!(task.await.unwrap_err().is_panic());
    assert!(charged_at_drop.load(Ordering::SeqCst));
    root.assert_idle();
}
