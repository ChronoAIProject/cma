//! Unit-compiled to seed only resolver I/O; actual routers and delivery run in process.
#[allow(dead_code)]
#[path = "../delivery/fake_http.rs"]
mod fake_http;
#[path = "deadline_observability.rs"]
mod observability;
#[allow(dead_code)]
#[path = "../delivery/recording_fixtures.rs"]
mod recording_fixtures;
use crate::channels::routing_fixture;
#[path = "deadline_support.rs"]
mod support;

use super::routed::TOOL_DEADLINE;
use crate::{
    delivery::{
        intents::{DeliveryCommand, DeliveryOutcome},
        router::ShardedScheduler,
    },
    http::request_start::RequestStart,
    turns::outcomes::TurnOutcome,
};
use cmaeg_core::message::OutboundMessage;
use fake_http::FakeHttp;
use futures_util::future::join_all;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use support::{caller, App};
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};

fn message(text: &str) -> OutboundMessage {
    serde_json::from_value(json!({"text":text})).unwrap()
}
fn unknown(value: &Value) {
    assert_eq!(value["delivery"]["outcome"], "unknown", "{value}");
    assert_eq!(value["delivery"]["reason"], "delivery_in_progress");
}

#[tokio::test(start_paused = true)]
async fn eight_consumers_answer_at_request_deadline_then_all_send_and_suppress_auto_final() {
    for body_delay in [0, 5] {
        let app = App::new(true, true, FakeHttp::relay());
        let http = app.http.clone();
        let started = Instant::now();
        let replies = (0..8).map(|index| {
            let reference = app.reference(index);
            let router = app.router();
            async move {
                let path = format!("/v1/events/{}/replies", reference.as_str());
                let (path, body) = if body_delay == 0 { (path, json!({"text":"private reply"})) }
                else { ("/mcp".into(), json!({"jsonrpc":"2.0","id":index,"method":"tools/call",
                    "params":{"name":"replyToEvent","arguments":{"event_ref":reference,"message":{"text":"private reply"}}}})) };
                let (status, value) = support::call(router, index, &path, body, Duration::from_secs(body_delay)).await;
                assert_eq!(status, 200, "{value}");
                assert!(started.elapsed() <= TOOL_DEADLINE + Duration::from_millis(100));
                if body_delay == 0 { value } else { value["result"]["structuredContent"].clone() }
            }
        });
        let receipts = join_all(replies).await;
        let confirmed = receipts
            .iter()
            .filter(|r| r["delivery"]["outcome"] == "confirmed")
            .count();
        assert_eq!(confirmed, if body_delay == 0 { 3 } else { 1 });
        for value in receipts
            .iter()
            .filter(|r| r["delivery"]["outcome"] != "confirmed")
        {
            unknown(value);
        }
        for index in 0..8 {
            let mut turn = recording_fixtures::turn(
                &app.state.keys,
                &app.channel,
                app.activity(),
                &app.event(index),
            );
            Arc::get_mut(&mut turn).unwrap().consumer = app.event(index).consumer;
            app.state
                .delivery
                .submit(DeliveryCommand::StreamOutcome {
                    turn,
                    outcome: TurnOutcome::Completed {
                        final_text: Some("duplicate auto-final".into()),
                    },
                })
                .await
                .unwrap();
        }
        app.finish().await;
        let sent = http.sent();
        assert_eq!(sent.len(), 8);
        for pair in sent.windows(2) {
            assert!(pair[1].at - pair[0].at >= Duration::from_secs(3));
        }
        assert!(sent
            .iter()
            .all(|s| !s.json().to_string().contains("duplicate auto-final")));
    }
}

#[tokio::test(start_paused = true)]
async fn expired_budget_and_full_queue_admit_nothing() {
    let mut app = App::new(true, true, FakeHttp::relay());
    app.drain().await;
    let (inbound, mut commands) = mpsc::channel(1);
    app.state.delivery = Arc::new(ShardedScheduler::new(inbound));
    let reference = app.reference(0);
    let start = RequestStart(Instant::now() - TOOL_DEADLINE);
    let error = app
        .state
        .reply(caller(0), &reference, message("expired"), None, start)
        .await
        .unwrap_err();
    assert_eq!(error.status, 504);
    assert!(commands.try_recv().is_err());
    let (receipt, _received) = oneshot::channel();
    app.state
        .delivery
        .submit(DeliveryCommand::Typing {
            trace: tracing::Span::none(),
            channel: app.channel.clone(),
            event: app.event(0),
            receipt,
        })
        .await
        .unwrap();
    let start = RequestStart(Instant::now());
    let error = app
        .state
        .reply(caller(0), &reference, message("queue-full"), None, start)
        .await
        .unwrap_err();
    assert_eq!(error.status, 504);
    assert_eq!(error.detail.as_deref(), Some("delivery_timeout"));
    assert_eq!(start.0.elapsed(), TOOL_DEADLINE);
    assert!(matches!(
        commands.recv().await.unwrap(),
        DeliveryCommand::Typing { .. }
    ));
    assert!(commands.try_recv().is_err());
    assert!(app.http.sent().is_empty());
}

#[tokio::test(start_paused = true)]
async fn closed_queue_and_lost_worker_receipt_answer_unavailable() {
    let mut app = App::new(true, true, FakeHttp::relay());
    app.drain().await;
    let reference = app.reference(0);
    let error = app
        .state
        .reply(
            caller(0),
            &reference,
            message("closed"),
            None,
            RequestStart(Instant::now()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.status, 503);
    assert_eq!(error.detail.as_deref(), Some("delivery_unavailable"));
    let (inbound, mut commands) = mpsc::channel(1);
    app.state.delivery = Arc::new(ShardedScheduler::new(inbound));
    let dropped = tokio::spawn(async move {
        drop(commands.recv().await.unwrap());
    });
    let error = app
        .state
        .reply(
            caller(0),
            &reference,
            message("lost receipt"),
            None,
            RequestStart(Instant::now()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.status, 503);
    dropped.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn legacy_clock_starts_at_deliver_and_unstarted_replies_are_skipped() {
    for routed_definition in [false, true] {
        let mut app = App::new(routed_definition, false, FakeHttp::relay());
        app.drain().await;
        let (inbound, commands) = mpsc::channel(16);
        app.state.delivery = Arc::new(ShardedScheduler::new(inbound));
        let reference = app.reference(0);
        let start = RequestStart(Instant::now());
        tokio::time::sleep(Duration::from_secs(5)).await;
        let error = app
            .state
            .reply(caller(0), &reference, message("legacy"), None, start)
            .await
            .unwrap_err();
        assert_eq!(error.status, 504);
        assert_eq!(start.0.elapsed(), Duration::from_secs(13));
        app.engine = Some(support::start_engine(
            commands,
            &app.state,
            app.http.clone(),
        ));
        let http = app.http.clone();
        app.drain().await;
        assert!(http.sent().is_empty());
        // A successful legacy reply still records the handler-side marker.
        let (inbound, commands) = mpsc::channel(16);
        app.state.delivery = Arc::new(ShardedScheduler::new(inbound));
        app.engine = Some(support::start_engine(
            commands,
            &app.state,
            app.http.clone(),
        ));
        let receipt = app
            .state
            .reply(caller(0), &reference, message("confirmed"), None, start)
            .await
            .unwrap();
        assert!(matches!(
            receipt.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        assert!(app.state.tracker.has_tool_reply(
            reference.as_str(),
            time::OffsetDateTime::now_utc().unix_timestamp()
        ));
        app.finish().await;
    }
}

#[tokio::test(start_paused = true)]
async fn routed_updates_and_flag_off_marked_consumers_keep_the_request_clock() {
    for flag in [false, true] {
        let mut app = App::new(true, flag, FakeHttp::platforms());
        app.drain().await;
        let (inbound, commands) = mpsc::channel(16);
        app.state.delivery = Arc::new(ShardedScheduler::new(inbound));
        let start = RequestStart(Instant::now());
        tokio::time::sleep(Duration::from_secs(5)).await;
        let reference = app.reference(1);
        let message_ref = app.message_ref(1);
        let (reply, update) = tokio::join!(
            app.state
                .reply(caller(1), &reference, message("late reply"), None, start),
            app.state
                .update(caller(1), &message_ref, message("late update"), start),
        );
        for receipt in [reply.unwrap(), update.unwrap()] {
            unknown(&serde_json::to_value(receipt).unwrap());
        }
        assert_eq!(start.0.elapsed(), TOOL_DEADLINE);
        app.engine = Some(support::start_engine(
            commands,
            &app.state,
            app.http.clone(),
        ));
        let http = app.http.clone();
        app.finish().await;
        assert_eq!(http.sent().len(), 2);
    }
}

#[tokio::test(start_paused = true)]
async fn typing_retains_its_legacy_deadline_and_answer() {
    for routed in [false, true] {
        let mut app = App::new(routed, true, FakeHttp::relay());
        let receipt = app
            .state
            .typing(caller(0), &app.reference(0))
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(receipt).unwrap(),
            json!({"outcome":"unsupported"})
        );
        app.drain().await;
        let (inbound, _commands) = mpsc::channel(1);
        app.state.delivery = Arc::new(ShardedScheduler::new(inbound));
        let start = Instant::now();
        let error = app
            .state
            .typing(caller(0), &app.reference(0))
            .await
            .unwrap_err();
        assert_eq!(error.status, 504);
        assert_eq!(start.elapsed(), TOOL_DEADLINE);
    }
}
