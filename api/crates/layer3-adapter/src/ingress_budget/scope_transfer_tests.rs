use super::*;
use crate::ingress_budget::{
    AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
};
use std::sync::atomic::{AtomicBool, Ordering};

fn fixture() -> (IngressBudget, IngressWorkScope, IngressWorkScope) {
    let root = IngressBudget::new(64 * 1024);
    let plan = |bytes| AllocationPlan::checked("scope transfer", &[bytes]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan(4096), plan(16 * 1024)).unwrap(),
    )
    .unwrap();
    let scope = || IngressWorkScope::new(policy.admit(IngressClass::Data, plan(8192)).unwrap());
    (root, scope(), scope())
}

#[tokio::test]
async fn completed_child_result_transfers_only_into_its_exact_receiving_scope() {
    let (root, scope, sibling) = fixture();
    drop(sibling);
    let output = scope
        .run(async {
            let child = tokio::spawn(IngressWorkScope::inherit_owned(async { vec![19u8; 4096] }));
            let output = child.await.unwrap();
            let pointer = output.as_ptr();
            let transferred = output.into_current().unwrap();
            assert_eq!(transferred.as_ptr(), pointer);
            assert_eq!(root.reserved_work_bytes(), 8192);
            transferred
        })
        .await;
    drop(scope);
    assert_eq!(output[4095], 19);
    assert_eq!(root.reserved_work_bytes(), 8192);
    drop(output);
    root.assert_idle();
}

#[tokio::test]
async fn transfer_mismatch_preserves_the_original_value_and_custody() {
    let (root, scope, sibling) = fixture();
    let output = scope.run(async { vec![23u8; 4096] }).await;
    let pointer = output.as_ptr();
    let output = output.into_current().unwrap_err();
    let rejected = sibling
        .run(async { output.into_current().unwrap_err() })
        .await;
    assert_eq!(rejected.as_ptr(), pointer);
    drop(sibling);
    let output = scope
        .run(async {
            // The outer envelope belongs to the sibling; recovering the inner
            // result must not silently accept root equality as scope equality.
            assert!(rejected.into_current().is_err());
            IngressWorkScope::inherit_owned(async { vec![7u8; 4096] })
                .await
                .into_current()
                .unwrap()
        })
        .await;
    drop(scope);
    assert_eq!(root.reserved_work_bytes(), 8192);
    assert_eq!(output[4095], 7);
    drop(output);
    root.assert_idle();
}

#[tokio::test]
async fn unowned_transfer_requires_both_sides_to_be_disabled() {
    let unowned = IngressWorkScope::inherit_owned(async { String::from("legacy") }).await;
    assert_eq!(unowned.into_current().unwrap(), "legacy");
    let (root, scope, sibling) = fixture();
    drop(sibling);
    let unowned = IngressWorkScope::inherit_owned(async { vec![11u8; 64] }).await;
    let pointer = unowned.as_ptr();
    let rejected = scope
        .run(async { unowned.into_current().unwrap_err() })
        .await;
    assert_eq!(rejected.as_ptr(), pointer);
    drop((rejected, scope));
    root.assert_idle();
}

#[tokio::test]
async fn heap_error_transfer_to_a_scope_clone_allocates_nothing_and_keeps_custody() {
    let (root, scope, sibling) = fixture();
    drop(sibling);
    let mut probe = crate::allocation_probe::AllocationProbe::new();
    let output = probe.track(|| scope.initialize_owned(|| Err::<(), String>("e".repeat(4096))));
    let original = output.as_ref().unwrap_err().as_ptr();
    let allocated = probe.allocated();
    let receiving = scope.clone();
    let returned = receiving
        .run(async {
            let result = probe.track(|| output.into_current().unwrap());
            assert_eq!(result.as_ref().unwrap_err().as_ptr(), original);
            assert_eq!(probe.allocated(), allocated);
            assert_eq!(root.reserved_work_bytes(), 8192);
            result
        })
        .await;
    drop((scope, receiving));
    assert_eq!(probe.live(), 4096);
    assert!(probe.live() <= root.reserved_work_bytes());
    assert_eq!(returned.as_ref().unwrap_err().len(), 4096);
    drop(returned);
    assert_eq!(probe.live(), 0);
    root.assert_idle();
}

struct NonClonePayload {
    bytes: Vec<u8>,
    root: IngressBudget,
    dropped: Arc<AtomicBool>,
}

impl std::fmt::Debug for NonClonePayload {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("NonClonePayload")
            .field("bytes", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

impl Drop for NonClonePayload {
    fn drop(&mut self) {
        assert!(self.root.reserved_work_bytes() >= self.bytes.capacity());
        self.dropped.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn nested_foreign_scope_rejection_preserves_a_non_clone_payload_and_its_owner() {
    let (root, scope, sibling) = fixture();
    drop(sibling);
    let (foreign_root, foreign, sibling) = fixture();
    drop(sibling);
    let dropped = Arc::new(AtomicBool::new(false));
    let output = scope.initialize_owned(|| NonClonePayload {
        bytes: vec![31u8; 4096],
        root: root.clone(),
        dropped: dropped.clone(),
    });
    let original = output.bytes.as_ptr();
    let rejected = scope
        .run(async {
            foreign
                .run(async {
                    let rejected = output.into_current().unwrap_err();
                    assert_eq!(rejected.bytes.as_ptr(), original);
                    assert!(!dropped.load(Ordering::SeqCst));
                    rejected
                })
                .await
        })
        .await;
    drop((scope, foreign));
    assert_eq!(rejected.bytes[4095], 31);
    assert!(!dropped.load(Ordering::SeqCst));
    assert_eq!(root.reserved_work_bytes(), 8192);
    assert_eq!(foreign_root.reserved_work_bytes(), 8192);
    drop(rejected);
    assert!(dropped.load(Ordering::SeqCst));
    root.assert_idle();
    foreign_root.assert_idle();
}

#[tokio::test]
async fn cancellation_drops_transferred_payload_before_its_receiving_scope() {
    let (root, scope, sibling) = fixture();
    drop(sibling);
    let dropped = Arc::new(AtomicBool::new(false));
    let output = scope.initialize_owned(|| NonClonePayload {
        bytes: vec![37u8; 4096],
        root: root.clone(),
        dropped: dropped.clone(),
    });
    let (started, ready) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        scope
            .run(async move {
                let transferred = output.into_current().unwrap();
                started.send(()).unwrap();
                std::future::pending::<()>().await;
                drop(transferred);
            })
            .await
    });
    ready.await.unwrap();
    assert!(!dropped.load(Ordering::SeqCst));
    assert_eq!(root.reserved_work_bytes(), 8192);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(dropped.load(Ordering::SeqCst));
    root.assert_idle();
}
