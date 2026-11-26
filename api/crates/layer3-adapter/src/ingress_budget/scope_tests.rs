use super::*;
use crate::ingress_budget::{
    AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
};

fn fixture() -> (IngressBudget, IngressWorkScope) {
    let plan = |n| AllocationPlan::new("detached work", &[n]).unwrap();
    let root = IngressBudget::new(8192);
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

#[tokio::test]
async fn detached_result_remains_charged_after_parent_and_future_finish() {
    let (root, scope) = fixture();
    let (finished, done) = tokio::sync::oneshot::channel();
    let (handle_tx, handle_rx) = tokio::sync::oneshot::channel();
    let parent_result = scope
        .run(async {
            let handle = tokio::spawn(IngressWorkScope::inherit_owned(async move {
                let value = vec![17u8; 2048];
                finished.send(()).unwrap();
                value
            }));
            handle_tx.send(handle).unwrap();
        })
        .await;
    let result = handle_rx.await.unwrap();
    drop(parent_result);
    done.await.unwrap();
    drop(scope);
    while !result.is_finished() {
        tokio::task::yield_now().await;
    }
    assert_eq!(root.available_bytes(), 4096 - root.metadata_bytes());
    // Dropping the completed task's handle drops its still-owned output.
    drop(result);
    assert_eq!(root.available_bytes(), 8192 - root.metadata_bytes());
}

#[tokio::test]
async fn caller_cancellation_does_not_release_detached_work() {
    let (root, scope) = fixture();
    let (started, ready) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let (handle_tx, handle_rx) = tokio::sync::oneshot::channel();
    let parent = tokio::spawn(async move {
        scope
            .run(async move {
                let child = tokio::spawn(IngressWorkScope::inherit_owned(async move {
                    let value = vec![23u8; 2048];
                    started.send(()).unwrap();
                    released.await.unwrap();
                    value
                }));
                handle_tx.send(child).unwrap();
                std::future::pending::<()>().await;
            })
            .await
    });
    let child = handle_rx.await.unwrap();
    ready.await.unwrap();
    parent.abort();
    assert!(parent.await.unwrap_err().is_cancelled());
    assert_eq!(root.available_bytes(), 4096 - root.metadata_bytes());
    release.send(()).unwrap();
    let output = child.await.unwrap();
    assert!(output.is_admitted());
    assert_eq!(output[2047], 23);
    assert_eq!(root.available_bytes(), 4096 - root.metadata_bytes());
    drop(output);
    assert_eq!(root.available_bytes(), 8192 - root.metadata_bytes());
}

#[tokio::test]
async fn abort_and_panic_release_custody_after_owned_storage() {
    let (root, scope) = fixture();
    let (started, ready) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        scope
            .run(async move {
                let _value = vec![31u8; 2048];
                started.send(()).unwrap();
                std::future::pending::<()>().await;
            })
            .await
    });
    ready.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(root.available_bytes(), 8192 - root.metadata_bytes());
    let (root, scope) = fixture();
    let task = tokio::spawn(async move {
        scope
            .run(async {
                let _value = vec![1u8; 2048];
                panic!("worker failed")
            })
            .await
    });
    assert!(task.await.unwrap_err().is_panic());
    assert_eq!(root.available_bytes(), 8192 - root.metadata_bytes());
}

#[tokio::test]
async fn disabled_inheritance_keeps_unrelated_work_unaccounted() {
    let result = IngressWorkScope::inherit_owned(async { 42 }).await;
    assert!(!result.is_admitted());
    assert_eq!(*result, 42);
}

#[test]
fn scope_and_result_custody_domains_are_exact_and_allocation_free() {
    let make_policy = || {
        let root = IngressBudget::new(64 * 1024);
        let floor = AllocationPlan::checked("domain floor", &[4096]).unwrap();
        let bounds = ClassBounds::new(root.capacity_bytes(), floor, floor)
            .unwrap()
            .with_attachment_limit(2)
            .unwrap();
        let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
        (root, policy)
    };
    let (root, process) = make_policy();
    let (foreign_root, foreign) = make_policy();
    let first = process.admit_attachment().unwrap();
    let second = process.admit_attachment().unwrap();
    let policies = [
        process.clone(),
        first.policy().clone(),
        second.policy().clone(),
        foreign,
    ];
    let scopes = policies.each_ref().map(|policy| {
        IngressWorkScope::new(
            policy
                .admit(
                    IngressClass::Data,
                    AllocationPlan::checked("domain scope", &[2048]).unwrap(),
                )
                .unwrap(),
        )
    });
    let results = scopes
        .each_ref()
        .map(|scope| scope.initialize_owned(|| None::<usize>));
    let available = root.available_bytes();
    let mut probe = crate::allocation_probe::AllocationProbe::new();
    probe.track(|| {
        for (source, (scope, result)) in scopes.iter().zip(&results).enumerate() {
            for (target, policy) in policies.iter().enumerate() {
                assert_eq!(scope.shares_custody_domain(policy), source == target);
                assert_eq!(result.shares_custody_domain(policy), source == target);
                let directional = source == target || (target == 0 && matches!(source, 1 | 2));
                assert_eq!(scope.belongs_to(policy), directional);
                assert_eq!(result.belongs_to(policy), directional);
            }
        }
    });
    assert_eq!(probe.allocated(), 0);
    assert_eq!(root.available_bytes(), available);
    drop((results, scopes, policies, first, second));
    root.assert_idle();
    foreign_root.assert_idle();
}

#[test]
fn result_custody_checks_envelope_without_certifying_nested_owners() {
    let root = IngressBudget::new(64 * 1024);
    let plan = AllocationPlan::checked("nested domain", &[2048]).unwrap();
    let bounds = ClassBounds::new(root.capacity_bytes(), plan, plan)
        .unwrap()
        .with_attachment_limit(1)
        .unwrap();
    let process = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    let attachment = process.admit_attachment().unwrap();
    let attached_policy = attachment.policy().clone();
    let scope = IngressWorkScope::new(process.admit(IngressClass::Data, plan).unwrap());
    let nested = attached_policy
        .admit(IngressClass::Data, plan)
        .unwrap()
        .initialize(|| 7);
    let envelope = scope.initialize_owned(|| nested);
    assert!(envelope.shares_custody_domain(&process));
    assert!(!envelope.shares_custody_domain(&attached_policy));
    assert!(!(*envelope).shares_custody_domain(&process));
    assert!((*envelope).shares_custody_domain(&attached_policy));
    let error = envelope.map(Err::<(), _>).transpose().unwrap_err();
    assert!(error.shares_custody_domain(&process));
    assert!((*error).shares_custody_domain(&attached_policy));
    drop((scope, attachment, attached_policy));
    assert!(process.admit_attachment().is_err());
    drop(error);
    drop(process.admit_attachment().unwrap());
    root.assert_idle();
}

#[test]
fn unowned_results_never_match_a_custody_domain() {
    use futures_util::FutureExt;
    let root = IngressBudget::new(64 * 1024);
    let floor = AllocationPlan::checked("unowned domain", &[4096]).unwrap();
    let bounds = ClassBounds::new(root.capacity_bytes(), floor, floor)
        .unwrap()
        .with_attachment_limit(1)
        .unwrap();
    let process = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    let attachment = process.admit_attachment().unwrap();
    let value = IngressWorkScope::inherit_owned(async { None::<usize> })
        .now_or_never()
        .unwrap();
    let mut probe = crate::allocation_probe::AllocationProbe::new();
    probe.track(|| {
        assert!(!value.shares_custody_domain(&process));
        assert!(!value.shares_custody_domain(attachment.policy()));
        assert!(!value.belongs_to(&process));
        assert!(!value.is_admitted());
    });
    assert_eq!(probe.allocated(), 0);
    drop((value, attachment));
    root.assert_idle();
}

#[tokio::test]
async fn result_transfer_releases_work_only_after_retained_admission() {
    let plan = |n| AllocationPlan::new("result transfer", &[n]).unwrap();
    let root = IngressBudget::new(16384);
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(16384, plan(1024), plan(4096)).unwrap(),
    )
    .unwrap();
    let work = policy.budget(IngressClass::Transformation);
    let scope = IngressWorkScope::new(
        policy
            .reserve(IngressClass::Transformation, plan(4096))
            .unwrap(),
    );
    let result = scope.run(async { vec![19u8; 2048] }).await;
    drop(scope);
    assert_eq!(work.available_bytes(), 0);
    let retained = policy.reserve(IngressClass::Data, plan(4096)).unwrap();
    let result = result.retain(retained).unwrap().into_shared().unwrap();
    assert_eq!(work.available_bytes(), 4096);
    assert_eq!(root.available_bytes(), 12288 - root.metadata_bytes());
    assert_eq!(result[2047], 19);
    let (_, other) = fixture();
    let foreign = other.run(async { 42 }).await;
    assert!(!foreign.belongs_to(&policy));
    drop(result);
    assert_eq!(root.available_bytes(), 16384 - root.metadata_bytes());
}

#[test]
fn admitted_retention_rejects_foreign_storage_without_allocating_a_diagnostic() {
    let plan = |n| AllocationPlan::checked("foreign retention", &[n]).unwrap();
    let make_policy = || {
        let root = IngressBudget::new(16384);
        let policy = IngressOwnershipPolicy::new(
            root.clone(),
            ClassBounds::new(16384, plan(1024), plan(4096)).unwrap(),
        )
        .unwrap();
        // The probe isolates diagnostic allocation from the policy's lazy
        // semaphore backing, which lives with the fixture's root.
        drop(policy.admit(IngressClass::Data, plan(1)).unwrap());
        (root, policy)
    };
    let (root, policy) = make_policy();
    let scope = IngressWorkScope::new(policy.admit(IngressClass::Data, plan(2048)).unwrap());
    let result = scope.initialize_owned(|| vec![19u8; 1024]);
    drop(scope);
    let (foreign, policy) = make_policy();
    let storage = policy.admit(IngressClass::Data, plan(2048)).unwrap();
    let mut probe = crate::allocation_probe::AllocationProbe::new();
    let error = probe.track(|| result.retain_admitted(storage).unwrap_err());
    assert_eq!(
        error,
        crate::ingress_budget::AdmissionRefusal::InvalidPlan {
            phase: "result retention ownership"
        }
    );
    assert_eq!(probe.allocated(), 0);
    root.assert_idle();
    foreign.assert_idle();
}
