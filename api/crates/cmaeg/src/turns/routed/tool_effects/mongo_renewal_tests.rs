//! Exercise real ownership loss while the delivery coordinator awaits a receipt.
use super::{super::replica_test_fixture::*, mongo_progress_fixture::*, *};
use crate::{
    delivery::intents::DeliveryOutcome,
    turns::routed::{
        adapter_test_server::HttpServer, delivery_tests::command, test_support::recovery_frame,
        tool_reply_store::ReplyStore, DispatchOutcome, HttpExecutionPort, ReplayEffect,
        RoutedDelivery, ToolReplyStatus,
    },
};
use std::sync::atomic::Ordering;

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn concurrent_replica_observes_pending_through_slow_renewed_send() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut lease) = h.accepted().await;
            let journal = h.nodes[owner].journal();
            let delivery = RoutedDelivery::new(
                h.nodes[owner].plans(),
                journal.clone(),
                h.current.clone(),
                h.tools[owner].coordinator.clone(),
            );
            let mut rx = std::mem::replace(&mut h.rx, tokio::sync::mpsc::channel(1).1);
            let frame = recovery_frame("fixture-response", 0, false);
            let visitor = h.nodes[1 - owner].journal();
            let (sent, ()) = tokio::join!(
                delivery.deliver_frame(
                    &h.plan.identity,
                    &h.target().id,
                    &mut lease,
                    &frame,
                    ReplayEffect::Reply(Box::new(message()))
                ),
                async {
                    let reply = command(&mut rx).await;
                    let _permit = reply.gate.attempt().await.unwrap();
                    assert!(reply.gate.commit());
                    let before = ReplyStore(visitor.clone(), None)
                        .operator_record(&h.plan, h.target())
                        .await
                        .unwrap()
                        .0;
                    tokio::time::sleep(Duration::from_secs(16)).await;
                    assert_eq!(
                        ReplyStore(visitor.clone(), None)
                            .status(&h.plan, h.target())
                            .await
                            .unwrap(),
                        ToolReplyStatus::Pending
                    );
                    assert!(matches!(
                        visitor
                            .claim(&h.plan, h.target(), "concurrent-visitor")
                            .await,
                        Err(ExecutionError::Busy)
                    ));
                    assert!(
                        reply.gate.commit(),
                        "healthy send scope remains live beyond its first lease"
                    );
                    let after = ReplyStore(visitor.clone(), None)
                        .operator_record(&h.plan, h.target())
                        .await
                        .unwrap()
                        .0;
                    assert_eq!(before["automatic"]["claim"], after["automatic"]["claim"]);
                    assert!(
                        after["automatic"]["until"].as_i64().unwrap()
                            > before["automatic"]["until"].as_i64().unwrap()
                    );
                    reply.receipt.send(receipt()).unwrap();
                }
            );
            assert!(matches!(
                sent.unwrap(),
                super::super::FrameDelivery::Settled { .. }
            ));
            assert!(rx.try_recv().is_err());
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn automatic_and_immediate_send_renewal_loss_cancels_late_receipts() {
    for immediate in [false, true] {
        with_harness(|h| {
            Box::pin(async move {
                let (owner, mut lease) = h.accepted().await;
                let journal = h.nodes[owner].journal();
                let delivery = RoutedDelivery::new(
                    h.nodes[owner].plans(),
                    journal.clone(),
                    h.current.clone(),
                    h.tools[owner].coordinator.clone(),
                );
                let mut rx = std::mem::replace(&mut h.rx, tokio::sync::mpsc::channel(1).1);
                let frame = recovery_frame("fixture-response", 0, false);
                let channel = h.current.channel.lock().unwrap().clone();
                let send = async {
                    if immediate {
                        h.tools[owner]
                            .reply(caller(), &channel, &h.event(), message(), false)
                            .await
                            .map(|_| ())
                    } else {
                        delivery
                            .deliver_frame(
                                &h.plan.identity,
                                &h.target().id,
                                &mut lease,
                                &frame,
                                ReplayEffect::Reply(Box::new(message())),
                            )
                            .await
                            .map(|_| ())
                    }
                };
                let (result, reply) = tokio::join!(send, async {
                    let reply = command(&mut rx).await;
                    let _permit = reply.gate.attempt().await.unwrap();
                    assert!(reply.gate.commit());
                    h.nodes[owner].lose_work(&h.target().id).await.unwrap();
                    reply
                });
                assert!(matches!(result, Err(ExecutionError::LostLease)));
                assert!(!reply.gate.commit());
                assert!(reply.receipt.send(receipt()).is_err());
                assert!(rx.try_recv().is_err());
            })
        })
        .await;
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn delayed_send_renewal_loss_preserves_unknown_without_resend() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut lease) = h.accepted().await;
            h.finish_parent(owner, &mut lease).await;
            let mut capability = h.capability();
            capability.issued_at -= 5;
            let key = "renewal-loss";
            assert!(matches!(
                h.tools[0]
                    .reply_delayed(caller(), &capability, key, message())
                    .await,
                Err(ExecutionError::Busy)
            ));
            let id = effect_id(h, &capability, key);
            let mut rx = std::mem::replace(&mut h.rx, tokio::sync::mpsc::channel(1).1);
            let ((), reply) = tokio::join!(sweep(h, &id), async {
                let reply = command(&mut rx).await;
                let _permit = reply.gate.attempt().await.unwrap();
                assert!(reply.gate.commit());
                let mut revoked = false;
                for node in &h.nodes {
                    if node.lose_work(&id).await.is_ok() {
                        revoked = true;
                        break;
                    }
                }
                assert!(revoked);
                reply
            });
            assert!(!reply.gate.commit());
            assert!(reply.receipt.send(receipt()).is_err());
            sweep(h, &id).await;
            let retained = h.tools[1]
                .reply_delayed(caller(), &capability, key, message())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(retained.delivery, DeliveryOutcome::Unknown { .. }));
            assert!(rx.try_recv().is_err());
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn acceptance_renewal_loss_never_commits_late_response_or_reposts() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut lease) = h.acquire().await;
            let journal = h.nodes[owner].journal();
            journal.release(&mut lease).await.unwrap();
            let http = HttpServer::start().await;
            http.memory.lock().unwrap().config.post_delay = Duration::from_secs(16);
            let port = Arc::new(HttpExecutionPort::new(
                http.client(true, Duration::from_secs(30)),
                Arc::new(Builder(h.database.keys())),
                h.nodes[owner].plans(),
                journal.clone(),
                h.current.clone(),
            ));
            let dispatcher = port.clone().dispatcher();
            let (result, ()) = tokio::join!(
                dispatcher.dispatch(&h.plan.identity, &h.target().id, "acceptance-loss"),
                async {
                    while http.seen().iter().all(|r| r.method != "POST") {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    h.nodes[owner].lose_work(&h.target().id).await.unwrap();
                }
            );
            assert!(matches!(result, Err(ExecutionError::LostLease)));
            let result = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    for node in &h.nodes {
                        let recovery = Arc::new(HttpExecutionPort::new(
                            http.client(true, Duration::from_secs(30)),
                            Arc::new(Builder(h.database.keys())),
                            node.plans(),
                            node.journal(),
                            h.current.clone(),
                        ));
                        let result = recovery
                            .dispatcher()
                            .dispatch(&h.plan.identity, &h.target().id, "recover")
                            .await;
                        if !matches!(result, Err(ExecutionError::Busy)) {
                            return result;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await
            .unwrap();
            assert!(matches!(result, Ok(DispatchOutcome::Unknown)));
            assert_eq!(http.seen().iter().filter(|r| r.method == "POST").count(), 1);
            assert_eq!(h.current.revoke_at.load(Ordering::SeqCst), usize::MAX);
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn parent_retry_preserves_a_slow_immediate_tool_send_until_receipt() {
    use crate::{
        http::readiness::Readiness,
        turns::{
            routed::{
                scheduler::{Disposition, TargetExecutor, WorkItem},
                RoutedTargetExecutor,
            },
            TurnTracker,
        },
        DeliveryConfig,
    };
    with_harness(|h| {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(3200)).await;
            let (owner, mut lease) = h.accepted().await;
            let journal = h.nodes[owner].journal();
            journal
                .transition(
                    &mut lease,
                    Phase::Accepted {
                        response_id: "resp_accepted".into(),
                        durable_cursor: None,
                    },
                )
                .await
                .unwrap();
            journal.release(&mut lease).await.unwrap();
            let http = HttpServer::start().await;
            let snapshot = Arc::new(tokio::sync::Notify::new());
            let notify = snapshot.clone();
            http.memory.lock().unwrap().config.on_snapshot =
                Some(Arc::new(move || notify.notify_one()));
            http.memory.lock().unwrap().config.replay_delay = Duration::from_secs(1);
            let port = Arc::new(HttpExecutionPort::new(
                http.client(true, Duration::from_secs(30)),
                Arc::new(Builder(h.database.keys())),
                h.nodes[owner].plans(),
                journal.clone(),
                h.current.clone(),
            ));
            let delivery = RoutedDelivery::new(
                h.nodes[owner].plans(),
                journal.clone(),
                h.current.clone(),
                h.tools[owner].coordinator.clone(),
            );
            let runner = RoutedTargetExecutor::new(
                port,
                delivery,
                TurnTracker::new(Readiness::initialized()),
                &DeliveryConfig::default(),
            );
            let work = WorkItem {
                identity: h.plan.identity.clone(),
                target_id: h.target().id.clone(),
            };
            let mut rx = std::mem::replace(&mut h.rx, tokio::sync::mpsc::channel(1).1);
            let channel = h.current.channel.lock().unwrap().clone();
            let (parent, tool, ()) = tokio::join!(
                runner.execute(&work),
                async {
                    snapshot.notified().await;
                    h.tools[owner]
                        .reply(caller(), &channel, &h.event(), message(), false)
                        .await
                },
                async {
                    let reply = command(&mut rx).await;
                    let _permit = reply.gate.attempt().await.unwrap();
                    assert!(reply.gate.commit());
                    tokio::time::sleep(Duration::from_secs(16)).await;
                    assert_eq!(
                        ReplyStore(h.nodes[1 - owner].journal(), None)
                            .status(&h.plan, h.target())
                            .await
                            .unwrap(),
                        ToolReplyStatus::Pending
                    );
                    assert!(reply.gate.commit());
                    reply.receipt.send(receipt()).unwrap();
                }
            );
            assert!(matches!(parent, Disposition::Retry(_)));
            assert!(matches!(
                tool.unwrap().unwrap().delivery,
                DeliveryOutcome::Confirmed { .. }
            ));
            assert!(rx.try_recv().is_err());
            assert!(http.seen().iter().all(|r| r.method == "GET"));
        })
    })
    .await;
}
