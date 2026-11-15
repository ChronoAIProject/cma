use super::{
    replica_test_fixture::*,
    tool_reply_store::{Admission, ReplyStore},
    *,
};
use crate::delivery::intents::DeliveryOutcome;
use std::sync::atomic::Ordering;

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn tool_receipt_and_close_reject_captured_authority_after_takeover() {
    let mut h = Harness::new().await;
    let (owner, lease) = h.accepted().await;
    let stale = ReplyStore(h.nodes[owner].journal(), lease.partition.clone());
    let Admission::Claimed(claim) = stale
        .admit_keyed(&h.plan, h.target(), "a".repeat(64), "b".repeat(64))
        .await
        .unwrap()
    else {
        panic!("claim")
    };
    drop(h.nodes[owner].worker.take());
    let (successor, next) = h.acquire().await;
    assert_ne!(owner, successor);
    assert_eq!(
        stale.finish(&claim, &receipt()).await.err(),
        Some(ExecutionError::LostLease)
    );
    assert_eq!(
        stale.guard(&claim).await.err(),
        Some(ExecutionError::LostLease)
    );
    let current = ReplyStore(h.nodes[successor].journal(), next.partition.clone());
    assert!(matches!(
        current
            .admit_keyed(&h.plan, h.target(), "a".repeat(64), "b".repeat(64))
            .await
            .unwrap(),
        Admission::Pending | Admission::Unknown
    ));
    assert!(matches!(
        current.close(&h.plan, h.target()).await.unwrap(),
        ToolReplyStatus::Pending | ToolReplyStatus::Unknown
    ));
    assert!(h.rx.try_recv().is_err());
    h.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn unkeyed_replays_survive_intervening_content_and_closed_parent() {
    let mut h = Harness::new().await;
    let (owner, lease) = h.accepted().await;
    let tools = h.tools[owner].clone();
    let channel = h.current.channel.lock().unwrap().clone();
    let event = h.event();
    for text in ["first", "second"] {
        let mut message = message();
        message.text = Some(text.into());
        let (result, ()) = tokio::join!(
            tools.reply(caller(), &channel, &event, message, false),
            async {
                let reply = super::delivery_tests::command(&mut h.rx).await;
                let _permit = reply.gate.attempt().await.unwrap();
                assert!(reply.gate.commit());
                reply.receipt.send(receipt()).unwrap();
            }
        );
        assert!(matches!(
            result.unwrap().unwrap().delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
    }
    ReplyStore(h.nodes[owner].journal(), lease.partition.clone())
        .close(&h.plan, h.target())
        .await
        .unwrap();
    h.nodes[owner]
        .partitions
        .complete(lease.partition.as_ref().unwrap())
        .await
        .unwrap();
    let other = 1 - owner;
    for text in ["first", "second"] {
        let mut message = message();
        message.text = Some(text.into());
        let replay = h.tools[other]
            .reply(caller(), &channel, &event, message, false)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(replay.delivery, DeliveryOutcome::Confirmed { .. }));
    }
    assert!(h.rx.try_recv().is_err());
    h.current.revoke_at.store(0, Ordering::SeqCst);
    assert!(matches!(
        h.tools[other]
            .reply(caller(), &channel, &event, message(), false)
            .await,
        Err(ExecutionError::AuthorityChanged)
    ));
    h.cleanup().await;
}
