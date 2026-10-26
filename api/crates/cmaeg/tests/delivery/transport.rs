use super::{fake_nyxid_reply::Reply, support::App};
use cmaeg::{
    delivery::intents::{DeliveryCommand, DeliveryOutcome, ReplySource},
    turns::outcomes::TurnOutcome,
    DeliveryConfig,
};
use cmaeg_core::{message::OutboundMessage, seal::ReplyAddress};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
#[tokio::test]
async fn rate_limit_retries_once_with_route_key_and_second_rate_limit_is_unknown() {
    for replies in [
        vec![Reply::status(429)],
        vec![Reply::status(429), Reply::status(429)],
    ] {
        let confirmed = replies.len() == 1;
        let mut app = App::new(replies, 0, Default::default(), Default::default()).await;
        let result = app.reply(json!({"text":"hello"})).await;
        assert_eq!(
            matches!(result.delivery, DeliveryOutcome::Confirmed { .. }),
            confirmed
        );
        if !confirmed {
            assert!(matches!(result.delivery, DeliveryOutcome::Unknown { .. }));
        }
        app.drain().await;
        let requests = app.fake.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].authorization, "Bearer test-route-credential");
        assert!(requests[1].at.duration_since(requests[0].at) >= Duration::from_secs(3));
    }
}
#[tokio::test]
async fn malformed_failed_redirected_and_lost_responses_are_unknown_without_retry() {
    let mut cases = vec![Reply::status(200), Reply::status(500), Reply::status(302)];
    let mut slow = Reply::status(200);
    slow.delay = Duration::from_secs(1);
    cases.push(slow);
    for reply in cases {
        let mut app = App::new(
            vec![reply],
            0,
            DeliveryConfig {
                reply_timeout: Duration::from_millis(100),
                ..Default::default()
            },
            Default::default(),
        )
        .await;
        let result = app.reply(json!({"text":"hello"})).await;
        assert!(matches!(result.delivery, DeliveryOutcome::Unknown { .. }));
        app.outcome(TurnOutcome::Completed {
            final_text: Some("possible duplicate".into()),
        })
        .await;
        app.drain().await;
        assert_eq!(app.fake.requests.lock().unwrap().len(), 1);
    }
}
#[tokio::test]
async fn expired_reply_token_uses_route_key_and_missing_credentials_reject_before_io() {
    let mut app = App::standard(0).await;
    if let ReplyAddress::NyxidRelay {
        reply_token_expires_at,
        ..
    } = &mut app.event.reply
    {
        *reply_token_expires_at = 1;
    }
    assert!(matches!(
        app.reply(json!({"text":"route key"})).await.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert_eq!(
        app.fake.requests.lock().unwrap()[0].authorization,
        "Bearer test-route-credential"
    );
    let turn = Arc::get_mut(&mut app.turn).unwrap();
    let previous = &turn.channel;
    let mut definition = previous.definition.clone();
    definition.secrets.channel_key = None;
    turn.channel = Arc::new(cmaeg::channels::resolver::ResolvedChannel {
        id: previous.id.clone(),
        pointer: previous.pointer.clone(),
        definition,
        agent_key_hash: previous.agent_key_hash,
        consumer_keys: previous.consumer_keys.clone(),
        loaded_at: previous.loaded_at,
        version: previous.version,
    });
    // Avoid sharing the event state from the preceding successful send.
    app.event.event_id = "new-event".parse().unwrap();
    let result = app.reply(json!({"text":"no credentials"})).await;
    assert_eq!(
        result.delivery,
        DeliveryOutcome::Rejected {
            reason: "no_credential".into()
        }
    );
    app.drain().await;
    assert_eq!(app.fake.requests.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn closed_receipt_skips_queued_tool_send_and_drain_deadline_cancels_slow_work() {
    let mut slow = Reply::status(200);
    slow.delay = Duration::from_secs(10);
    let mut app = App::new(vec![slow], 0, Default::default(), Default::default()).await;
    let (receipt, received) = oneshot::channel();
    drop(received);
    app.commands
        .send(DeliveryCommand::Reply {
            trace: tracing::Span::current(),
            channel: app.turn.channel.clone(),
            event: app.event.clone(),
            message: OutboundMessage {
                thread_reply: false,
                text: Some("cancelled".into()),
                blocks: vec![],
                reply_to: None,
            },
            source: ReplySource::Tool,
            receipt,
        })
        .await
        .unwrap();
    app.outcome(TurnOutcome::Completed {
        final_text: Some("slow".into()),
    })
    .await;
    app.fake.wait(1).await;
    assert!(
        !app.engine
            .take()
            .unwrap()
            .drain(Instant::now() + Duration::from_millis(100))
            .await
    );
    assert_eq!(app.fake.texts(), ["slow"]);
}
