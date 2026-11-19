use super::*;
use crate::ingress_budget::{
    AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
};
use crate::{ConversationUpdate, RuntimeOutputUpdate};

#[test]
fn isolated_ack_queue_deallocation_contract() {
    use crate::allocation_probe::{deallocation, AllocationProbe};
    if deallocation::isolated(
        "runtime_output::runtime_output_ownership_tests::isolated_ack_queue_deallocation_contract",
    ) {
        return;
    }
    let root = IngressBudget::new(128 * 1024);
    let plan = |n| AllocationPlan::checked("ACK teardown fixture", &[n]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan(32768), plan(8192)).unwrap(),
    )
    .unwrap();
    let mut probe = AllocationProbe::new();
    for sender_last in [false, true] {
        let ((sender, receiver), pause) = probe
            .pause_deallocation(deallocation::arc_allocation_bytes::<AckQueue>(), || {
                RuntimeOutputReceipt::admitted_channel(policy.clone(), 2).unwrap()
            });
        let sender_clone = sender.clone();
        drop(sender);
        assert!(pause.admission_blocked_while_dropping(
            &policy,
            IngressClass::Control,
            move || {
                if sender_last {
                    drop(receiver);
                    drop(sender_clone);
                } else {
                    drop(sender_clone);
                    drop(receiver);
                }
            }
        ));
        assert_eq!(root.available_bytes(), root.capacity_bytes());
        assert_eq!(probe.live(), 0);
    }
}

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

fn ack_policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(65536);
    let plan = |n| AllocationPlan::new("ack fixture", &[n]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(65536, plan(32768), plan(8192)).unwrap(),
    )
    .unwrap();
    (root, policy)
}

fn source() -> RuntimeOutputSource {
    RuntimeOutputSource {
        stream_id: "s".repeat(36),
        cursor: 7,
        native_conversation_id: "c".repeat(1024),
        payload_sha256: "a".repeat(64),
    }
}

#[tokio::test]
async fn admitted_ack_progresses_at_data_ceiling_and_owns_its_copy_until_sink_drop() {
    let (root, policy) = ack_policy();
    let (sender, mut receiver) = RuntimeOutputReceipt::admitted_channel(policy.clone(), 2).unwrap();
    let data = policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("data saturation", &[policy.bounds().data_bytes()]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![5u8; 24000]);
    let receipt = RuntimeOutputReceipt::new_admitted(source(), sender);
    receipt.acknowledge().await.unwrap();
    let ack = receiver.recv().await.unwrap();
    assert_eq!(ack.cursor, 7);
    assert_ne!(
        ack.native_conversation_id.as_ptr(),
        receipt.source.native_conversation_id.as_ptr()
    );
    drop((data, receipt, receiver));
    assert_eq!(root.available_bytes(), 65536 - ack.requested_bytes());
    let final_sink = ack.clone();
    drop(ack);
    assert_eq!(final_sink.native_conversation_id.len(), 1024);
    drop(final_sink);
    assert_eq!(root.available_bytes(), 65536);
}

#[test]
fn invalid_and_saturated_ack_refuse_before_cloning_or_retaining_queue_slot() {
    let (root, policy) = ack_policy();
    let (sender, receiver) = RuntimeOutputReceipt::admitted_channel(policy.clone(), 1).unwrap();
    let initial = root.available_bytes();
    let mut oversized = source();
    oversized.native_conversation_id = "x".repeat(4 * 1024 * 1024);
    // Initialize process-lived synchronization/tracing before attributing the
    // per-attempt copy. The queue fixture separately owns that infrastructure.
    assert!(sender.send(&oversized).is_err());
    let mut probe = crate::allocation_probe::AllocationProbe::new();
    let error = probe.track(|| sender.send(&oversized));
    assert!(matches!(error, Err(crate::AdapterError::Rejected(_))));
    assert!(
        probe.peak() < 1024,
        "refusal copied an identity: {}",
        probe.peak()
    );
    drop(error);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), initial);
    drop(probe);
    let available = policy.budget(IngressClass::Control).available_bytes();
    let pressure = policy
        .reserve(
            IngressClass::Control,
            AllocationPlan::new("saturated control fixture", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 512]);
    let valid = source();
    assert!(matches!(
        sender.send(&valid),
        Err(crate::AdapterError::AtCapacity(_))
    ));
    let mut probe = crate::allocation_probe::AllocationProbe::new();
    let error = probe.track(|| sender.send(&valid));
    assert!(matches!(error, Err(crate::AdapterError::AtCapacity(_))));
    assert!(probe.peak() < 1024, "control pressure cloned the identity");
    drop(error);
    assert_eq!(probe.live(), 0);
    drop(probe);
    drop(pressure);
    sender.send(&source()).unwrap();
    assert!(sender.send(&source()).is_err());
    drop(receiver);
    assert!(sender.send(&source()).is_err());
    drop(sender);
    assert_eq!(root.available_bytes(), 65536);
}

#[test]
fn ack_queue_capacity_is_charged_until_last_handle_even_after_receiver_closes() {
    let (root, policy) = ack_policy();
    assert!(RuntimeOutputReceipt::admitted_channel(policy.clone(), 0).is_err());
    let (sender, receiver) = RuntimeOutputReceipt::admitted_channel(policy, 1).unwrap();
    let clone = sender.clone();
    let reserved = 65536 - root.available_bytes();
    assert!(reserved > 0);
    drop((sender, receiver));
    assert_eq!(65536 - root.available_bytes(), reserved);
    drop(clone);
    assert_eq!(root.available_bytes(), 65536);
}
