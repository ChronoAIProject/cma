use super::{
    delivery_tests::{command, lane, stored},
    executor_tests::{executor, frames},
    scheduler::{Disposition, TargetExecutor},
    test_support::*,
    tool_reply_store::{Admission, Kind, ReplyStore},
    *,
};
use crate::{
    auth::AgentCaller,
    delivery::{durable::receipt_for, intents::DeliveryOutcome},
};
use std::sync::Arc;

#[tokio::test]
async fn restarted_executor_uses_durable_tool_receipt_without_process_tracker() {
    let setup = Setup::new(supported()).await;
    {
        let mut current = setup.current.channel.lock().unwrap();
        Arc::get_mut(&mut current).unwrap().consumer_keys =
            vec![(setup.target().launch.consumer.clone(), [9; 32])];
    }
    let http = super::adapter_test_server::HttpServer::start().await;
    http.memory.lock().unwrap().config.replay_frames = Some(frames());
    let (mut first, _rx, work) = executor(&setup, &http);
    first.frame_limit = 1;
    assert!(matches!(first.execute(&work).await, Disposition::Retry(_)));
    let mut lease = setup
        .journal()
        .claim(&setup.plan, setup.target(), "tool-fixture")
        .await
        .unwrap();
    let body = setup.journal().body(&lease).await.unwrap();
    let event = event(&body);
    setup.journal().release(&mut lease).await.unwrap();
    let channel = setup.current.channel.lock().unwrap().clone();
    let (delivery, mut rx) = lane(&setup);
    let tools = delivery.tool_replies();
    let ReplayEffect::Reply(message) = super::delivery_tests::effect() else {
        unreachable!();
    };
    let (result, ()) = tokio::join!(
        tools.reply(
            AgentCaller { key_hash: [9; 32] },
            &channel,
            &event,
            *message,
            false
        ),
        async {
            let reply = command(&mut rx).await;
            let _permit = reply.gate.attempt().await.unwrap();
            assert!(reply.gate.commit());
            reply
                .receipt
                .send(receipt_for(DeliveryOutcome::Confirmed {
                    platform_message_id: Some("tool-answer".into()),
                }))
                .unwrap();
        }
    );
    result.unwrap();
    drop(first);
    let (resumed, mut rx, work) = executor(&setup, &http);
    assert!(matches!(resumed.execute(&work).await, Disposition::Settled));
    assert!(matches!(
        stored(&setup).await.phase,
        Phase::Completed {
            durable_cursor: Some(3),
            ..
        }
    ));
    assert!(
        rx.try_recv().is_err(),
        "confirmed tool receipt suppresses automatic fallback after restart"
    );
    assert_eq!(
        http.seen()
            .iter()
            .filter(|request| request.method == "POST")
            .count(),
        1
    );
}

#[tokio::test]
async fn pending_then_unknown_tool_delivery_prevents_terminal_completion_without_public_text() {
    let setup = Setup::new(supported()).await;
    let http = super::adapter_test_server::HttpServer::start().await;
    // Empty final output means no automatic public message is needed, but an
    // unresolved independent tool delivery must still prevent completion.
    let (mut runner, mut rx, work) = executor(&setup, &http);
    runner.frame_limit = 1;
    assert!(matches!(runner.execute(&work).await, Disposition::Retry(_)));
    let store = ReplyStore(setup.journal().clone());
    let Admission::Claimed(claim) = store
        .admit(&setup.plan, setup.target(), Kind::Tool, "a".repeat(64))
        .await
        .unwrap()
    else {
        panic!("claim");
    };
    runner.frame_limit = 4096;
    assert!(matches!(runner.execute(&work).await, Disposition::Retry(_)));
    assert!(matches!(
        stored(&setup).await.phase,
        Phase::Accepted {
            durable_cursor: Some(1),
            ..
        }
    ));
    store
        .finish(
            &claim,
            &receipt_for(DeliveryOutcome::Unknown {
                reason: "receipt_lost".into(),
            }),
        )
        .await
        .unwrap();
    assert!(matches!(runner.execute(&work).await, Disposition::Settled));
    assert!(matches!(
        stored(&setup).await.phase,
        Phase::Accepted {
            durable_cursor: Some(1),
            ..
        }
    ));
    assert!(rx.try_recv().is_err());
}
