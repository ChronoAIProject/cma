use super::*;
use crate::ingress_budget::{
    AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
};

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
            assert_eq!(root.capacity_bytes() - root.available_bytes(), 8192);
            transferred
        })
        .await;
    drop(scope);
    assert_eq!(output[4095], 19);
    assert_eq!(root.capacity_bytes() - root.available_bytes(), 8192);
    drop(output);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
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
    assert_eq!(root.capacity_bytes() - root.available_bytes(), 8192);
    assert_eq!(output[4095], 7);
    drop(output);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
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
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
