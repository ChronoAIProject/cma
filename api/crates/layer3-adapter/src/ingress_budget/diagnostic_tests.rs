use super::*;
use crate::ingress_budget::{sizing, ClassBounds, IngressBudget};
use futures_util::FutureExt;

pub(super) fn fixture() -> (IngressBudget, IngressOwnershipPolicy, IngressWorkScope) {
    let root = IngressBudget::new(256 * 1024);
    let floor = AllocationPlan::checked("diagnostic fixture", &[16 * 1024]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    let scope = IngressWorkScope::new(
        policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked("diagnostic scope", &[sizing::shared_owner_bytes::<()>()])
                    .unwrap(),
            )
            .unwrap(),
    );
    (root, policy, scope)
}

#[tokio::test]
async fn diagnostic_result_clones_retain_exact_formatted_storage() {
    let (root, policy, scope) = fixture();
    let payload = "x".repeat(4096);
    let result = scope
        .initialize_owned(|| async {
            Err::<(), _>(make(
                AdapterError::OperationIdentityConflict,
                format_args!("{}: {}", "identity", payload),
            ))
        })
        .resolve_admitted_result(&policy, IngressClass::Data)
        .await
        .unwrap_err();
    assert!(
        matches!(result.as_adapter(), Some(AdapterError::OperationIdentityConflict(value)) if value.len()==4106)
    );
    let charged = root.capacity_bytes() - root.available_bytes();
    assert_eq!(result.requested_bytes(), charged);
    let clone = result.clone();
    drop((scope, result));
    assert_eq!(root.capacity_bytes() - root.available_bytes(), charged);
    drop(clone);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn cancellation_drops_formatted_work_before_its_scope() {
    let (root, policy, scope) = fixture();
    let work = scope.initialize_owned(|| async {
        let _error = make(
            AdapterError::Unavailable,
            format_args!("{}", "retained failure"),
        );
        std::future::pending::<crate::Result<()>>().await
    });
    let mut operation = Box::pin(work.resolve_admitted_result(&policy, IngressClass::Data));
    assert!(operation.as_mut().now_or_never().is_none());
    drop(scope);
    assert!(root.available_bytes() < root.capacity_bytes());
    drop(operation);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn diagnostic_growth_refusal_uses_the_pre_admitted_error_owner_without_text() {
    let (root, policy, scope) = fixture();
    let entered = std::sync::atomic::AtomicBool::new(false);
    let result = scope
        .initialize_owned(|| async {
            let available = policy.budget(IngressClass::Data).available_bytes();
            let _pressure = policy
                .admit(
                    IngressClass::Data,
                    AllocationPlan::checked("diagnostic pressure", &[available]).unwrap(),
                )
                .unwrap()
                .initialize(|| vec![0u8; available - 1024]);
            entered.store(true, std::sync::atomic::Ordering::Release);
            Err::<(), _>(make(
                AdapterError::Rejected,
                format_args!("a refused diagnostic"),
            ))
        })
        .resolve_admitted_result(&policy, IngressClass::Data)
        .await
        .unwrap_err();
    assert!(entered.load(std::sync::atomic::Ordering::Acquire));
    assert!(
        matches!(result.as_adapter(), Some(AdapterError::AtCapacity(detail)) if detail.is_empty())
    );
    assert!(result.requested_bytes() > 0);
    drop((scope, result));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn legacy_error_formatting_keeps_exact_category_and_text() {
    assert_eq!(
        make(AdapterError::Rejected, format_args!("{}: {}", "method", 42)),
        AdapterError::Rejected("method: 42".into())
    );
}

#[tokio::test]
async fn empty_runtime_errors_keep_their_original_category() {
    for category in [AdapterError::Rejected, AdapterError::AtCapacity] {
        let (root, policy, scope) = fixture();
        let error = scope
            .initialize_owned(|| async { Err::<(), _>(category(String::new())) })
            .resolve_admitted_result(&policy, IngressClass::Data)
            .await
            .unwrap_err();
        assert_eq!(error.as_adapter(), Some(&category(String::new())));
        drop((error, scope));
        assert_eq!(root.available_bytes(), root.capacity_bytes());
    }
}

#[tokio::test]
async fn refusing_the_error_owner_precedes_operation_polling() {
    let (root, policy, scope) = fixture();
    let available = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("owner pressure", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 1024]);
    let result = scope
        .initialize_owned(|| {
            std::future::poll_fn::<crate::Result<()>, _>(|_| {
                panic!("operation polled after error owner refusal")
            })
        })
        .resolve_admitted_result(&policy, IngressClass::Data)
        .await;
    assert!(matches!(
        result,
        Err(AdmittedError::Refused(AdmissionRefusal::AtCapacity { .. }))
    ));
    drop((scope, pressure));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
