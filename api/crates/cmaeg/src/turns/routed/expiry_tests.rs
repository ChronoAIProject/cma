use super::{delivery_tests::stored, expiry::EXPIRED_BEFORE_START, test_support::*, *};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn expiry_between_intent_and_reply_validation_still_settles_without_sending() {
    use super::{
        delivery_tests::{accepted, effect, lane},
        projection_store_tests::frame,
    };
    use crate::delivery::{durable::receipt_for, intents::DeliveryCommand};
    use std::sync::Arc;
    let setup = Setup::with_request(supported(), fixture::request(|value| {
        fixture::targets(value);
        value["routing"]["consumers"][0]["reply"] = serde_json::json!({"destination":"default"});
        value["routing"]["default_reply"] = serde_json::json!({"type":"telegram_chat","source_id":"telegram","chat_id":"-200","thread_id":"7"});
    }, fixture::custom(None))).await;
    let expires = setup.target().launch.event.expires_at;
    setup
        .server
        .memory
        .clock
        .store(expires - 1, Ordering::SeqCst);
    let mut lease = accepted(&setup).await;
    let (mut delivery, mut outbound) = lane(&setup);
    let memory = setup.server.memory.clone();
    let id = setup.target().id.clone();
    delivery.journal.clock = Arc::new(move || {
        let blob = memory.rows.lock().unwrap().get(&id).unwrap().blob.clone();
        let opened = cmaeg_core::seal::open_interaction(&fixture::keys(), &id, &blob).unwrap();
        let state: TargetState = serde_json::from_slice(&opened.payload).unwrap();
        if state
            .delivery
            .is_some_and(|checkpoint| matches!(checkpoint.status, DeliveryStatus::Intent))
        {
            // The first post-intent check sees a live reference. Its next use sees expiry.
            memory.clock.swap(expires, Ordering::SeqCst)
        } else {
            memory.clock.load(Ordering::SeqCst)
        }
    });
    let event = frame(
        serde_json::json!({"type":"response.completed","sequence_number":0,
        "response":{"id":"resp_1","output":[]}}),
    );
    let (done, finished) = tokio::sync::oneshot::channel();
    let (result, ()) = tokio::join!(
        async {
            let result = delivery
                .deliver_frame(
                    &setup.plan.identity,
                    &setup.target().id,
                    &mut lease,
                    &event,
                    effect(),
                )
                .await;
            let _ = done.send(());
            result
        },
        async {
            let command = tokio::select! {
                command = outbound.recv() => command,
                _ = finished => None,
            };
            if let Some(DeliveryCommand::DurableReply(reply)) = command {
                let refusal = match reply.gate.enter_route("fixture").await {
                    Err(refusal) => refusal,
                    Ok(_) => panic!("expired reference cannot authorize a send"),
                };
                reply.receipt.send(receipt_for(refusal.outcome())).unwrap();
            }
        }
    );
    assert!(matches!(
        result.unwrap(),
        FrameDelivery::Settled { cursor: 0, .. }
    ));
    delivery
        .journal
        .complete(&mut lease, "resp_1")
        .await
        .unwrap();
    assert!(matches!(
        stored(&setup).await.phase,
        Phase::Completed { .. }
    ));
    assert_eq!(setup.port.starts.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn accepted_automation_settles_after_reply_reference_expiry_without_another_start() {
    use super::{
        delivery_tests::{accepted, lane},
        projection_store_tests::frame,
    };
    use cmaeg_core::activity::SourceKind;
    for source in [
        SourceKind::Custom,
        SourceKind::Github {
            app_ref: "integration".into(),
            installation_id: 99,
            repository_id: 1001,
        },
    ] {
        for terminal in ["response.completed", "response.failed"] {
            let mut activity = fixture::custom(None);
            activity.source = source.clone();
            let request = fixture::request(
                |value| {
                    fixture::targets(value);
                    if matches!(source, SourceKind::Github { .. }) {
                        value["sources"]
                            .as_array_mut()
                            .unwrap()
                            .push(serde_json::json!({
                                "type":"github","app_ref":"integration","installation_ids":[99],
                                "repository_ids":[1001],"events":["issues"],"actions":["opened"]
                            }));
                    }
                    value["reply"]["sinks"] = serde_json::json!([]);
                    value["reply"]["auto_final"] = false.into();
                },
                activity,
            );
            let setup = Setup::with_request(supported(), request).await;
            let mut lease = accepted(&setup).await;
            setup.journal().release(&mut lease).await.unwrap();
            setup
                .server
                .memory
                .clock
                .store(fixture::NOW + 1801, Ordering::SeqCst);
            let DispatchOutcome::Replay {
                mut lease, body, ..
            } = setup.dispatch().await.unwrap()
            else {
                panic!("accepted work remains replayable");
            };
            let (delivery, mut sends) = lane(&setup);
            assert_eq!(
                delivery
                    .tool_reply_status(&setup.plan.identity, &setup.target().id, &lease, &body)
                    .await
                    .unwrap(),
                ToolReplyStatus::None
            );
            let event = frame(serde_json::json!({"type":terminal,"sequence_number":0,
                "response":{"id":"resp_1","output":[],"error":{"code":"agent_command_queue_timeout","message":"The message expired before dispatch."}}}));
            let result = delivery
                .deliver_frame(
                    &setup.plan.identity,
                    &setup.target().id,
                    &mut lease,
                    &event,
                    ReplayEffect::NoEffect,
                )
                .await
                .unwrap();
            assert!(matches!(
                result,
                FrameDelivery::Settled {
                    cursor: 0,
                    receipt: None
                }
            ));
            setup
                .journal()
                .complete(&mut lease, "resp_1")
                .await
                .unwrap();
            assert!(matches!(
                stored(&setup).await.phase,
                Phase::Completed { .. }
            ));
            setup.journal().release(&mut lease).await.unwrap();
            assert!(matches!(
                setup.dispatch().await.unwrap(),
                DispatchOutcome::Terminal(_)
            ));
            assert_eq!(setup.port.starts.lock().unwrap().len(), 1);
            assert!(sends.try_recv().is_err());
        }
    }
}

#[tokio::test]
async fn expired_pending_is_durable_and_replayed_without_preparing_or_starting() {
    let setup = Setup::new(supported()).await;
    setup
        .server
        .memory
        .clock
        .store(fixture::NOW + 900, Ordering::SeqCst);
    setup.current.revoke_at.store(0, Ordering::SeqCst);
    assert!(matches!(setup.dispatch().await.unwrap(),
        DispatchOutcome::Terminal(Phase::Refused { code }) if code == EXPIRED_BEFORE_START));
    let original = stored(&setup).await;
    assert!(original.body.is_none());
    assert!(original.delivery.is_none());
    assert!(original.projection.is_none());
    assert_eq!(
        original.event_expires_at,
        setup.target().launch.event.expires_at
    );
    assert_eq!(original.retained_until, setup.plan.retained_until);
    let restarted = Dispatcher::new(
        setup.server.store(),
        setup.journal().clone(),
        setup.current.clone(),
        setup.port.clone(),
    );
    for _ in 0..2 {
        assert!(
            matches!(restarted.dispatch(&setup.plan.identity, &setup.target().id, "restarted").await.unwrap(),
            DispatchOutcome::Terminal(Phase::Refused { code }) if code == EXPIRED_BEFORE_START)
        );
    }
    assert_eq!(stored(&setup).await.phase, original.phase);
    assert_eq!(setup.current.calls.load(Ordering::SeqCst), 0);
    assert_eq!(setup.port.prepares.load(Ordering::SeqCst), 0);
    assert!(setup.port.starts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn expiry_settles_proven_unstarted_requests_and_preserves_ambiguous_starts() {
    for phase in [Phase::Prepared, Phase::Starting, Phase::Unknown] {
        let setup = Setup::new(supported()).await;
        let mut lease = setup.prepared().await;
        setup
            .journal()
            .transition(&mut lease, phase.clone())
            .await
            .unwrap();
        let original_body = lease.state.body.clone();
        setup
            .server
            .memory
            .clock
            .store(fixture::NOW + 900, Ordering::SeqCst);
        let result = setup.dispatch().await;
        let retained = stored(&setup).await;
        if phase == Phase::Prepared {
            assert!(
                matches!(result, Ok(DispatchOutcome::Terminal(Phase::Refused { code })) if code == EXPIRED_BEFORE_START)
            );
            assert_eq!(
                retained.phase,
                Phase::Refused {
                    code: EXPIRED_BEFORE_START.into()
                }
            );
        } else {
            assert!(matches!(result, Err(ExecutionError::Expired)));
            assert_eq!(retained.phase, phase);
        }
        assert!(retained.body == original_body);
        assert!(setup.port.starts.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn expiry_does_not_reclassify_a_provider_request_with_unknown_acceptance() {
    let setup = Setup::new(supported()).await;
    *setup.port.next.lock().unwrap() = Some(StartResult::Unknown);
    assert!(matches!(
        setup.dispatch().await.unwrap(),
        DispatchOutcome::Unknown
    ));
    setup
        .server
        .memory
        .clock
        .store(fixture::NOW + 900, Ordering::SeqCst);
    assert!(matches!(
        setup.dispatch().await,
        Err(ExecutionError::Expired)
    ));
    assert_eq!(stored(&setup).await.phase, Phase::Unknown);
    assert_eq!(setup.port.starts.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn unrelated_expiry_and_stale_ownership_cannot_terminalize_pending() {
    let setup = Setup::new(supported()).await;
    let mut lease = setup
        .journal()
        .claim(&setup.plan, setup.target(), "old")
        .await
        .unwrap();
    // An expired ancestor or malformed reference is not this event's expiry.
    assert_eq!(
        setup.journal().expire_pending(&mut lease).await.unwrap(),
        None
    );
    setup
        .server
        .memory
        .clock
        .store(fixture::NOW + 900, Ordering::SeqCst);
    let newer = setup
        .journal()
        .claim(&setup.plan, setup.target(), "new")
        .await
        .unwrap();
    assert_eq!(
        setup.journal().expire_pending(&mut lease).await,
        Err(ExecutionError::LostLease)
    );
    let retained = stored(&setup).await;
    assert_eq!(retained.phase, Phase::Pending);
    assert_eq!(retained.ownership, newer.state.ownership);
}
