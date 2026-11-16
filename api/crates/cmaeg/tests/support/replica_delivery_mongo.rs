//! Delivery checkpoint qualification across independent clients. Real process
//! loss/scale and real provider receipt reconciliation have separate gates.
use super::{
    candidate, database::DatabaseFixture, replica_delivery::*, replica_execution::*,
    support::Fixture, verified,
};
use cmaeg::{
    delivery::intents::DeliveryCommand,
    turns::routed::{ExecutionError, FrameDelivery},
};
use std::sync::{atomic::Ordering, Arc};

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn confirmed_receipt_and_cursor_survive_independent_client_handoff_without_resend() {
    let db = DatabaseFixture::new(std::path::Path::new(env!("CARGO_BIN_EXE_cmaeg"))).await;
    let fixture = Fixture::new(routed_channel, Default::default()).await;
    fixture.recorder.outcome.store(1, Ordering::SeqCst);
    let (request, body) = verified(&fixture, "original").await;
    let first = DeliveryOracle::open(&db.uri, &db.name, runner_keys()).await;
    let second = DeliveryOracle::open(&db.uri, &db.name, runner_keys()).await;
    let plan = first
        .plans
        .admit(&candidate(&request, &body))
        .await
        .unwrap();
    let first_port = Arc::new(Port::default());
    let second_port = Arc::new(Port::default());
    let first_dispatcher = dispatcher(&first, request.channel.clone(), first_port.clone());
    let second_dispatcher = dispatcher(&second, request.channel.clone(), second_port.clone());
    let mut lease = accepted(&first_dispatcher, &plan, "first").await;
    let (sender, mut rx) = lane(&first, request.channel.clone());
    let terminal = frame(0).await;
    let target = &plan.targets[0].id;
    let (result, ()) = tokio::join!(
        sender.deliver_frame(&plan.identity, target, &mut lease, &terminal, effect()),
        async {
            let DeliveryCommand::DurableReply(reply) = rx.recv().await.unwrap() else {
                panic!("durable reply")
            };
            let facts = second.targets(&plan).await;
            let fact = facts[target].as_ref().unwrap();
            assert_eq!(fact.outbound, OutboundEvidence::Intent);
            assert!(!fact.cursor_committed);
            let _route = reply.gate.enter_route("fixture").await.unwrap();
            let _permit = reply.gate.attempt().await.unwrap();
            assert!(reply.gate.commit());
            reply.receipt.send(confirmed()).unwrap();
        }
    );
    assert!(matches!(
        result.unwrap(),
        FrameDelivery::Settled { cursor: 0, .. }
    ));
    first.journal().release(&mut lease).await.unwrap();
    assert_eq!(
        first.journal().guard(&lease).await,
        Err(ExecutionError::LostLease)
    );

    let mut successor = accepted(&second_dispatcher, &plan, "second").await;
    let (resumed, mut successor_rx) = lane(&second, request.channel.clone());
    assert!(matches!(
        resumed
            .deliver_frame(&plan.identity, target, &mut successor, &terminal, effect())
            .await
            .unwrap(),
        FrameDelivery::Settled { cursor: 0, .. }
    ));
    assert!(successor_rx.try_recv().is_err());
    second
        .journal()
        .complete(&mut successor, "response-fixture")
        .await
        .unwrap();
    let facts = first.targets(&plan).await;
    assert!(facts[target].as_ref().unwrap().confirmed_terminal_text());
    assert_eq!(first_port.0.load(Ordering::SeqCst), 1);
    assert_eq!(second_port.0.load(Ordering::SeqCst), 0);
    assert_eq!(second.plan(&plan.identity).await.unwrap(), plan);
    db.admin.database(&db.name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn cancelled_send_fences_old_gate_and_successor_retains_unknown_without_replay() {
    let db = DatabaseFixture::new(std::path::Path::new(env!("CARGO_BIN_EXE_cmaeg"))).await;
    let fixture = Fixture::new(routed_channel, Default::default()).await;
    fixture.recorder.outcome.store(1, Ordering::SeqCst);
    let (request, body) = verified(&fixture, "original").await;
    let first = DeliveryOracle::open(&db.uri, &db.name, runner_keys()).await;
    let second = DeliveryOracle::open(&db.uri, &db.name, runner_keys()).await;
    let plan = first
        .plans
        .admit(&candidate(&request, &body))
        .await
        .unwrap();
    let port = Arc::new(Port::default());
    let first_dispatcher = dispatcher(&first, request.channel.clone(), port.clone());
    let second_dispatcher = dispatcher(&second, request.channel.clone(), Arc::new(Port::default()));
    let mut lease = accepted(&first_dispatcher, &plan, "first").await;
    let (sender, mut rx) = lane(&first, request.channel.clone());
    let terminal = frame(0).await;
    let target = &plan.targets[0].id;
    let reply = {
        let delivery =
            sender.deliver_frame(&plan.identity, target, &mut lease, &terminal, effect());
        tokio::pin!(delivery);
        tokio::select! {
            _ = &mut delivery => panic!("delivery must await its receipt"),
            command = rx.recv() => match command.unwrap() {
                DeliveryCommand::DurableReply(reply) => reply,
                _ => panic!("durable reply"),
            },
        }
        // Dropping the suspended future cancels its gate, retaining Intent.
    };
    assert!(reply.gate.attempt().await.is_err());
    assert!(!reply.gate.commit());
    first.journal().release(&mut lease).await.unwrap();
    let mut successor = accepted(&second_dispatcher, &plan, "second").await;
    let (resumed, mut successor_rx) = lane(&second, request.channel.clone());
    assert!(matches!(
        resumed
            .deliver_frame(&plan.identity, target, &mut successor, &terminal, effect())
            .await
            .unwrap(),
        FrameDelivery::Unknown
    ));
    assert!(successor_rx.try_recv().is_err());
    assert!(second
        .journal()
        .complete(&mut successor, "response-fixture")
        .await
        .is_err());
    let facts = second.targets(&plan).await;
    assert_eq!(
        facts[target].as_ref().unwrap().outbound,
        OutboundEvidence::Unknown
    );
    assert!(!facts[target].as_ref().unwrap().confirmed_terminal_text());
    assert_eq!(port.0.load(Ordering::SeqCst), 1);
    drop(reply);
    db.admin.database(&db.name).drop().await.unwrap();
}
