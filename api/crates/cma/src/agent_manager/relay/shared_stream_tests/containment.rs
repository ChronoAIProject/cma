//! Real Mongo renewal faults beside the production public HTTP projections.
use super::*;
use crate::store::agent_ownership::mongo_renewal_faults::Fault;
use layer2_core::{ids::AgentCommandId, RuntimeCapabilities, RuntimeCapability};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

#[path = "containment_controls.rs"]
mod controls;
pub(super) use controls::controls_router;

#[tokio::test]
#[ignore = "requires disposable failpoint-enabled Mongo replica set"]
async fn isolated_renewal_preserves_public_reads_streams_and_controls() {
    for fault in [
        Fault::KnownAbort,
        Fault::UnknownCommit,
        Fault::WithheldReply,
    ] {
        public_case(fault).await;
    }
}

async fn public_case(fault: Fault) {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!("cma_public_containment_{}", uuid::Uuid::new_v4().simple());
    let off = MongoSandboxStore::open(&client, &name).await.unwrap();
    let healthy = fixture::seed_named(&off, &client.database(&name), "healthy").await;
    let faulted = fixture::seed_named(&off, &client.database(&name), "isolated").await;
    assert_eq!(healthy.owner, faulted.owner);
    // Advertise exactly the fixture runtime's demonstrated control capability.
    for agent in [&healthy, &faulted] {
        let mut enabled = agent.clone();
        enabled.capabilities = RuntimeCapabilities::new([
            RuntimeCapability::Conversation,
            RuntimeCapability::Interrupt,
        ]);
        enabled.revision += 1;
        off.save_agent(agent.revision, enabled).await.unwrap();
    }
    drop(off);
    let replica = Replica::open_with_fault(&name, &healthy.owner, Some(faulted.id.clone())).await;
    let good = replica
        .coordinator
        .acquire(&healthy.id)
        .await
        .unwrap()
        .unwrap();
    let bad = replica
        .coordinator
        .acquire(&faulted.id)
        .await
        .unwrap()
        .unwrap();
    let sends = Arc::new(AtomicUsize::new(0));
    let healthy_response = controls::start(&replica, &good, sends.clone()).await;
    let faulted_response = controls::start(&replica, &bad, sends.clone()).await;
    assert_eq!(sends.load(Ordering::SeqCst), 2);
    let mut stream = replica.stream(&healthy.id, None, true).await;
    let current = replica.store.get_agent(&healthy.id).await.unwrap().unwrap();
    for sequence in 1..=current.conversation.last_seq {
        assert_eq!(
            stream.event("cma.agent.event").await.body["sequence"],
            sequence
        );
    }
    subscribed(&replica, &healthy.id, 1).await;
    publish(
        &replica,
        &good,
        current.conversation.last_seq,
        "before fault",
    )
    .await;
    assert_eq!(
        stream.event("cma.agent.live").await.body["delta"],
        "before fault"
    );

    let faults = replica.faults.as_ref().unwrap();
    faults.arm(fault).await;
    let result = replica.coordinator.maintain().await;
    faults.disarm().await;
    assert!(
        result.is_ok(),
        "one renewal stopped public serving: {fault:?}"
    );
    assert!(replica.coordinator.is_ready());
    assert!(!*replica.coordinator.failure_signal().borrow());
    assert!(!bad.is_current());
    assert!(bad.receiver_cancelled());
    assert!(good.is_current());
    assert!(Arc::ptr_eq(
        &replica.coordinator.cancelled_receivers().await[0],
        &bad
    ));

    let (status, head) = controls::call(
        &replica,
        reqwest::Method::GET,
        &format!("/api/v2/agents/{}/events/head", healthy.id),
        None,
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{head}");
    publish(
        &replica,
        &good,
        current.conversation.last_seq,
        "after fault",
    )
    .await;
    assert_eq!(
        stream.event("cma.agent.live").await.body["delta"],
        "after fault"
    );
    let next = commit(
        &replica,
        &good,
        vec![AgentLogKind::AgentMessage {
            content: "retained after fault".into(),
        }],
    )
    .await;
    assert!(replica.updates.notify_owned_head(&healthy.id, &good));
    let frame = stream.event("cma.agent.event").await;
    assert_eq!(frame.body["sequence"], next.conversation.last_seq);
    assert!(frame.body.to_string().contains("retained after fault"));
    assert!(frame.id.is_some());

    let control = controls::cancel(&replica, &healthy.id, &healthy_response).await;
    let delivered = controls::execute(&replica, &good, sends.clone()).await;
    assert_eq!(delivered, control);
    assert_eq!(sends.load(Ordering::SeqCst), 3);
    let (status, receipt) = controls::call(
        &replica,
        reqwest::Method::GET,
        &format!("/api/v2/commands/{control}"),
        None,
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{receipt}");
    assert_eq!(receipt["status"], "completed");

    // Admission may durably queue intent; no cancelled-token worker or live
    // publisher may execute it or send it to the runtime.
    controls::cancel(&replica, &faulted.id, &faulted_response).await;
    let before = sends.load(Ordering::SeqCst);
    assert!(bad
        .run(controls::execute(&replica, &bad, sends.clone()))
        .await
        .is_err());
    let updates = replica.updates.clone();
    assert!(bad
        .run(async move {
            updates.publish_live_fixture(AgentLiveFragment {
                agent_id: faulted.id,
                runtime_generation: 1,
                turn_sequence: 3,
                after_seq: 4,
                message_id: "cancelled".into(),
                kind: AgentLiveFragmentKind::Message,
                delta: "must not publish".into(),
            });
            panic!("cancelled publisher ran");
        })
        .await
        .is_err());
    assert_eq!(sends.load(Ordering::SeqCst), before);
    replica.coordinator.maintain().await.unwrap();
    assert_eq!(faults.calls.load(Ordering::SeqCst), 1);
    drop(stream);
    replica.close().await;
    client.database(&name).drop().await.unwrap();
}
