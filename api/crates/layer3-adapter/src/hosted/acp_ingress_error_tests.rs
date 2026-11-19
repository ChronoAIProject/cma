use super::tests::fixture;
use crate::hosted::update_relay::UpdateRelay;
use crate::ingress_budget::{
    sizing, AdmittedConversationSendOutcome, AllocationPlan, IngressClass, IngressWorkScope,
};
use crate::{AdapterError, ConversationInput};

#[tokio::test(start_paused = true)]
async fn rpc_timeout_error_keeps_custody_after_bridge_and_operation_drop() {
    let fixture = fixture();
    let root = fixture.root.clone();
    let scope = IngressWorkScope::new(
        fixture
            .policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked(
                    "RPC timeout fixture",
                    &[sizing::shared_owner_bytes::<()>()],
                )
                .unwrap(),
            )
            .unwrap(),
    );
    let error = scope
        .initialize_owned(|| async {
            fixture
                .bridge
                .call_within(
                    "synthetic/method",
                    serde_json::Value::Null,
                    std::time::Duration::from_secs(1),
                )
                .await
        })
        .resolve_admitted_result(&fixture.policy, IngressClass::Data)
        .await
        .unwrap_err();
    assert!(
        matches!(error.as_adapter(), Some(AdapterError::Unavailable(detail))
        if detail == "synthetic/method was never answered")
    );
    assert_eq!(fixture.writes.lock().unwrap().len(), 1);
    let cloned = error.clone();
    drop((fixture, scope, error));
    assert_eq!(
        root.capacity_bytes() - root.available_bytes(),
        cloned.requested_bytes()
    );
    drop(cloned);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn admitted_writer_returns_owned_bridge_failure_without_runtime_effects() {
    use super::super::channel::{Admitted, Mode};
    use std::sync::{atomic::Ordering, Arc};
    let root;
    let outcome = {
        let fixture = fixture();
        root = fixture.root.clone();
        fixture.bridge.recovering_context.store(1, Ordering::SeqCst);
        let bridge = Arc::new(fixture.bridge);
        let (updates, _capture) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
        let sender = Admitted::input(&bridge, &updates).await.unwrap();
        let input = IngressWorkScope::new(
            fixture
                .policy
                .admit(
                    IngressClass::Control,
                    AllocationPlan::checked("writer error fixture", &[256]).unwrap(),
                )
                .unwrap(),
        )
        .initialize_owned(|| ConversationInput::Cancel);
        let result = sender.send_classified(input).await;
        tokio::task::yield_now().await;
        assert!(sender.is_closed());
        assert!(fixture.writes.lock().unwrap().is_empty());
        assert!(
            matches!(&result, AdmittedConversationSendOutcome::Ambiguous(error)
            if matches!(error.as_adapter(), Some(AdapterError::OperationIdentityConflict(detail))
                if detail == "admitted context recovery must complete before other input"))
        );
        result
    };
    let AdmittedConversationSendOutcome::Ambiguous(error) = outcome else {
        unreachable!()
    };
    let clone = error.clone();
    drop(error);
    assert_eq!(
        root.capacity_bytes() - root.available_bytes(),
        clone.requested_bytes()
    );
    drop(clone);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
