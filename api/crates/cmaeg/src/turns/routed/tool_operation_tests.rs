use super::{
    delivery_tests::*,
    test_support::*,
    tool_reply_store::{Admission, Kind, ReplyStore},
    *,
};
use crate::{
    auth::AgentCaller,
    channels::resolver::ResolvedChannel,
    delivery::{
        durable::receipt_for,
        intents::{DeliveryCommand, DeliveryOutcome, IntentReceipt},
    },
};
use cmaeg_core::{message::OutboundMessage, seal::EventRefPayload};
use std::sync::{atomic::Ordering, Arc};
use tokio::sync::mpsc;

fn message() -> OutboundMessage {
    let ReplayEffect::Reply(message) = effect() else {
        unreachable!()
    };
    *message
}
fn receipt() -> IntentReceipt {
    receipt_for(DeliveryOutcome::Confirmed {
        platform_message_id: Some("keyed-send".into()),
    })
}
fn operation(setup: &Setup, key: &str) -> String {
    storage::hash(&storage::bytes(&("reply-operation-v1", &setup.target().id, key)).unwrap())
}
fn content() -> String {
    storage::hash(&storage::bytes(&message()).unwrap())
}
fn guard_id(setup: &Setup) -> String {
    storage::identifier(&("ordinary-route-reply-guard-v1", &setup.target().id))
}
fn operation_id(setup: &Setup, key: &str) -> String {
    storage::identifier(&(
        "ordinary_route_tool_operation_v1",
        &setup.target().id,
        operation(setup, key),
    ))
}
struct Harness {
    setup: Setup,
    caller: AgentCaller,
    channel: Arc<ResolvedChannel>,
    event: EventRefPayload,
    tools: RoutedToolReplies,
    rx: mpsc::Receiver<DeliveryCommand>,
}
impl Harness {
    async fn new() -> Self {
        let setup = Setup::new(supported()).await;
        {
            let mut current = setup.current.channel.lock().unwrap();
            Arc::get_mut(&mut current).unwrap().consumer_keys =
                vec![(setup.target().launch.consumer.clone(), [9; 32])];
        }
        let lease = accepted(&setup).await;
        let event = event(&setup.journal().body(&lease).await.unwrap());
        let channel = setup.current.channel.lock().unwrap().clone();
        let (delivery, rx) = lane(&setup);
        Self {
            setup,
            caller: AgentCaller { key_hash: [9; 32] },
            channel,
            event,
            tools: delivery.tool_replies(),
            rx,
        }
    }
    async fn send(&mut self, key: Option<&str>) -> IntentReceipt {
        let (result, ()) = tokio::join!(
            self.tools.reply_with_idempotency(
                self.caller,
                &self.channel,
                &self.event,
                message(),
                false,
                key
            ),
            async {
                let reply = command(&mut self.rx).await;
                let _permit = reply.gate.attempt().await.unwrap();
                assert!(reply.gate.commit());
                reply.receipt.send(receipt()).unwrap();
            }
        );
        result.unwrap().unwrap()
    }
}

#[tokio::test]
async fn stable_keys_replay_but_distinct_keys_send_identical_content_and_survive_restart_and_close()
{
    let mut h = Harness::new().await;
    let first = h.send(Some("first")).await;
    let second = h.send(Some("second")).await;
    assert_eq!(
        storage::bytes(&first).unwrap(),
        storage::bytes(&second).unwrap()
    );
    let store = ReplyStore(h.setup.journal().clone());
    assert_eq!(
        store.close(&h.setup.plan, h.setup.target()).await.unwrap(),
        ToolReplyStatus::Confirmed
    );
    let (delivery, mut restarted_rx) = lane(&h.setup);
    for key in ["first", "second"] {
        let result = delivery
            .tool_replies()
            .reply_with_idempotency(h.caller, &h.channel, &h.event, message(), false, Some(key))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            storage::bytes(&result).unwrap(),
            storage::bytes(&first).unwrap()
        );
        let rows = h.setup.server.memory.rows.lock().unwrap();
        let row = &rows[&operation_id(&h.setup, key)];
        assert_eq!(row.expiry, h.setup.plan.retained_until);
        assert_eq!(row.owner, h.setup.plan.identity.owner);
        assert_eq!(row.version, 2);
    }
    assert!(h.rx.try_recv().is_err());
    assert!(restarted_rx.try_recv().is_err());
}

#[tokio::test]
async fn changed_content_conflicts_for_pending_and_confirmed_keys_without_sending() {
    let mut h = Harness::new().await;
    let mut changed = message();
    changed.text = Some("different text".into());
    let (result, ()) = tokio::join!(
        h.tools.reply_with_idempotency(
            h.caller,
            &h.channel,
            &h.event,
            message(),
            false,
            Some("same")
        ),
        async {
            let reply = command(&mut h.rx).await;
            assert!(matches!(
                h.tools
                    .reply_with_idempotency(
                        h.caller,
                        &h.channel,
                        &h.event,
                        changed.clone(),
                        false,
                        Some("same")
                    )
                    .await,
                Err(ExecutionError::IdempotencyConflict)
            ));
            let _permit = reply.gate.attempt().await.unwrap();
            assert!(reply.gate.commit());
            reply.receipt.send(receipt()).unwrap();
        }
    );
    result.unwrap();
    assert!(matches!(
        h.tools
            .reply_with_idempotency(h.caller, &h.channel, &h.event, changed, false, Some("same"))
            .await,
        Err(ExecutionError::IdempotencyConflict)
    ));
    assert!(h.rx.try_recv().is_err());
}

#[tokio::test]
async fn concurrent_duplicate_calls_have_one_send_and_retry_the_retained_receipt() {
    let mut h = Harness::new().await;
    let call = || {
        h.tools.reply_with_idempotency(
            h.caller,
            &h.channel,
            &h.event,
            message(),
            false,
            Some("race"),
        )
    };
    let (first, second, ()) = tokio::join!(call(), call(), async {
        let reply = command(&mut h.rx).await;
        let _permit = reply.gate.attempt().await.unwrap();
        assert!(reply.gate.commit());
        reply.receipt.send(receipt()).unwrap();
    });
    assert!(first.is_ok() || second.is_ok());
    for result in [first, second] {
        match result {
            Ok(Some(value)) => assert_eq!(
                storage::bytes(&value).unwrap(),
                storage::bytes(&receipt()).unwrap()
            ),
            Err(ExecutionError::Busy | ExecutionError::LostLease) => {}
            other => panic!("unexpected concurrent result: {other:?}"),
        }
    }
    let retried = call().await.unwrap().unwrap();
    assert_eq!(
        storage::bytes(&retried).unwrap(),
        storage::bytes(&receipt()).unwrap()
    );
    assert!(h.rx.try_recv().is_err());
}

#[tokio::test]
async fn operation_cap_rejects_before_writes_but_keeps_old_receipts_replayable() {
    let setup = Setup::new(supported()).await;
    let store = ReplyStore(setup.journal().clone());
    for index in 0..64 {
        let key = index.to_string();
        let Admission::Claimed(claim) = store
            .admit_keyed(
                &setup.plan,
                setup.target(),
                operation(&setup, &key),
                content(),
            )
            .await
            .unwrap()
        else {
            panic!("claim")
        };
        store.finish(&claim, &receipt()).await.unwrap();
    }
    let writes = setup.server.memory.writes.load(Ordering::SeqCst);
    let rows = setup.server.memory.rows.lock().unwrap().len();
    assert!(matches!(
        store
            .admit_keyed(
                &setup.plan,
                setup.target(),
                operation(&setup, "65"),
                content()
            )
            .await,
        Err(ExecutionError::OperationLimit)
    ));
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
    assert_eq!(setup.server.memory.rows.lock().unwrap().len(), rows);
    assert!(!setup
        .server
        .memory
        .rows
        .lock()
        .unwrap()
        .contains_key(&operation_id(&setup, "65")));
    assert!(matches!(
        store
            .admit_keyed(
                &setup.plan,
                setup.target(),
                operation(&setup, "0"),
                content()
            )
            .await
            .unwrap(),
        Admission::Replay(_)
    ));
}

#[tokio::test]
async fn ambiguous_admission_or_preparation_never_produces_a_send_permission() {
    for prepare in [false, true] {
        for persisted in [false, true] {
            let setup = Setup::new(supported()).await;
            let store = ReplyStore(setup.journal().clone());
            let id = if prepare {
                operation_id(&setup, "ambiguous")
            } else {
                guard_id(&setup)
            };
            setup.server.fault(id, persisted, true);
            assert!(store
                .admit_keyed(
                    &setup.plan,
                    setup.target(),
                    operation(&setup, "ambiguous"),
                    content()
                )
                .await
                .is_err());
            if prepare || persisted {
                assert!(matches!(
                    store
                        .admit_keyed(
                            &setup.plan,
                            setup.target(),
                            operation(&setup, "ambiguous"),
                            content()
                        )
                        .await
                        .unwrap(),
                    Admission::Pending
                ));
                setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
                assert!(matches!(
                    store
                        .admit_keyed(
                            &setup.plan,
                            setup.target(),
                            operation(&setup, "ambiguous"),
                            content()
                        )
                        .await
                        .unwrap(),
                    Admission::Unknown
                ));
                assert!(matches!(
                    store
                        .admit_keyed(
                            &setup.plan,
                            setup.target(),
                            operation(&setup, "different"),
                            content()
                        )
                        .await
                        .unwrap(),
                    Admission::Unknown
                ));
            } else {
                // Readback proved the initial guard absent, and no send was admitted.
                assert!(matches!(
                    store
                        .admit_keyed(
                            &setup.plan,
                            setup.target(),
                            operation(&setup, "ambiguous"),
                            content()
                        )
                        .await
                        .unwrap(),
                    Admission::Claimed(_)
                ));
            }
        }
    }
}

#[tokio::test]
async fn receipt_storage_loss_stays_unknown_and_persisted_receipts_reconcile_before_auto_output() {
    for guard in [false, true] {
        for persisted in [false, true] {
            let setup = Setup::new(supported()).await;
            let store = ReplyStore(setup.journal().clone());
            let Admission::Claimed(claim) = store
                .admit_keyed(
                    &setup.plan,
                    setup.target(),
                    operation(&setup, "sent"),
                    content(),
                )
                .await
                .unwrap()
            else {
                panic!("claim")
            };
            let id = if guard {
                guard_id(&setup)
            } else {
                operation_id(&setup, "sent")
            };
            setup.server.fault(id, persisted, true);
            assert!(store.finish(&claim, &receipt()).await.is_err());
            setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
            let has_receipt = guard || persisted;
            assert_eq!(
                store.status(&setup.plan, setup.target()).await.unwrap(),
                if has_receipt {
                    ToolReplyStatus::Confirmed
                } else {
                    ToolReplyStatus::Unknown
                }
            );
            let retry = store
                .admit_keyed(
                    &setup.plan,
                    setup.target(),
                    operation(&setup, "sent"),
                    content(),
                )
                .await
                .unwrap();
            if has_receipt {
                assert!(
                    matches!(retry, Admission::Replay(value) if storage::bytes(&value).unwrap() == storage::bytes(&receipt()).unwrap())
                );
                assert!(matches!(
                    store
                        .admit(&setup.plan, setup.target(), Kind::Automatic, "a".repeat(64))
                        .await
                        .unwrap(),
                    Admission::Suppressed
                ));
            } else {
                assert!(matches!(retry, Admission::Unknown));
            }
        }
    }
}

#[tokio::test]
async fn rejected_unsupported_and_unknown_receipts_are_replayed_without_new_effects() {
    for outcome in [
        DeliveryOutcome::Rejected {
            reason: "refused".into(),
        },
        DeliveryOutcome::RateLimited { retry_after: 3 },
        DeliveryOutcome::Unsupported,
        DeliveryOutcome::Unknown {
            reason: "no confirmation".into(),
        },
    ] {
        let setup = Setup::new(supported()).await;
        let store = ReplyStore(setup.journal().clone());
        let Admission::Claimed(claim) = store
            .admit_keyed(
                &setup.plan,
                setup.target(),
                operation(&setup, "result"),
                content(),
            )
            .await
            .unwrap()
        else {
            panic!("claim")
        };
        let saved = receipt_for(outcome);
        store.finish(&claim, &saved).await.unwrap();
        let Admission::Replay(replayed) = store
            .admit_keyed(
                &setup.plan,
                setup.target(),
                operation(&setup, "result"),
                content(),
            )
            .await
            .unwrap()
        else {
            panic!("replay")
        };
        assert_eq!(
            storage::bytes(&replayed).unwrap(),
            storage::bytes(&saved).unwrap()
        );
    }
}

#[tokio::test]
async fn requests_without_keys_keep_the_old_guard_shape_and_storage_count() {
    let mut h = Harness::new().await;
    let writes = h.setup.server.memory.writes.load(Ordering::SeqCst);
    let rows = h.setup.server.memory.rows.lock().unwrap().len();
    h.send(None).await;
    assert_eq!(
        h.setup.server.memory.writes.load(Ordering::SeqCst),
        writes + 2
    );
    assert_eq!(h.setup.server.memory.rows.lock().unwrap().len(), rows + 1);
    let (record, _) = h
        .setup
        .journal()
        .read::<serde_json::Value>(&guard_id(&h.setup))
        .await
        .unwrap()
        .unwrap();
    assert!(record.get("operation_keys").is_none());
    assert!(record["tool"].get("content_hash").is_none());
    let replay = h
        .tools
        .reply(h.caller, &h.channel, &h.event, message(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        storage::bytes(&replay).unwrap(),
        storage::bytes(&receipt()).unwrap()
    );
    assert!(h.rx.try_recv().is_err());
}

#[path = "split_receipt_tests.rs"]
mod split_receipts;
