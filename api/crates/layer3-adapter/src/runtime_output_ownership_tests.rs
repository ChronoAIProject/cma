use super::*;
use crate::ingress_budget::{
    AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
};
use crate::{ConversationUpdate, RuntimeOutputUpdate};

#[tokio::test]
async fn shared_delivery_preserves_receipt_and_effect_through_recorder_retries() {
    let root = IngressBudget::new(8192);
    let plan = |n| AllocationPlan::new("runtime delivery", &[n]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(8192, plan(1024), plan(2048)).unwrap(),
    )
    .unwrap();
    let (acks, mut received) = mpsc::channel(1);
    let owned = policy
        .reserve(IngressClass::Data, plan(4096))
        .unwrap()
        .initialize(|| ConversationUpdate::RuntimeOutput {
            receipt: RuntimeOutputReceipt::new(
                RuntimeOutputSource {
                    stream_id: "synthetic-stream".into(),
                    cursor: 7,
                    native_conversation_id: "synthetic-conversation".into(),
                    payload_sha256: "synthetic-digest".into(),
                },
                acks,
            ),
            update: Some(RuntimeOutputUpdate::new(
                ConversationUpdate::Failed {
                    message: "synthetic failure".into(),
                    capability: None,
                },
                true,
            )),
        });
    let update = ConversationUpdate::RetainedEffect(RuntimeOutputUpdate::from_admitted(owned));
    let retries = [update.clone(), update.clone()];
    let original = update
        .delivery()
        .receipt
        .unwrap()
        .source
        .native_conversation_id
        .as_ptr();
    drop(update);
    for retry in &retries {
        let view = retry.delivery();
        assert_eq!(
            view.receipt.unwrap().source.native_conversation_id.as_ptr(),
            original
        );
        assert!(matches!(
            view.effect,
            Some(ConversationUpdate::Failed { .. })
        ));
    }
    assert_eq!(root.available_bytes(), 4096);
    retries[0]
        .delivery()
        .receipt
        .unwrap()
        .acknowledge()
        .await
        .unwrap();
    assert_eq!(received.recv().await.unwrap().cursor, 7);
    drop(retries);
    assert_eq!(root.available_bytes(), 8192);
}

#[test]
fn persisted_source_shape_stays_exact_and_strict() {
    let fixture =
        r#"{"stream_id":"s","cursor":1,"native_conversation_id":"c","payload_sha256":"h"}"#;
    let source: RuntimeOutputSource = serde_json::from_str(fixture).unwrap();
    assert_eq!(serde_json::to_string(&source).unwrap(), fixture);
    assert!(serde_json::from_str::<RuntimeOutputSource>(
        &fixture.replace("\"cursor\":1", "\"cursor\":1,\"reservation\":5")
    )
    .is_err());
}
