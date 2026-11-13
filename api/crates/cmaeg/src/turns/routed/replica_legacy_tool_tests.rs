//! Upgrade retained OFF-writer receipts through the actual ON reply entry point.
use super::{
    replica_test_fixture::*,
    tool_reply_store::{Admission, ReplyStore},
    *,
};
use crate::delivery::{durable::receipt_for, intents::DeliveryOutcome};
use std::{sync::atomic::Ordering, time::Duration};

fn legacy(h: &Harness, owner: usize) -> RoutedToolReplies {
    let mut journal = h.nodes[owner].journal();
    journal.partitions = None;
    RoutedToolReplies::new(
        h.nodes[owner].plans(),
        journal,
        h.current.clone(),
        h.tools[owner].coordinator.clone(),
    )
}
async fn retained(h: &Harness, owner: usize) -> (serde_json::Value, u64) {
    let id = storage::identifier(&("ordinary-route-reply-guard-v1", &h.target().id));
    h.nodes[owner].journal().read(&id).await.unwrap().unwrap()
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn legacy_unkeyed_confirmation_replays_when_enabled_with_open_or_closed_guard() {
    let mut h = Harness::new().await;
    let (owner, lease) = h.accepted().await;
    let old = legacy(&h, owner);
    let channel = h.current.channel.lock().unwrap().clone();
    let event = h.event();
    let (sent, ()) = tokio::join!(
        old.reply(caller(), &channel, &event, message(), false),
        async {
            let reply = super::delivery_tests::command(&mut h.rx).await;
            let _permit = reply.gate.attempt().await.unwrap();
            assert!(reply.gate.commit());
            reply.receipt.send(receipt()).unwrap();
        }
    );
    let expected = sent.unwrap().unwrap();
    let original = retained(&h, owner).await;
    assert!(original.0.get("operation_keys").is_none());
    assert!(original.0["tool"].get("content_hash").is_none());
    drop(old); // The old writer is stopped before the enabled reader is used.
    let store = ReplyStore(h.nodes[owner].journal(), lease.partition.clone());
    for closed in [false, true] {
        if closed {
            store.close(&h.plan, h.target()).await.unwrap();
        }
        let before = retained(&h, owner).await;
        assert_eq!(before.0["closed"], closed);
        h.nodes[owner]
            .journal()
            .guard_partition(&lease.partition)
            .await
            .unwrap();
        let replay = tokio::time::timeout(
            Duration::from_secs(2),
            h.tools[owner].reply(caller(), &channel, &event, message(), false),
        )
        .await
        .expect("retained confirmation must not wait for a send")
        .unwrap()
        .unwrap();
        assert_eq!(
            storage::bytes(&replay).unwrap(),
            storage::bytes(&expected).unwrap()
        );
        // The mutation entry point also protects the gap after an earlier replay read.
        let operation = storage::hash(&storage::bytes(&(&h.target().id, message())).unwrap());
        let content = storage::hash(&storage::bytes(&message()).unwrap());
        let Admission::Replay(replay) = store
            .admit_keyed(&h.plan, h.target(), operation, content)
            .await
            .unwrap()
        else {
            panic!("exact legacy receipt must precede a new keyed claim");
        };
        assert_eq!(
            storage::bytes(&replay).unwrap(),
            storage::bytes(&expected).unwrap()
        );
        assert_eq!(
            retained(&h, owner).await,
            before,
            "receipt replay is read-only"
        );
        assert!(h.rx.try_recv().is_err(), "no duplicate delivery command");
    }
    h.current.revoke_at.store(0, Ordering::SeqCst);
    assert!(matches!(
        h.tools[owner]
            .reply(caller(), &channel, &event, message(), false)
            .await,
        Err(ExecutionError::AuthorityChanged)
    ));
    h.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn legacy_unkeyed_pending_and_unknown_never_gain_resend_authority() {
    for unknown in [false, true] {
        let mut h = Harness::new().await;
        let (owner, _) = h.accepted().await;
        let old = legacy(&h, owner);
        let channel = h.current.channel.lock().unwrap().clone();
        let event = h.event();
        if unknown {
            let (result, ()) = tokio::join!(
                old.reply(caller(), &channel, &event, message(), false),
                async {
                    let reply = super::delivery_tests::command(&mut h.rx).await;
                    let _permit = reply.gate.attempt().await.unwrap();
                    reply
                        .receipt
                        .send(receipt_for(DeliveryOutcome::Unknown {
                            reason: "legacy_fixture_uncertain".into(),
                        }))
                        .unwrap();
                }
            );
            assert!(matches!(
                result.unwrap().unwrap().delivery,
                DeliveryOutcome::Unknown { .. }
            ));
        } else {
            let send = old.reply(caller(), &channel, &event, message(), false);
            tokio::pin!(send);
            let reply = tokio::select! {
                _ = &mut send => panic!("awaiting legacy send"),
                reply = super::delivery_tests::command(&mut h.rx) => reply,
            };
            let _permit = reply.gate.attempt().await.unwrap();
            assert!(reply.gate.commit());
            // Drop both sides after the external attempt; the intent stays pending.
        }
        drop(old);
        let before = retained(&h, owner).await;
        for different in [false, true] {
            let mut reply = message();
            if different {
                reply.text = Some("different message".into());
            }
            let replay = h.tools[owner]
                .reply(caller(), &channel, &event, reply, false)
                .await;
            if unknown {
                assert!(matches!(
                    replay.unwrap().unwrap().delivery,
                    DeliveryOutcome::Unknown { .. }
                ));
            } else {
                assert!(matches!(replay, Err(ExecutionError::Busy)));
            }
            assert_eq!(retained(&h, owner).await, before);
            assert!(h.rx.try_recv().is_err());
        }
        h.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn legacy_confirmation_does_not_confirm_different_enabled_content() {
    let mut h = Harness::new().await;
    let (owner, _) = h.accepted().await;
    let old = legacy(&h, owner);
    let channel = h.current.channel.lock().unwrap().clone();
    let event = h.event();
    let (sent, ()) = tokio::join!(
        old.reply(caller(), &channel, &event, message(), false),
        async {
            let reply = super::delivery_tests::command(&mut h.rx).await;
            let _permit = reply.gate.attempt().await.unwrap();
            reply.receipt.send(receipt()).unwrap();
        }
    );
    sent.unwrap().unwrap();
    drop(old);
    let mut changed = message();
    changed.text = Some("another explicit reply".into());
    let expected = receipt_for(DeliveryOutcome::Confirmed {
        platform_message_id: Some("second-distinct-receipt".into()),
    });
    let (sent, ()) = tokio::join!(
        h.tools[owner].reply(caller(), &channel, &event, changed, false),
        async {
            let reply = super::delivery_tests::command(&mut h.rx).await;
            let _permit = reply.gate.attempt().await.unwrap();
            assert!(reply.gate.commit());
            reply.receipt.send(expected.clone()).unwrap();
        }
    );
    assert_eq!(
        storage::bytes(&sent.unwrap().unwrap()).unwrap(),
        storage::bytes(&expected).unwrap()
    );
    assert!(h.rx.try_recv().is_err());
    h.cleanup().await;
}
